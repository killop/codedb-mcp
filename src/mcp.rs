use crate::tools::{ProjectManager, dispatch_tool};
use anyhow::Result;
use serde_json::{Value, json};
use std::io::{self, BufRead, Read, Write};
use std::sync::Arc;

pub fn serve(manager: Arc<ProjectManager>) -> Result<()> {
    let stdin = io::stdin();
    let mut reader = io::BufReader::new(stdin.lock());
    let stdout = io::stdout();
    let mut writer = stdout.lock();

    while let Some(message) = read_message(&mut reader)? {
        if message.trim().is_empty() {
            continue;
        }
        let parsed: Value = match serde_json::from_str(&message) {
            Ok(value) => value,
            Err(err) => {
                let response = json!({
                    "jsonrpc": "2.0",
                    "id": Value::Null,
                    "error": {"code": -32700, "message": format!("parse error: {err}")},
                });
                write_message(&mut writer, &response)?;
                continue;
            }
        };

        if let Some(batch) = parsed.as_array() {
            let responses = batch
                .iter()
                .filter_map(|request| handle_request(&manager, request))
                .collect::<Vec<_>>();
            if !responses.is_empty() {
                write_message(&mut writer, &Value::Array(responses))?;
            }
        } else if let Some(response) = handle_request(&manager, &parsed) {
            write_message(&mut writer, &response)?;
        }
    }
    Ok(())
}

fn handle_request(manager: &ProjectManager, request: &Value) -> Option<Value> {
    let id = request.get("id").cloned();
    let method = request
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let is_notification = id.is_none();

    let result = match method {
        "initialize" => Ok(json!({
            "protocolVersion": "2024-11-05",
            "serverInfo": {
                "name": "codebase-mcp",
                "version": env!("CARGO_PKG_VERSION")
            },
            "capabilities": {
                "tools": {"listChanged": false}
            }
        })),
        "notifications/initialized" | "notifications/cancelled" => return None,
        "ping" => Ok(json!({})),
        "tools/list" => Ok(tools_list()),
        "tools/call" => {
            let params = request.get("params").unwrap_or(&Value::Null);
            let name = params
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let args = params.get("arguments").unwrap_or(&Value::Null);
            if name.is_empty() {
                Ok(tool_text("error: missing tool name".to_string(), true))
            } else {
                let text = dispatch_tool(manager, name, args);
                let is_error = text.starts_with("error:");
                Ok(tool_text(text, is_error))
            }
        }
        _ => Err((-32601, format!("method not found: {method}"))),
    };

    if is_notification {
        return None;
    }
    let id = id.unwrap_or(Value::Null);
    Some(match result {
        Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
        Err((code, message)) => json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {"code": code, "message": message},
        }),
    })
}

fn tool_text(text: String, is_error: bool) -> Value {
    json!({
        "content": [{"type": "text", "text": text}],
        "isError": is_error
    })
}

fn read_message<R: BufRead + Read>(reader: &mut R) -> Result<Option<String>> {
    let mut line = String::new();
    loop {
        line.clear();
        let read = reader.read_line(&mut line)?;
        if read == 0 {
            return Ok(None);
        }
        if !line.trim().is_empty() {
            break;
        }
    }

    let trimmed = line.trim_end_matches(['\r', '\n']);
    if let Some(value) = trimmed.strip_prefix("Content-Length:") {
        let len = value.trim().parse::<usize>()?;
        loop {
            line.clear();
            reader.read_line(&mut line)?;
            if line.trim().is_empty() {
                break;
            }
        }
        let mut body = vec![0u8; len];
        reader.read_exact(&mut body)?;
        return Ok(Some(String::from_utf8_lossy(&body).to_string()));
    }

    Ok(Some(trimmed.to_string()))
}

fn write_message<W: Write>(writer: &mut W, value: &Value) -> Result<()> {
    let body = serde_json::to_vec(value)?;
    write!(writer, "Content-Length: {}\r\n\r\n", body.len())?;
    writer.write_all(&body)?;
    writer.flush()?;
    Ok(())
}

pub fn tools_list() -> Value {
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
