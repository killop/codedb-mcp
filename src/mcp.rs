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
    "Use codedb_* for indexed source lookup. Start broad with unscoped codedb_flow to read the graph atlas, then choose a structural prefix and call scoped codedb_flow with the same opaque task label and path_glob. ",
    "Atlas rows written as parent: child(count) are leaf choices: scope first to the entry leaf parent/child/**, never to the broad parent/**. Once an exact body is found, follow its qualified/tail handoffs across directories; open another scoped flow only when the current evidence has no exact next path. ",
    "Follow exact returned paths/symbols/refs/deps/callpaths progressively. Prefer callpath/deps/symbol body before reads. A symbol body includes compact deterministic executable corridor paths, continuing without a fixed traversal depth until a real branch, terminal, or cycle. Use callpath when endpoints are known; use symbol expand=true only when the compact corridor identifies a needed branch whose bodies are still missing. When one outline shows several answer-critical members in the same file, rerun that outline once with include_connected_ranges=true, then execute one returned connected_range read instead of sequential symbol calls. That read is a complete same-file evidence closure: do not reopen contained members or overlapping ranges. Do not request connected ranges preemptively for every file. ",
    "Follow structural roots, community boundaries, weighted bridges, literal bridge leads, and active-body qualified/tail handoffs before declaring a static gap. Follow an exact qualified call target before search or find. Do not derive keywords, synonyms, facets, morphology, language rules, or repository-specific search terms from the task. ",
    "Never invent or probe lifecycle symbol names. Request only symbols returned verbatim by flow, outline, body handoffs, callers, search, or callpath; after one missing exact symbol in a known file, call that file's outline once instead of guessing siblings. ",
    "There is no source-call quota, but stop when every requested phase has one active body, adjacent phases have a direct handoff or graph path, and the final readiness callback has an active body. The final endpoint body is a hard stop: answer immediately. Traverse phases once; do not return to closed phases for constants, assets, compile variants, callers, later readiness variants, or a second verification pass unless downstream evidence contradicts the chain."
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
    serde_json::from_value(tools_value).map_err(|err| {
        McpError::internal_error(format!("failed to build codedb MCP tool list: {err}"), None)
    })
}

