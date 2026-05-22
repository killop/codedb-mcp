---
name: codedb-mcp
description: Set up, install, and operate the bundled local codebase-mcp server for tree-sitter indexed repository search, typed callers, dependency queries, graph analysis, file watching, and project-local .codedb-mcp storage. Use when Codex needs to configure MCP for a repo, create .codedb-mcp config, run codedb_* tools, compare with rg, or troubleshoot local code search performance.
---

# codedb-mcp

## Core Rules

- Use the bundled executable at `assets/codebase-mcp.exe` when this skill folder has been copied standalone.
- Keep all project configuration and generated index data under the target repo's `.codedb-mcp` directory.
- Do not rely on environment variables for behavior. Read and edit `.codedb-mcp/codedb-mcp.toml`.
- Do not let setup scripts install MCP globally. The agent should install/register the MCP server explicitly in its own MCP configuration.
- Treat indexed languages as explicit config, not hidden defaults. The bundled template includes C#, Java, Python, JavaScript/TypeScript, C, and C++; Unity `Library/PackageCache` is intentionally included while the rest of `Library` is skipped.

## Setup

Run the setup script for the target repository:

```powershell
powershell -ExecutionPolicy Bypass -File <skill-root>\scripts\setup.ps1 -ProjectRoot <repo-root>
```

The script creates `<repo-root>\.codedb-mcp`, writes `codedb-mcp.toml` from `assets/codedb-mcp.toml.template` if missing, and prints the MCP server command to register. Use `-Force` only when the repo config should be replaced by the template.

## MCP Installation

Read `references/mcp-install.md` when registering the server. The essential command shape is:

```text
<skill-root>\assets\codebase-mcp.exe --config <repo-root>\.codedb-mcp\codedb-mcp.toml mcp <repo-root>
```

For an agent MCP config, register the executable as the command and pass the remaining items as args. MCP mode uses the Rust `rmcp` stdio server, answers the protocol handshake first, and builds the default project index in the background; early tool calls may wait until that initial index is ready. Keep the server alive for editor/agent workflows because warm tool latency is the representative number.

## Tool Use

Load `references/tools.md` when deciding which `codedb_*` tool to call. The common choices are:

- `codedb_search`: semantic/BM25 search or regex line search; supports `queries` batch.
- `codedb_callers`: LSP-like references anchored to a definition; supports `targets` batch. Accuracy is strongest for C#/Java.
- `codedb_deps`: direct or transitive file dependencies and reverse dependencies. C#/Java namespace/package imports are the most precise path.
- `codedb_outline`: precomputed file symbols.
- `codedb_query`: compact find/filter/search/outline pipeline.
- `codedb_bundle`: up to 100 mixed tool calls in one MCP round trip.
- `codedb_status`, `codedb_changes`, `codedb_hot`: health and freshness checks.
- `codedb_graph`, `codedb_communities`, `codedb_analyze`, `codedb_export`: graph inspection and export.

Use `rg` alongside MCP when validating exact text behavior. Use MCP when the task needs indexed speed, typed references, dependencies, outlines, graph context, project-local cache reuse, or batch operations.

## Operational Checks

After setup or config edits, run:

```powershell
<skill-root>\assets\codebase-mcp.exe --config <repo-root>\.codedb-mcp\codedb-mcp.toml index <repo-root>
```

Then call `codedb_status` through MCP. Confirm:

- `extensions` contains the intended source extensions.
- `storage_dir` points inside `<repo-root>\.codedb-mcp`.
- `cache` is `hit` on repeated opens when files and config are unchanged.

For correctness checks, compare exact MCP regex search with `rg --no-ignore` on the same include/skip baseline, then use `codedb_callers` or `codedb_deps` for code-aware results that `rg` cannot reproduce directly.
