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
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

pub fn serve(manager: Arc<ProjectManager>, watch_enabled: bool) -> Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("failed to create MCP async runtime")?;
    runtime.block_on(async move {
        let server = CodedbServer::new(manager, watch_enabled);
        let running = rmcp::serve_server(server, transport::stdio()).await?;
        let _ = running.waiting().await?;
        Ok(())
    })
}

fn start_background_services(
    manager: Arc<ProjectManager>,
    watch_enabled: bool,
) -> Result<Vec<JoinHandle<()>>> {
    let mut handles = Vec::new();
    handles.push(start_initial_index(manager.clone())?);
    if watch_enabled {
        handles.push(watcher::start_project_watcher(manager)?);
    }
    Ok(handles)
}

fn start_initial_index(manager: Arc<ProjectManager>) -> Result<JoinHandle<()>> {
    std::thread::Builder::new()
        .name("codebase-mcp-initial-index".to_string())
        .spawn(move || {
            if let Err(err) = manager.reindex_default() {
                eprintln!("codebase-mcp initial index failed: {err:#}");
            }
        })
        .context("failed to spawn initial index thread")
}

struct CodedbServer {
    manager: Arc<ProjectManager>,
    watch_enabled: bool,
    startup_started: AtomicBool,
}

impl CodedbServer {
    fn new(manager: Arc<ProjectManager>, watch_enabled: bool) -> Self {
        Self {
            manager,
            watch_enabled,
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
        if let Err(err) = start_background_services(self.manager.clone(), self.watch_enabled) {
            eprintln!("codebase-mcp background startup failed: {err:#}");
        }
    }
}

impl ServerHandler for CodedbServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("codedb-mcp", env!("CARGO_PKG_VERSION")))
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
        let text = dispatch_tool(self.manager.as_ref(), request.name.as_ref(), &args);
        let is_error = text.starts_with("error:");
        let content = vec![Content::text(text)];
        let result = if is_error {
            CallToolResult::error(content)
        } else {
            CallToolResult::success(content)
        };
        future::ready(Ok(result))
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
        return Err(McpError::internal_error("codedb tool list is malformed", None));
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
                "description": "Whole-repo file tree with per-file language, line counts, and symbol counts. This Rust build indexes C# and Java files from the explicit config.",
                "inputSchema": {"type": "object", "properties": {"project": {"type": "string"}}, "required": []}
            },
            {
                "name": "codedb_outline",
                "description": "Symbol outline of one file: classes, methods, properties, imports, and line numbers.",
                "inputSchema": {"type": "object", "properties": {"path": {"type": "string"}, "compact": {"type": "boolean"}, "project": {"type": "string"}}, "required": ["path"]}
            },
            {
                "name": "codedb_symbol",
                "description": "Find where a named symbol is defined across the index.",
                "inputSchema": {"type": "object", "properties": {"name": {"type": "string"}, "body": {"type": "boolean"}, "project": {"type": "string"}}, "required": ["name"]}
            },
            {
                "name": "codedb_search",
                "description": "Search indexed C#/Java code. Pass query for one search, or queries for a batch of strings/objects. Default is BM25 plus local Model2Vec/Vicinity vector search; set regex=true for regex line matching.",
                "inputSchema": {"type": "object", "properties": {"query": {"type": "string"}, "queries": {"type": "array", "items": {"oneOf": [{"type": "string"}, {"type": "object"}]}}, "max_results": {"type": "integer"}, "scope": {"type": "boolean"}, "compact": {"type": "boolean"}, "regex": {"type": "boolean"}, "path_glob": {"type": "string"}, "project": {"type": "string"}}, "required": []}
            },
            {
                "name": "codedb_word",
                "description": "Exact identifier lookup via inverted index.",
                "inputSchema": {"type": "object", "properties": {"word": {"type": "string"}, "project": {"type": "string"}}, "required": ["word"]}
            },
            {
                "name": "codedb_callers",
                "description": "Find references of named symbols with definition anchoring and C#/Java type-reference filtering. Pass name for one lookup, or targets for a batch of strings/objects. Pass definition_path and definition_line to disambiguate same-name symbols.",
                "inputSchema": {"type": "object", "properties": {"name": {"type": "string"}, "targets": {"type": "array", "items": {"oneOf": [{"type": "string"}, {"type": "object"}]}}, "definition_path": {"type": "string"}, "definition_line": {"type": "integer"}, "path": {"type": "string"}, "line": {"type": "integer"}, "max_results": {"type": "integer"}, "project": {"type": "string"}}, "required": []}
            },
            {
                "name": "codedb_hot",
                "description": "Most recently modified indexed files.",
                "inputSchema": {"type": "object", "properties": {"limit": {"type": "integer"}, "project": {"type": "string"}}, "required": []}
            },
            {
                "name": "codedb_deps",
                "description": "Dependency graph from C#/Java imports, packages/namespaces, and symbol references.",
                "inputSchema": {"type": "object", "properties": {"path": {"type": "string"}, "direction": {"type": "string", "enum": ["imported_by", "depends_on"]}, "transitive": {"type": "boolean"}, "max_depth": {"type": "integer"}, "project": {"type": "string"}}, "required": ["path"]}
            },
            {
                "name": "codedb_read",
                "description": "Read indexed file contents, optionally a line range.",
                "inputSchema": {"type": "object", "properties": {"path": {"type": "string"}, "line_start": {"type": "integer"}, "line_end": {"type": "integer"}, "if_hash": {"type": "string"}, "compact": {"type": "boolean"}, "project": {"type": "string"}}, "required": ["path"]}
            },
            {
                "name": "codedb_edit",
                "description": "Compatibility stub. This server is read-only and returns an error for edits.",
                "inputSchema": {"type": "object", "properties": {"path": {"type": "string"}, "op": {"type": "string"}, "content": {"type": "string"}, "range_start": {"type": "integer"}, "range_end": {"type": "integer"}, "after": {"type": "integer"}, "if_hash": {"type": "string"}, "dry_run": {"type": "boolean"}}, "required": ["path", "op"]}
            },
            {
                "name": "codedb_changes",
                "description": "Files changed since a sequence number.",
                "inputSchema": {"type": "object", "properties": {"since": {"type": "integer"}}, "required": []}
            },
            {
                "name": "codedb_status",
                "description": "Current indexed-file count, sequence number, scan state, vector index, and embedding model.",
                "inputSchema": {"type": "object", "properties": {"project": {"type": "string"}}, "required": []}
            },
            {
                "name": "codedb_snapshot",
                "description": "JSON snapshot of files, symbols, and dependency graph.",
                "inputSchema": {"type": "object", "properties": {"project": {"type": "string"}}, "required": []}
            },
            {
                "name": "codedb_bundle",
                "description": "Run up to 100 codedb_* calls in one round trip. Extra ops are reported as truncated.",
                "inputSchema": {"type": "object", "properties": {"ops": {"type": "array", "items": {"type": "object"}}, "project": {"type": "string"}}, "required": ["ops"]}
            },
            {
                "name": "codedb_remote",
                "description": "Compatibility stub for codedb remote queries.",
                "inputSchema": {"type": "object", "properties": {"repo": {"type": "string"}, "action": {"type": "string"}}, "required": ["repo", "action"]}
            },
            {
                "name": "codedb_projects",
                "description": "List locally indexed projects in this server process.",
                "inputSchema": {"type": "object", "properties": {}, "required": []}
            },
            {
                "name": "codedb_index",
                "description": "Index a local folder using the configured source extensions.",
                "inputSchema": {"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"]}
            },
            {
                "name": "codedb_find",
                "description": "Fuzzy file-name search against indexed source file paths.",
                "inputSchema": {"type": "object", "properties": {"query": {"type": "string"}, "max_results": {"type": "integer"}, "project": {"type": "string"}}, "required": ["query"]}
            },
            {
                "name": "codedb_query",
                "description": "Small composable pipeline: find, search, filter, limit, outline.",
                "inputSchema": {"type": "object", "properties": {"pipeline": {"type": "array", "items": {"type": "object"}}, "project": {"type": "string"}}, "required": ["pipeline"]}
            },
            {
                "name": "codedb_glob",
                "description": "Match indexed paths against a glob.",
                "inputSchema": {"type": "object", "properties": {"pattern": {"type": "string"}, "max_results": {"type": "integer"}, "project": {"type": "string"}}, "required": ["pattern"]}
            },
            {
                "name": "codedb_ls",
                "description": "List immediate children of an indexed directory.",
                "inputSchema": {"type": "object", "properties": {"path": {"type": "string"}, "project": {"type": "string"}}, "required": []}
            },
            {
                "name": "codedb_graph",
                "description": "Graphify-style code graph summary or limited export. Formats: summary, json, graphml, cypher.",
                "inputSchema": {"type": "object", "properties": {"format": {"type": "string", "enum": ["summary", "json", "graphml", "cypher"]}, "max_nodes": {"type": "integer"}, "max_edges": {"type": "integer"}, "project": {"type": "string"}}, "required": []}
            },
            {
                "name": "codedb_explain",
                "description": "Explain a graph node by fuzzy node/label match and show incoming/outgoing connections.",
                "inputSchema": {"type": "object", "properties": {"node": {"type": "string"}, "query": {"type": "string"}, "limit": {"type": "integer"}, "project": {"type": "string"}}, "required": []}
            },
            {
                "name": "codedb_path",
                "description": "Find the shortest graph path between two symbols, files, namespaces, or node IDs.",
                "inputSchema": {"type": "object", "properties": {"source": {"type": "string"}, "target": {"type": "string"}, "from": {"type": "string"}, "to": {"type": "string"}, "max_depth": {"type": "integer"}, "project": {"type": "string"}}, "required": []}
            },
            {
                "name": "codedb_communities",
                "description": "List lazy Louvain graph communities, inspect members, or split one community with children=true/subcommunities=true. Results are cached under the project-local storage directory.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "community_id": {"type": "integer"},
                        "id": {"type": "integer"},
                        "child_id": {"type": "integer"},
                        "subcommunity_id": {"type": "integer"},
                        "limit": {"type": "integer"},
                        "community_limit": {"type": "integer"},
                        "include_members": {"type": "boolean"},
                        "members": {"type": "boolean"},
                        "children": {"type": "boolean"},
                        "subcommunities": {"type": "boolean"},
                        "project": {"type": "string"}
                    },
                    "required": []
                }
            },
            {
                "name": "codedb_analyze",
                "description": "Graph analysis: top nodes, relation/type counts, surprising connections, suggested questions.",
                "inputSchema": {"type": "object", "properties": {"top_n": {"type": "integer"}, "project": {"type": "string"}}, "required": []}
            },
            {
                "name": "codedb_export",
                "description": "Export the graph as json, graphml, or cypher; returns text or writes to output_path.",
                "inputSchema": {"type": "object", "properties": {"format": {"type": "string", "enum": ["json", "graphml", "cypher"]}, "output_path": {"type": "string"}, "max_nodes": {"type": "integer"}, "max_edges": {"type": "integer"}, "project": {"type": "string"}}, "required": []}
            }
        ]
    })
}
