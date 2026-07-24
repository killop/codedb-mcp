use crate::config_watcher;
use crate::event_log;
use crate::tools::{ProjectManager, dispatch_tool};
use crate::watcher;
use anyhow::{Context, Result};
use rmcp::{
    ErrorData as McpError, ServerHandler,
    model::{
        CallToolRequestParams, CallToolResult, Content, Implementation, ListToolsResult,
        PaginatedRequestParams, ServerCapabilities, ServerInfo, Tool,
    },
    service::{NotificationContext, RequestContext},
    transport,
};
use serde_json::{Value, json};
use std::future;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

const MCP_INSTRUCTIONS: &str = concat!(
    "Use codedb_graph_query first for discovery and every cross-file continuation; never turn task wording into keywords or guessed identifiers. ",
    "Without an anchor, query EntryFile and BoundaryFile labels plus Community metrics; do not list every File. With an exact endpoint, use incoming patterns; the planner starts from the more selective endpoint. ",
    "Use CALLS/DISPATCHES_TO, REFERENCES, argument/parameter, branch, and shared-state edges before reading bodies. Read one exact body only for local semantics absent from graph facts. ",
    "There is no artificial call, row, output, or token quota; reduce tokens by closing the evidence chain earlier."
);

pub fn serve(
    manager: Arc<ProjectManager>,
    watch_enabled: bool,
    watch_poll_interval: Duration,
    config_path: PathBuf,
) -> Result<()> {
    event_log::emit(|| {
        format!(
            "event=mcp_serve_start watch_enabled={} watch_poll_interval_ms={}",
            watch_enabled,
            watch_poll_interval.as_millis()
        )
    });
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("failed to create MCP async runtime")?;
    runtime.block_on(async move {
        let server = CodedbServer::new(manager, watch_enabled, watch_poll_interval, config_path);
        let running = rmcp::serve_server(server, transport::stdio()).await?;
        let _ = running.waiting().await?;
        Ok(())
    })
}

fn start_background_services(
    manager: Arc<ProjectManager>,
    watch_enabled: bool,
    watch_poll_interval: Duration,
    config_path: PathBuf,
) -> Result<Vec<JoinHandle<()>>> {
    let mut handles = Vec::new();
    event_log::emit(|| "event=background_services_start initial_index=true".to_string());
    handles.push(start_initial_index(manager.clone())?);
    handles.push(config_watcher::start_config_watcher(
        manager.clone(),
        config_path,
        watch_poll_interval,
    )?);
    if watch_enabled {
        event_log::emit(|| {
            format!(
                "event=background_services_start watcher=true poll_interval_ms={}",
                watch_poll_interval.as_millis()
            )
        });
        handles.push(watcher::start_project_watcher(
            manager,
            watch_poll_interval,
        )?);
    }
    Ok(handles)
}

fn start_initial_index(manager: Arc<ProjectManager>) -> Result<JoinHandle<()>> {
    std::thread::Builder::new()
        .name("codebase-mcp-initial-index".to_string())
        .spawn(move || {
            let started = std::time::Instant::now();
            event_log::emit(|| "event=initial_index_start mode=cache_open".to_string());
            match manager.get(None) {
                Ok(index) => {
                    let stats = index.stats();
                    event_log::emit(|| {
                        format!(
                            "event=initial_index_finish mode=cache_open elapsed_ms={:.3} files={} chunks={} symbols={} cache={}",
                            started.elapsed().as_secs_f64() * 1000.0,
                            stats.files,
                            stats.chunks,
                            stats.symbols,
                            stats.cache
                        )
                    });
                }
                Err(err) => {
                    event_log::emit(|| {
                        format!(
                            "event=initial_index_failed elapsed_ms={:.3} error={}",
                            started.elapsed().as_secs_f64() * 1000.0,
                            sanitize_log_value(&err.to_string())
                        )
                    });
                    eprintln!("codebase-mcp initial index failed: {err:#}");
                }
            }
        })
        .context("failed to spawn initial index thread")
}

fn sanitize_log_value(value: &str) -> String {
    value
        .chars()
        .map(|ch| match ch {
            '\r' | '\n' | '\t' | ' ' => '_',
            '\\' => '/',
            _ => ch,
        })
        .collect()
}