fn tools_list() -> Value {
    json!({
        "tools": [
            {
                "name": "codedb_tree",
                "description": "Bounded tree summary; focus with path_prefix/path_glob.",
                "inputSchema": {"type": "object", "properties": {"max_depth": {"type": "integer"}, "max_results": {"type": "integer"}, "path_prefix": {"type": "string"}, "path_glob": {"type": "string"}, "include_files": {"type": "boolean"}, "full": {"type": "boolean"}, "project": {"type": "string"}}, "required": []}
            },
            {
                "name": "codedb_outline",
                "description": "File symbol outline with structural body candidates and literal bridge leads. If several returned members are needed, rerun once with include_connected_ranges=true to get graph-connected compact read ranges; do not enable it preemptively for every file.",
                "inputSchema": {"type": "object", "properties": {"path": {"type": "string"}, "compact": {"type": "boolean"}, "skeleton": {"type": "boolean"}, "include_body_followups": {"type": "boolean"}, "include_connected_ranges": {"type": "boolean", "description": "Emit same-file call/reference connected components as exact compact read ranges; request only after the base outline shows several needed members."}, "project": {"type": "string"}}, "required": ["path"]}
            },
            {
                "name": "codedb_symbol",
                "description": "Symbol lookup; body=true returns the complete active body first plus direct/tail/literal handoffs and compact deterministic executable corridor paths until a branch, terminal, or cycle. Add expand=true only when those corridor bodies or deeper references are actually needed.",
                "inputSchema": {"type": "object", "properties": {"name": {"type": "string"}, "prefix": {"type": "string"}, "pattern": {"type": "string"}, "kind": {"type": "string"}, "path": {"type": "string"}, "definition_path": {"type": "string"}, "path_glob": {"type": "string"}, "fuzzy": {"type": "boolean"}, "body": {"type": "boolean"}, "expand": {"type": "boolean", "description": "Add deep reference/continuation evidence after the complete body; default false for progressive disclosure."}, "max_results": {"type": "integer"}, "format": {"type": "string", "enum": ["text", "json"]}, "project": {"type": "string"}}, "required": []}
            },
            {
                "name": "codedb_search",
                "description": "Definition-first lexical code search: exact/regex text, BM25, symbols, and word-trigram evidence.",
                "inputSchema": {"type": "object", "properties": {"query": {"type": "string"}, "max_results": {"type": "integer"}, "offset": {"type": "integer"}, "scope": {"type": "boolean"}, "compact": {"type": "boolean"}, "paths_only": {"type": "boolean"}, "regex": {"type": "boolean"}, "path_glob": {"type": "string"}, "format": {"type": "string"}, "project": {"type": "string"}}, "required": ["query"]}
            },
            {
                "name": "codedb_word",
                "description": "Exact identifier lookup.",
                "inputSchema": {"type": "object", "properties": {"word": {"type": "string"}, "project": {"type": "string"}}, "required": ["word"]}
            },
            {
                "name": "codedb_callers",
                "description": "Reference/caller sites for a symbol.",
                "inputSchema": {"type": "object", "properties": {"name": {"type": "string"}, "max_results": {"type": "integer"}, "project": {"type": "string"}}, "required": ["name"]}
            },
            {
                "name": "codedb_callpath",
                "description": "Generic symbol-reference path.",
                "inputSchema": {"type": "object", "properties": {"from": {"type": "string"}, "to": {"type": "string"}, "from_path": {"type": "string"}, "to_path": {"type": "string"}, "from_line": {"type": "integer"}, "to_line": {"type": "integer"}, "max_hops": {"type": "integer"}, "project": {"type": "string"}}, "required": ["from", "to"]}
            },
            {
                "name": "codedb_context",
                "description": "Graph atlas without path_glob; scoped structural roots, community boundaries, weighted bridges, calls, and optional snippets with path_glob. Task text is an opaque label.",
                "inputSchema": {"type": "object", "properties": {"task": {"type": "string"}, "max_tokens": {"type": "integer"}, "max_chars": {"type": "integer"}, "max_files": {"type": "integer"}, "path_glob": {"type": "string"}, "include_deps": {"type": "boolean"}, "include_snippets": {"type": "boolean"}, "snippet_radius": {"type": "integer"}, "snippets_per_file": {"type": "integer"}, "include_inventory": {"type": "boolean"}, "project": {"type": "string"}}, "required": ["task"]}
            },
            {
                "name": "codedb_flow",
                "description": "Graph atlas without path_glob; scoped structural roots, community boundaries, weighted bridges, calls, and bodies with path_glob. Task text is an opaque label.",
                "inputSchema": {"type": "object", "properties": {"task": {"type": "string"}, "max_tokens": {"type": "integer"}, "max_chars": {"type": "integer"}, "max_files": {"type": "integer"}, "path_glob": {"type": "string"}, "include_inventory": {"type": "boolean"}, "project": {"type": "string"}}, "required": ["task"]}
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
                "description": "File dependencies or reverse dependencies.",
                "inputSchema": {"type": "object", "properties": {"path": {"type": "string"}, "direction": {"type": "string", "enum": ["imported_by", "depends_on"]}, "transitive": {"type": "boolean"}, "max_depth": {"type": "integer"}, "project": {"type": "string"}}, "required": ["path"]}
            },
            {
                "name": "codedb_read",
                "description": "Read one exact file or line range. Use a connected range command returned by outline as a complete same-file evidence closure; do not reopen its contained members individually.",
                "inputSchema": {"type": "object", "properties": {"path": {"type": "string"}, "line_start": {"type": "integer"}, "line_end": {"type": "integer"}, "if_hash": {"type": "string"}, "compact": {"type": "boolean"}, "connected_range": {"type": "boolean", "description": "Read the full active-code connected component without the ordinary compact-line cap and emit a closure marker; use only with a range returned by codedb_outline include_connected_ranges=true."}, "include_symbol_leads": {"type": "boolean"}, "project": {"type": "string"}}, "required": ["path"]}
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
                "description": "Index status.",
                "inputSchema": {"type": "object", "properties": {"project": {"type": "string"}}, "required": []}
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
                "description": "Loaded projects.",
                "inputSchema": {"type": "object", "properties": {}, "required": []}
            },
            {
                "name": "codedb_index",
                "description": "Index a local source folder.",
                "inputSchema": {"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"]}
            },
            {
                "name": "codedb_find",
                "description": "Fuzzy path search.",
                "inputSchema": {"type": "object", "properties": {"query": {"type": "string"}, "max_results": {"type": "integer"}, "include_symbols": {"type": "boolean"}, "project": {"type": "string"}}, "required": ["query"]}
            },
            {
                "name": "codedb_query",
                "description": "Composable lookup pipeline.",
                "inputSchema": {"type": "object", "properties": {"pipeline": {"type": "array", "items": {"type": "object"}}, "project": {"type": "string"}}, "required": ["pipeline"]}
            },
            {
                "name": "codedb_glob",
                "description": "Glob indexed paths with central summaries.",
                "inputSchema": {"type": "object", "properties": {"pattern": {"type": "string"}, "max_results": {"type": "integer"}, "include_symbols": {"type": "boolean"}, "include_paths": {"type": "boolean"}, "include_actionable_leads": {"type": "boolean"}, "summary_limit": {"type": "integer"}, "project": {"type": "string"}}, "required": ["pattern"]}
            },
            {
                "name": "codedb_ls",
                "description": "List one directory.",
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
        let tools = tools_list();
        let bundle = tools["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["name"] == "codedb_bundle");

        assert!(bundle.is_none());
    }
}
