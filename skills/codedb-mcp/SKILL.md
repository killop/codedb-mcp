---
name: codedb-mcp
description: Operate an already configured local codebase-mcp server for tree-sitter indexed repository search, typed callers, dependency queries, graph analysis, file watching, and project-local .codedb-mcp storage. Use when Codex needs to run codedb_* tools, compare results with rg, inspect index status, or troubleshoot local code search behavior after setup.
---

# codedb-mcp

## Core Rules

- Use the bundled executable at `assets/codebase-mcp.exe` when this skill folder has been copied standalone.
- Keep all project configuration and generated index data under the target repo's `.codedb-mcp` directory.
- Do not rely on environment variables for behavior. Read and edit `.codedb-mcp/codedb-mcp.toml`.
- Do not perform installation from this skill. For setup, use the repository-level `setup-for-agent.md` guide, then ask the human before configuring a specific agent's MCP settings.
- Treat indexed languages as explicit config, not hidden defaults. The template includes C#, Java, Rust, Python, JavaScript/TypeScript, C, and C++; humans can edit `.codedb-mcp/codedb-mcp.toml` before indexing.

## Setup Boundary

This skill does not own setup or MCP registration. If the repo is not configured yet, leave this skill and follow `setup-for-agent.md` from the package root. That guide downloads the model into an explicit configured cache path, writes demo config, and asks the human before any agent-specific MCP registration.

When MCP is already configured, the server command shape is:

```text
<skill-root>\assets\codebase-mcp.exe --config <repo-root>\.codedb-mcp\codedb-mcp.toml mcp <repo-root>
```

MCP mode uses the Rust `rmcp` stdio server, answers the protocol handshake first, and builds the default project index in the background; early tool calls may wait until that initial index is ready. Keep the server alive for editor/agent workflows because warm tool latency is the representative number.

## Tool Use

Load `references/tools.md` when deciding which `codedb_*` tool to call. The common choices are:

- `codedb_search`: semantic/BM25 search or regex line search; supports `queries` batch.
- `codedb_callers`: LSP-like references anchored to a definition; supports `targets` batch. Accuracy is strongest for C#/Java.
- `codedb_deps`: direct or transitive file dependencies and reverse dependencies. C#/Java namespace/package imports are the most precise path.
- `codedb_outline`: precomputed file symbols.
- `codedb_query`: compact find/filter/search/outline pipeline.
- `codedb_bundle`: up to 100 mixed tool calls in one MCP round trip.
- `codedb_status`, `codedb_changes`, `codedb_hot`: health and freshness checks.
- `codedb_graph`, `codedb_communities`, `codedb_module_map`, `codedb_module_atlas`, `codedb_analyze`, `codedb_export`: graph inspection, DeepWiki module planning, viewer export, and graph export.

Use `rg` alongside MCP when validating exact text behavior. Use MCP when the task needs indexed speed, typed references, dependencies, outlines, graph context, project-local cache reuse, or batch operations.

## Operational Checks

After config edits, run:

```powershell
<skill-root>\assets\codebase-mcp.exe --config <repo-root>\.codedb-mcp\codedb-mcp.toml index <repo-root>
```

Then call `codedb_status` through MCP. Confirm:

- `extensions` contains the intended source extensions.
- `storage_dir` points inside `<repo-root>\.codedb-mcp`.
- `cache` is `hit` on repeated opens when files and config are unchanged.

For correctness checks, compare exact MCP regex search with `rg --no-ignore` on the same include/skip baseline, then use `codedb_callers` or `codedb_deps` for code-aware results that `rg` cannot reproduce directly.