struct CodedbServer {
    manager: Arc<ProjectManager>,
    watch_enabled: bool,
    watch_poll_interval: Duration,
    config_path: PathBuf,
    startup_started: AtomicBool,
}

impl CodedbServer {
    fn new(
        manager: Arc<ProjectManager>,
        watch_enabled: bool,
        watch_poll_interval: Duration,
        config_path: PathBuf,
    ) -> Self {
        Self {
            manager,
            watch_enabled,
            watch_poll_interval,
            config_path,
            startup_started: AtomicBool::new(false),
        }
    }

    fn start_background_services_once(&self) {
        if self
            .startup_started
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }
        if let Err(err) = start_background_services(
            self.manager.clone(),
            self.watch_enabled,
            self.watch_poll_interval,
            self.config_path.clone(),
        ) {
            eprintln!("codebase-mcp background startup failed: {err:#}");
        }
    }
}

impl ServerHandler for CodedbServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("codedb-mcp", env!("CARGO_PKG_VERSION")))
            .with_instructions(MCP_INSTRUCTIONS)
    }

    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<rmcp::RoleServer>,
    ) -> impl Future<Output = Result<ListToolsResult, McpError>> + Send + '_ {
        future::ready(list_tools_result())
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        tools_from_json()
            .ok()
            .and_then(|tools| tools.into_iter().find(|tool| tool.name == name))
    }

    fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<rmcp::RoleServer>,
    ) -> impl Future<Output = Result<CallToolResult, McpError>> + Send + '_ {
        let args = Value::Object(request.arguments.unwrap_or_default());
        let name = request.name.to_string();
        let manager = self.manager.clone();
        async move {
            let text =
                tokio::task::spawn_blocking(move || dispatch_tool(manager.as_ref(), &name, &args))
                    .await
                    .map_err(|err| {
                        McpError::internal_error(format!("codedb tool task failed: {err}"), None)
                    })?;
            let is_error = text.starts_with("error:");
            let content = vec![Content::text(text)];
            Ok(if is_error {
                CallToolResult::error(content)
            } else {
                CallToolResult::success(content)
            })
        }
    }

    fn on_initialized(
        &self,
        _context: NotificationContext<rmcp::RoleServer>,
    ) -> impl Future<Output = ()> + Send + '_ {
        self.start_background_services_once();
        future::ready(())
    }
}

fn list_tools_result() -> Result<ListToolsResult, McpError> {
    tools_from_json().map(ListToolsResult::with_all_items)
}

fn tools_from_json() -> Result<Vec<Tool>, McpError> {
    let Some(tools_value) = tools_list().get("tools").cloned() else {
        return Err(McpError::internal_error(
            "codedb tool list is malformed",
            None,
        ));
    };
    let mut tools: Vec<Tool> = serde_json::from_value(tools_value).map_err(|err| {
        McpError::internal_error(format!("failed to build codedb MCP tool list: {err}"), None)
    })?;
    tools.retain(|tool| mcp_tool_is_exposed(tool.name.as_ref()));
    tools.sort_by_key(|tool| tool_priority(tool.name.as_ref()));
    Ok(tools)
}

fn mcp_tool_is_exposed(name: &str) -> bool {
    !matches!(
        name,
        "codedb_changes"
            | "codedb_context"
            | "codedb_diagnostics"
            | "codedb_edit"
            | "codedb_find"
            | "codedb_flow"
            | "codedb_glob"
            | "codedb_hot"
            | "codedb_index"
            | "codedb_ls"
            | "codedb_module_atlas"
            | "codedb_projects"
            | "codedb_query"
            | "codedb_remote"
            | "codedb_search"
            | "codedb_snapshot"
            | "codedb_tree"
            | "codedb_word"
            | "codedb_callpath"
            | "codedb_callers"
            | "codedb_deps"
    )
}

