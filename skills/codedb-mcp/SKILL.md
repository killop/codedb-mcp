---
name: codedb-mcp
description: >-
  For repositories configured with .codedb-mcp, use codedb tools before broad
  source reads, and default to `codedb_bundle` whenever a task needs more than
  one lookup. Batch status, search, outline, read, caller, and dependency checks
  into one MCP round trip; use direct single tools only for truly one-step
  questions. Use `codedb_search` for natural-language, business-concept, or fuzzy
  lookup. Use `codedb_text_search` for exact text, regex, and string search, then
  diagnose missing results with codedb status, scope, and reindex tools. Use
  `codedb_symbol`, `codedb_outline`, and `codedb_read` for definitions, outlines,
  and line-scoped context. Use `codedb_callers` for references/callers, with
  `definition_path` and `definition_line` when known. Use `codedb_deps` for
  dependencies and cross-module relationships. Use `codedb_query` for compact
  find/search/filter/outline pipelines, and `codedb_status`, `codedb_changes`, or
  `codedb_hot` when the index may be stale.
---

# codedb-mcp

## Core Rules

- Use the bundled executable at `assets/codebase-mcp.exe` when this skill folder has been copied standalone.
- Keep all project configuration and generated index data under the target repo's `.codedb-mcp` directory.
- Do not rely on environment variables for behavior. Read and edit `.codedb-mcp/codedb-mcp.toml`.
- Do not perform installation from this skill. For setup, use the repository-level `setup-for-agent.md` guide, then ask the human before configuring a specific agent's MCP settings.
- Treat indexed languages as explicit config, not hidden defaults. The template includes C#, Java, Rust, Python, Lua, JavaScript/TypeScript, C, and C++; humans can edit `.codedb-mcp/codedb-mcp.toml` before indexing.

## Setup Boundary

This skill does not own setup or MCP registration. If the repo is not configured yet, leave this skill and follow `setup-for-agent.md` from the package root. That guide downloads the model into an explicit configured cache path, writes demo config, and asks the human before any agent-specific MCP registration.

When MCP is already configured, the server command shape is:

```text
<skill-root>\assets\codebase-mcp.exe --config <repo-root>\.codedb-mcp\codedb-mcp.toml mcp <repo-root>
```

MCP mode uses the Rust `rmcp` stdio server, answers the protocol handshake first, and builds the default project index in the background; early tool calls may wait until that initial index is ready. Keep the server alive for editor/agent workflows because warm tool latency is the representative number. File freshness is config-driven: `[watch] enabled = true` and `poll_interval_seconds = 5` make the server queue filesystem events and apply them as one serialized batch every 5 seconds. Normal source edits should return `cache: live-incremental`: only new/modified files are parsed, dependency refresh is narrowed to changed-file symbols, and lazy text/search/caller sidecars are rebuilt on demand when their source fingerprint changes. Cache commits are manifest-last generation writes, so an interrupted index should leave the previous cache usable.

## Tool Use

Load `references/tools.md` when deciding which `codedb_*` tool to call. The common choices are:

- Bundle first: when a task needs more than one codedb lookup, use `codedb_bundle` to combine status, search, outline, read, caller, and dependency calls into one MCP request. Use direct single-tool calls only for simple one-step questions.
- Natural-language or fuzzy conceptual code search: use `codedb_search` first instead of reading broad source trees.
- Exact text or regex lookup: use `codedb_text_search`; if results look incomplete, check `codedb_status`, scan scope, watch freshness, and reindex through codedb tools.
- Symbol references or callers: use `codedb_callers` with `definition_path` and `definition_line` when known.
- Definitions, file outlines, or code file context: use `codedb_symbol`, `codedb_outline`, and `codedb_read` before opening large files manually.
- Dependencies and repeated lookups: use `codedb_deps`; use `codedb_bundle` as the default way to reduce MCP round trips and token usage, or `codedb_query` for compact pipeline-style exploration.

- `codedb_text_search`: trigram-accelerated exact/regex full-text search; supports `queries` batch, `path_glob`, compact output, and scopes.
- `codedb_search`: symbol/word-trigram search fused with natural-language vector search; `regex=true` delegates to `codedb_text_search`; supports `queries` batch.
- `codedb_callers`: LSP-like references anchored to a definition; supports `targets` batch. Accuracy is strongest for C#/Java.
- `codedb_deps`: direct or transitive file dependencies and reverse dependencies. C#/Java namespace/package imports are the most precise path.
- `codedb_outline`: precomputed file symbols.
- `codedb_read`: indexed file content; use `paths` for batch reads and line ranges to keep context small.
- `codedb_query`: compact find/filter/search/outline pipeline.
- `codedb_bundle`: up to 100 mixed tool calls in one MCP round trip.
- `codedb_status`, `codedb_changes`, `codedb_hot`: health and freshness checks.
- `codedb_graph`, `codedb_communities`, `codedb_module_map`, `codedb_module_atlas`, `codedb_analyze`, `codedb_export`: graph inspection, DeepWiki module planning, viewer export, and graph export.

Use codedb tools only for repository lookup. Stay inside the configured index and codedb status/config/reindex workflow when results need investigation.

## Operational Checks

After config edits, run:

```powershell
<skill-root>\assets\codebase-mcp.exe --config <repo-root>\.codedb-mcp\codedb-mcp.toml index <repo-root>
```

Then call `codedb_status` through MCP. Confirm:

- `extensions` contains the intended source extensions.
- `root_paths`, `include_paths`, `exclude_paths`, and `skip_dirs` match the intended scan scope. For Unity runtime-only scans, prefer `root_paths = ["Assets", "Packages", "Library/PackageCache"]` plus `exclude_paths = ["**/Editor", "**/Editor/**"]`.
- `storage_dir` points inside `<repo-root>\.codedb-mcp`.
- `cache` is `hit` on repeated opens when files and config are unchanged.
- `[watch] enabled = true` and `poll_interval_seconds = 5` are present unless the human explicitly wants static benchmark behavior.

For correctness checks, use `codedb_status`, `codedb_changes`, `codedb_hot`, scan-scope inspection, and repeated `codedb_text_search`/`codedb_callers`/`codedb_deps` calls rather than switching tools.
