# MCP Installation

Register `assets/codebase-mcp.exe` as an MCP stdio server. The setup script prints the exact paths; it does not edit global MCP settings.

Generic command:

```text
<skill-root>\assets\codebase-mcp.exe --config <repo-root>\.codedb-mcp\codedb-mcp.toml mcp <repo-root>
```

Codex-style TOML shape:

```toml
[mcp_servers.codedb-mcp]
command = "C:\\path\\to\\skills\\codedb-mcp\\assets\\codebase-mcp.exe"
args = [
  "--config",
  "C:\\path\\to\\repo\\.codedb-mcp\\codedb-mcp.toml",
  "mcp",
  "C:\\path\\to\\repo",
]
```

Use `--no-watch` only when the host agent or benchmark needs a static index:

```toml
args = [
  "--config",
  "C:\\path\\to\\repo\\.codedb-mcp\\codedb-mcp.toml",
  "--no-watch",
  "mcp",
  "C:\\path\\to\\repo",
]
```

After registration, call `codedb_status`. A healthy server reports file count, extensions, graph stats, vector count, embedding model, storage dir, and cache state.
