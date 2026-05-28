# codedb-mcp Tools

## Search And Symbols

| Tool | Use | Notes |
|---|---|---|
| `codedb_search` | Semantic/BM25 search or regex line search | Use `query` for one lookup or `queries` for batch. `regex=true` is closest to `rg`; default search uses indexed lexical and vector ranking. |
| `codedb_callers` | LSP-like symbol references | Pass `definition_path` and `definition_line` for same-name symbols. Use `targets` for batch. Strongest on C#/Java. |
| `codedb_symbol` | Find definitions by symbol name | Add `body=true` only when the body is needed. |
| `codedb_word` | Exact identifier inverted-index lookup | Fast primitive for debugging reference results. |
| `codedb_outline` | File symbol outline | Prefer this before full reads. |
| `codedb_read` | Indexed file content | Use line ranges and `compact=true` to keep context small. |

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
| `codedb_status` | Health and index stats | Check after setup, watch rebuild, or benchmark. |
| `codedb_changes` | Files changed since sequence | Useful for incremental agent context. |
| `codedb_index` | Reindex a local folder | Usually not needed when the server watches files. |
| `codedb_bundle` | Up to 100 mixed tool calls | Use to reduce MCP round trips. Nested `codedb_bundle` is rejected. |
| `codedb_query` | Small find/filter/search/limit/outline pipeline | Good for compact exploration without writing a custom loop. |
| `codedb_projects` | Projects loaded in this server process | Mostly diagnostic because storage is project-local under `.codedb-mcp`. |
| `codedb_snapshot` | JSON snapshot of files, symbols, dependencies | Use carefully on large repos. |
| `codedb_edit` | Compatibility stub | Read-only; returns an error. |
| `codedb_remote` | Compatibility stub | Local build does not implement remote queries. |

## rg Comparison

Use `rg` for raw filesystem search across arbitrary file types. Use `codedb_search regex=true` when exact text search should stay inside the indexed tree-sitter corpus and reuse the warm index. Use `codedb_outline` across all configured languages, including Rust. Use `codedb_callers` and `codedb_deps` when the task needs code-aware behavior that `rg` does not model, with the highest confidence on C#/Java symbols.
