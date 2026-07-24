# MCP Registration

Use this reference only after the repository-level `setup-for-agent.md` has created `.codedb-mcp/codedb-mcp.toml` and completed an index check.

Do not silently edit global MCP settings. Ask the human whether this specific agent should register the server, then use that agent's normal MCP mechanism.

Generic command:

```text
<skill-root>\assets\codebase-mcp.exe --config <repo-root>\.codedb-mcp\codedb-mcp.toml mcp <repo-root>
```

Codex-style TOML shape:

```toml
[mcp_servers.codedb-mcp]
command = "<skill-root>\\assets\\codebase-mcp.exe"
args = [
  "--config",
  "<repo-root>\\.codedb-mcp\\codedb-mcp.toml",
  "mcp",
  "<repo-root>",
]

[mcp_servers.codedb-mcp.tools.codedb_graph_query]
approval_mode = "approve"

[mcp_servers.codedb-mcp.tools.codedb_symbol]
approval_mode = "approve"

[mcp_servers.codedb-mcp.tools.codedb_outline]
approval_mode = "approve"

[mcp_servers.codedb-mcp.tools.codedb_read]
approval_mode = "approve"

[mcp_servers.codedb-mcp.tools.codedb_status]
approval_mode = "approve"
```

Use `codedb_graph_query` for both structural discovery and evidence-chain
continuation. Community/File metrics replace the flow-atlas wrapper; callers,
call paths, and dependencies are expressed as graph patterns. Composite and
administrative/compatibility operations remain internal or CLI-only so the MCP
tool surface stays atomic and graph-language-first.

Use `--no-watch` only when the host agent or benchmark needs a static index:

```toml
args = [
  "--config",
  "<repo-root>\\.codedb-mcp\\codedb-mcp.toml",
  "--no-watch",
  "mcp",
  "<repo-root>",
]
```

After registration, restart or reload the agent MCP session and call `codedb_status`. A healthy server reports file count, extensions, graph stats, graph retrieval mode, storage dir, and cache state.
