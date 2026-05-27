# MCP Registration

Use this reference only after the repository-level `setup-for-agent.md` has created `.codedb-mcp/codedb-mcp.toml` and placed the Model2Vec model in the configured explicit cache path.

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
```

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

After registration, restart or reload the agent MCP session and call `codedb_status`. A healthy server reports file count, extensions, graph stats, vector count, embedding model, storage dir, and cache state.