fn tool_priority(name: &str) -> usize {
    match name {
        // Tool order is part of the agent-facing routing surface: graph
        // language first, then exact source projection, then explicit health.
        "codedb_graph_query" => 0,
        "codedb_symbol" => 1,
        "codedb_outline" => 2,
        "codedb_read" => 3,
        "codedb_status" => 4,
        _ => usize::MAX,
    }
}

fn tools_list() -> Value {
    json!({
        "tools": [
            {
                "name": "codedb_graph_query",
                "description": "Atomic Cypher-like read-only graph language: MATCH, optional SHORTEST, WHERE (=, !=, <, <=, >, >=, including property-to-property), RETURN, ORDER BY, optional LIMIT, typed directions, finite *min..max. Labels EntryFile (zero incoming, nonzero outgoing), BoundaryFile, SinkFile, Community, File, Symbol, SharedState, CallSite, Value, Parameter, Condition, ControlAction. Community properties: id/name/size/boundary_links/representative_path. File: path/community/degree/boundary_degree/incoming_degree/outgoing_degree. Main edges: CONTAINS, DEPENDS_ON(count or file_edges), CALLS, DISPATCHES_TO, REFERENCES, HAS_CALLSITE, TARGET, ARGUMENT, BINDS_TO, HAS_PARAMETER, USED_IN, TRUE, FALSE, PREVENTS, REACHES, READS, WRITES. HAS_CALLSITE retains syntax-certain qualified calls with resolution=syntax when no unique target exists; ARGUMENT remains valid, while TARGET/BINDS_TO require precise resolution. Value can be reached through ARGUMENT or seeded by an exact expression predicate.",
                "inputSchema": {"type": "object", "properties": {"query": {"type": "string", "description": "Cypher-like structural query over exact graph labels/properties."}}, "required": ["query"]}
            },
            {
                "name": "codedb_tree",
                "description": "Internal path orientation after graph evidence or a user-provided exact path.",
                "inputSchema": {"type": "object", "properties": {"max_depth": {"type": "integer"}, "max_results": {"type": "integer"}, "path_prefix": {"type": "string"}, "path_glob": {"type": "string"}, "include_files": {"type": "boolean"}, "full": {"type": "boolean"}, "project": {"type": "string"}}, "required": []}
            },
            {
                "name": "codedb_outline",
                "description": "Outline one exact file returned by graph evidence or supplied by the user. Use its real symbols instead of guessing names. If several returned members are needed, rerun once with include_connected_ranges=true to get graph-connected compact read ranges; do not enable it preemptively for every file.",
                "inputSchema": {"type": "object", "properties": {"path": {"type": "string"}, "compact": {"type": "boolean"}, "skeleton": {"type": "boolean"}, "include_connected_ranges": {"type": "boolean", "description": "Emit same-file call/reference connected components only after the base outline proves several members are required."}}, "required": ["path"]}
            },
            {
                "name": "codedb_symbol",
                "description": "One exact symbol definition/body for local semantics. Multi-hop calls, callers, references, dispatch, arguments, guards, state, and branches belong in codedb_graph_query.",
                "inputSchema": {"type": "object", "properties": {"name": {"type": "string", "description": "Exact symbol name copied verbatim from graph or outline evidence."}, "kind": {"type": "string"}, "path": {"type": "string"}, "definition_path": {"type": "string"}, "path_glob": {"type": "string"}, "body": {"type": "boolean"}, "max_results": {"type": "integer"}, "format": {"type": "string", "enum": ["text", "json"]}}, "required": ["name"]}
            },
            {
                "name": "codedb_search",
                "description": "Internal lexical fallback only for an exact identifier, quoted literal, or code token copied from prior codedb evidence. Never use natural-language task words, inferred filenames, keyword bags, synonyms, or guessed lifecycle names; start analysis with codedb_graph_query.",
                "inputSchema": {"type": "object", "properties": {"query": {"type": "string", "description": "Exact code/literal evidence copied from a prior codedb result, not task wording."}, "max_results": {"type": "integer"}, "offset": {"type": "integer"}, "scope": {"type": "boolean"}, "compact": {"type": "boolean"}, "paths_only": {"type": "boolean"}, "regex": {"type": "boolean"}, "path_glob": {"type": "string", "description": "An exact scope returned by graph/path evidence, not a task-derived filename pattern."}, "format": {"type": "string"}, "project": {"type": "string"}}, "required": ["query"]}
            },
            {
                "name": "codedb_word",
                "description": "Exact identifier lookup for a token returned verbatim by prior codedb evidence; not a task-derived discovery entrypoint.",
                "inputSchema": {"type": "object", "properties": {"word": {"type": "string"}, "project": {"type": "string"}}, "required": ["word"]}
            },
            {
                "name": "codedb_callers",
                "description": "Reference/caller sites for an exact symbol returned by graph or body evidence. Use only when runtime ownership or direction remains unresolved.",
                "inputSchema": {"type": "object", "properties": {"name": {"type": "string"}, "max_results": {"type": "integer"}}, "required": ["name"]}
            },
            {
                "name": "codedb_callpath",
                "description": "Preferred atomic graph chain when exact endpoint symbols are already known. A found path includes active bodies; do not re-read its nodes unless a concrete gap remains.",
                "inputSchema": {"type": "object", "properties": {"from": {"type": "string"}, "to": {"type": "string"}, "from_path": {"type": "string"}, "to_path": {"type": "string"}, "from_line": {"type": "integer"}, "to_line": {"type": "integer"}, "max_hops": {"type": "integer"}}, "required": ["from", "to"]}
            },
            {
                "name": "codedb_context",
                "description": "Internal compatibility context pack for an already selected exact scope. Use codedb_graph_query for new source analysis.",
                "inputSchema": {"type": "object", "properties": {"task": {"type": "string"}, "max_tokens": {"type": "integer"}, "max_chars": {"type": "integer"}, "max_files": {"type": "integer"}, "path_glob": {"type": "string"}, "include_deps": {"type": "boolean"}, "include_snippets": {"type": "boolean"}, "snippet_radius": {"type": "integer"}, "snippets_per_file": {"type": "integer"}, "include_inventory": {"type": "boolean"}, "project": {"type": "string"}}, "required": ["task"]}
            },
            {
                "name": "codedb_flow",
                "description": "Internal compatibility flow atlas. MCP source analysis uses codedb_graph_query instead.",
                "inputSchema": {"type": "object", "properties": {"task": {"type": "string"}, "max_tokens": {"type": "integer"}, "max_chars": {"type": "integer"}, "max_files": {"type": "integer"}, "path_glob": {"type": "string"}, "include_inventory": {"type": "boolean"}}, "required": ["task"]}
            },
            {
                "name": "codedb_module_atlas",
                "description": "Dependency-connected module/file atlas JSON.",
                "inputSchema": {"type": "object", "properties": {"limit": {"type": "integer"}, "min_files": {"type": "integer"}, "include_files": {"type": "boolean"}, "split_files": {"type": "boolean"}, "path_prefix": {"type": "string"}, "output_path": {"type": "string"}, "project": {"type": "string"}}, "required": []}
            },
            {
                "name": "codedb_diagnostics",
                "description": "Diagnostics compatibility stub; currently reports that diagnostics are unavailable.",
                "inputSchema": {"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"]}
            },
            {
                "name": "codedb_hot",
                "description": "Recently modified indexed files.",
                "inputSchema": {"type": "object", "properties": {"limit": {"type": "integer"}, "project": {"type": "string"}}, "required": []}
            },
            {
                "name": "codedb_deps",
                "description": "Graph continuation for an exact file returned by prior evidence: forward or reverse dependencies, optionally transitive.",
                "inputSchema": {"type": "object", "properties": {"path": {"type": "string"}, "direction": {"type": "string", "enum": ["imported_by", "depends_on"]}, "transitive": {"type": "boolean"}, "max_depth": {"type": "integer"}}, "required": ["path"]}
            },
            {
                "name": "codedb_read",
                "description": "Read one exact graph-returned file/range when no symbol body fits. A connected range closes same-file source; use codedb_graph_query for outgoing calls, dispatch, argument binding, branch facts, shared state, and exact call-site guards instead of preview text or sequential wrapper reads.",
                "inputSchema": {"type": "object", "properties": {"path": {"type": "string"}, "line_start": {"type": "integer"}, "line_end": {"type": "integer"}, "if_hash": {"type": "string"}, "compact": {"type": "boolean"}, "connected_range": {"type": "boolean", "description": "Read the full active-code connected component, emit a closure marker, retain exact cross-file outgoing handoffs, and show exact incoming callers with call-site preprocessor guards; use only with a range returned by codedb_outline include_connected_ranges=true."}, "include_symbol_leads": {"type": "boolean"}}, "required": ["path"]}
            },
            {
                "name": "codedb_edit",
                "description": "Read-only edit compatibility stub.",
                "inputSchema": {"type": "object", "properties": {"path": {"type": "string"}, "op": {"type": "string", "enum": ["str_replace", "replace", "insert", "delete", "create"]}, "content": {"type": "string"}, "old_string": {"type": "string"}, "new_string": {"type": "string"}, "range_start": {"type": "integer"}, "range_end": {"type": "integer"}, "after": {"type": "integer"}, "if_hash": {"type": "string"}, "dry_run": {"type": "boolean"}}, "required": ["path", "op"]}
            },
            {
                "name": "codedb_changes",
                "description": "Files changed since sequence.",
                "inputSchema": {"type": "object", "properties": {"since": {"type": "integer"}}, "required": []}
            },
            {
                "name": "codedb_status",
                "description": "Index health only when the user asks about setup, freshness, or diagnostics. The server is already bound to its repository; never call status before source analysis.",
                "inputSchema": {"type": "object", "properties": {}, "required": []}
            },
            {
                "name": "codedb_snapshot",
                "description": "Full JSON index snapshot.",
                "inputSchema": {"type": "object", "properties": {"project": {"type": "string"}}, "required": []}
            },
            {
                "name": "codedb_remote",
                "description": "Remote compatibility stub.",
                "inputSchema": {"type": "object", "properties": {"repo": {"type": "string"}, "action": {"type": "string", "enum": ["tree", "outline", "search", "read", "actions", "symbol", "policy", "deps", "score", "cves", "commits", "branches", "dep-history"]}, "query": {"type": "string"}, "path": {"type": "string"}, "lines": {"type": "string"}, "limit": {"type": "integer"}, "offset": {"type": "integer"}, "prefix": {"type": "string"}, "expand": {"type": "boolean"}, "since": {"type": "string"}, "scope": {"type": "string", "enum": ["runtime", "all"]}, "backend": {"type": "string", "enum": ["wiki"]}}, "required": ["repo", "action"]}
            },
            {
                "name": "codedb_projects",
                "description": "Administrative project listing only when the user explicitly asks. A normal MCP server is already bound to the target repository; never call this before source analysis.",
                "inputSchema": {"type": "object", "properties": {}, "required": []}
            },
            {
                "name": "codedb_index",
                "description": "Index a local source folder.",
                "inputSchema": {"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"]}
            },
            {
                "name": "codedb_find",
                "description": "Internal fuzzy path resolution only for a user-provided path hint or an exact path fragment returned by codedb evidence. Never derive filenames from the task; start analysis with codedb_graph_query.",
                "inputSchema": {"type": "object", "properties": {"query": {"type": "string", "description": "User-provided or evidence-returned path fragment, not task wording."}, "max_results": {"type": "integer"}, "include_symbols": {"type": "boolean"}, "project": {"type": "string"}}, "required": ["query"]}
            },
            {
                "name": "codedb_query",
                "description": "Administrative composable pipeline. Prefer atomic graph tools for source analysis.",
                "inputSchema": {"type": "object", "properties": {"pipeline": {"type": "array", "items": {"type": "object"}}, "project": {"type": "string"}}, "required": ["pipeline"]}
            },
            {
                "name": "codedb_glob",
                "description": "Internal exact user-provided or graph-returned path pattern resolution. Never infer filename keywords from the task; start source analysis with codedb_graph_query.",
                "inputSchema": {"type": "object", "properties": {"pattern": {"type": "string", "description": "Exact user-provided or evidence-returned path pattern, not a task-derived keyword glob."}, "max_results": {"type": "integer"}, "include_symbols": {"type": "boolean"}, "include_paths": {"type": "boolean"}, "include_actionable_leads": {"type": "boolean"}, "summary_limit": {"type": "integer"}, "project": {"type": "string"}}, "required": ["pattern"]}
            },
            {
                "name": "codedb_ls",
                "description": "List one exact user-provided or graph-returned directory. Never use for broad walking or as the first source call.",
                "inputSchema": {"type": "object", "properties": {"path": {"type": "string"}, "project": {"type": "string"}}, "required": []}
            }
        ]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundle_is_not_exposed_as_an_mcp_tool() {
        let tools = tools_from_json().unwrap();
        let bundle = tools
            .iter()
            .find(|tool| tool.name.as_ref() == "codedb_bundle");

        assert!(bundle.is_none());
    }

    #[test]
    fn composite_and_administrative_tools_stay_internal() {
        let tools = tools_from_json().unwrap();
        let names = tools
            .iter()
            .map(|tool| tool.name.as_ref())
            .collect::<Vec<_>>();

        for internal in [
            "codedb_callers",
            "codedb_callpath",
            "codedb_changes",
            "codedb_context",
            "codedb_deps",
            "codedb_diagnostics",
            "codedb_edit",
            "codedb_find",
            "codedb_flow",
            "codedb_glob",
            "codedb_hot",
            "codedb_index",
            "codedb_ls",
            "codedb_module_atlas",
            "codedb_projects",
            "codedb_query",
            "codedb_remote",
            "codedb_search",
            "codedb_snapshot",
            "codedb_tree",
            "codedb_word",
        ] {
            assert!(!names.contains(&internal), "{internal} must stay internal");
        }
    }

    #[test]
    fn graph_language_replaces_graph_wrapper_tools() {
        let tools = tools_from_json().unwrap();
        let names = tools
            .iter()
            .map(|tool| tool.name.as_ref())
            .collect::<Vec<_>>();

        assert_eq!(names[0], "codedb_graph_query");
        assert_eq!(names[1], "codedb_symbol");
        assert_eq!(names.last().copied(), Some("codedb_status"));
        assert_eq!(names.len(), 5);
    }

    #[test]
    fn exposed_tools_are_repository_bound_and_symbol_lookup_is_exact() {
        let tools = tools_from_json().unwrap();
        for tool in &tools {
            let value = serde_json::to_value(tool).unwrap();
            assert!(value["inputSchema"]["properties"]["project"].is_null());
        }
        let symbol = tools
            .iter()
            .find(|tool| tool.name.as_ref() == "codedb_symbol")
            .unwrap();
        let value = serde_json::to_value(symbol).unwrap();
        let properties = &value["inputSchema"]["properties"];
        assert!(properties["pattern"].is_null());
        assert!(properties["prefix"].is_null());
        assert!(properties["expand"].is_null());
        assert_eq!(value["inputSchema"]["required"][0], "name");

        let outline = tools
            .iter()
            .find(|tool| tool.name.as_ref() == "codedb_outline")
            .unwrap();
        let value = serde_json::to_value(outline).unwrap();
        assert!(value["inputSchema"]["properties"]["include_body_followups"].is_null());
    }

    #[test]
    fn lexical_tools_reject_task_derived_discovery_in_their_contracts() {
        let tools = tools_list();
        let by_name = |name: &str| {
            tools["tools"]
                .as_array()
                .unwrap()
                .iter()
                .find(|tool| tool["name"] == name)
                .unwrap()
        };

        assert!(
            by_name("codedb_search")["description"]
                .as_str()
                .unwrap()
                .contains("prior codedb evidence")
        );
        assert!(
            by_name("codedb_find")["description"]
                .as_str()
                .unwrap()
                .contains("Never derive filenames from the task")
        );
        assert!(
            by_name("codedb_glob")["description"]
                .as_str()
                .unwrap()
                .contains("codedb_graph_query")
        );
        assert!(MCP_INSTRUCTIONS.contains("Use codedb_graph_query first"));
        assert!(MCP_INSTRUCTIONS.contains("more selective endpoint"));
        assert!(MCP_INSTRUCTIONS.contains("no artificial call"));
    }
}
