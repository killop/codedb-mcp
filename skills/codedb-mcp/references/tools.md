# codedb-mcp Tools

## Search And Symbols

| Tool | Use | Notes |
|---|---|---|
| `codedb_text_search` | Trigram exact/regex full-text search | Use `query` for one lookup or `queries` for batch. Supports `regex=true`, `path_glob`, `compact`, and `scope`. With `compact=true`, it returns file/line/scope evidence instead of source-line text. Stays inside the indexed source corpus. |
| `codedb_search` | Symbol/word-trigram plus natural-language vector search | Use `query` for one lookup or `queries` for batch. Use `compact=true` for discovery. Symbol-shaped default queries stay on the indexed lexical/symbol/text path, while natural-language queries add lazy Model2Vec flat-cosine ranking. `regex=true` delegates to `codedb_text_search`. |
| `codedb_context` | Answer-oriented context builder | Use first for architecture, flow, onboarding, or feature-area questions. It ranks files and returns reasons, hit lines, key symbols, and compact dependency signals without dumping large source bodies. |
| `codedb_explore` | Budgeted source-context explorer | Use after `codedb_context` or directly with a query/path when source snippets are needed. It returns focused outlines, dependencies, and line-numbered excerpts capped by `max_chars`. |
| `codedb_callers` | LSP-like symbol references | Pass `definition_path` and `definition_line` for same-name symbols. Use `targets` for batch. Strongest on C#/Java. |
| `codedb_symbol` | Find definitions by symbol name | Add `body=true` only when the body is needed. |
| `codedb_word` | Exact identifier inverted-index lookup | Fast primitive for debugging reference results. |
| `codedb_outline` | File symbol outline | Prefer this before full reads. |
| `codedb_read` | Indexed file content | Use `path` for one file or `paths` for batch. Line ranges and `compact=true` keep context small; object items in `paths` can override `line_start`, `line_end`, `compact`, and `if_hash`. |

## File Discovery

| Tool | Use | Notes |
|---|---|---|
| `codedb_find` | Fuzzy file/path lookup | Good when a path is remembered approximately. |
| `codedb_glob` | Glob indexed paths | Fast path-set creation. |
| `codedb_ls` | Immediate children under an indexed directory | Use for navigation. |
| `codedb_tree` | Whole indexed tree summary | Use sparingly on large repos. |
| `codedb_hot` | Recently modified indexed files | Good first check after watch rebuilds. |

## Dependencies And Graph

| Tool | Use | Notes |
|---|---|---|
| `codedb_deps` | File dependencies or reverse dependencies | Supports `direction=depends_on/imported_by`, `transitive=true`, and `max_depth`. C#/Java namespace/package imports are currently the most precise typed path; Rust `use`, Lua `require()`, C/C++ includes, and JS/Python imports are also indexed. |
| `codedb_graph` | Graph summary or limited graph export | Formats: `summary`, `json`, `graphml`, `cypher`. |
| `codedb_explain` | Explain a graph node and neighbors | Use with fuzzy node names or labels. |
| `codedb_path` | Shortest graph path between two nodes | Useful for cross-module coupling questions. |
| `codedb_communities` | Lazy Louvain communities and subcommunities | Use as a hint for module discovery, not as final architecture truth. |
| `codedb_module_map` | DeepWiki module-planning atlas | Rust dependency-connected module candidates with dependency cohesion, cross-folder evidence, key symbols, entry points, semantic neighbors, and c-TF-IDF-like labels. Use before writing DeepWiki pages. |
| `codedb_module_atlas` | Module/file atlas export | Writes or returns Rust-generated atlas JSON: modules, file points, terms, roots, central files, and entry points. Use `output_path` for large repos. Use the sibling `code-module-atlas` skill for webpage generation; this skill only exposes the MCP tool. |
| `codedb_analyze` | Graph stats, top nodes, relation counts, suggested questions | Costs more than simple lookup tools; use for planning. |
| `codedb_export` | Write or return graph export | Use `output_path` for large exports. |

## Process And Batch

| Tool | Use | Notes |
|---|---|---|
| `codedb_version` | Server/package version | Does not load a project index. Use for setup checks and release verification. |
| `codedb_status` | Health and index stats | Check after setup, watch rebuild, or benchmark. |
| `codedb_changes` | Files changed since sequence | Useful for incremental agent context. |
| `codedb_index` | Reindex a local folder | Usually not needed when the server watches files. |
| `codedb_bundle` | Up to 100 mixed tool calls | Prefer this whenever a task needs more than one codedb lookup. Batch status, search, outline, read, callers, and dependency calls into one MCP request. Nested `codedb_bundle` is rejected. Output is budgeted by default; set `max_output_chars` and `max_child_chars` explicitly when the task needs tighter or broader evidence, and use `discard_output=true` for benchmark timing. |
| `codedb_query` | Small find/filter/search/limit/outline pipeline | Good for compact exploration without writing a custom loop. |
| `codedb_projects` | Projects loaded in this server process | Mostly diagnostic because storage is project-local under `.codedb-mcp`. |
| `codedb_snapshot` | JSON snapshot of files, symbols, dependencies | Use carefully on large repos. |
| `codedb_edit` | Compatibility stub | Read-only; returns an error. |
| `codedb_remote` | Compatibility stub | Local build does not implement remote queries. |

## Codedb-Only Retrieval Policy

Use the codedb tool surface for repository lookup and stay inside codedb. Use `codedb_context` for broad architecture or flow questions before falling into manual multi-step lookup. Use `codedb_explore` when the answer needs source snippets under an explicit budget. Use `codedb_text_search` for exact or regex text search inside the indexed tree-sitter corpus. Use `codedb_search` for hybrid word-trigram/symbol/vector ranking. Use `codedb_outline` across configured languages, including Rust. Use `codedb_callers` and `codedb_deps` when the task needs code-aware behavior, with the highest confidence on C#/Java symbols. If results look incomplete, inspect `codedb_status`, scan roots, include/exclude rules, watch freshness, and reindex through codedb.

Default to `codedb_bundle` for multi-step investigation. A typical bundle should combine search, outline/read, and dependency or caller follow-up calls that would otherwise be separate MCP round trips. For broad feature analysis, keep the bundle output budgeted so the agent receives a compact evidence map instead of raw source dumps.
