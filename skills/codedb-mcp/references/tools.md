# codedb-mcp Tools

Use exact evidence only. After any `codedb_*` source lookup, stay on `codedb_*` for repo lookup in the same answer.

## Broad Pages

| Tool | Use |
|---|---|
| `codedb_flow` | Without `path_glob`, returns the graph atlas. With `path_glob`, returns structural roots, community boundaries, weighted bridges, symbols, edges, traces, previews, and follow-ups. |
| `codedb_context` | Atlas or scoped evidence pack with optional snippets. Keep snippets focused. |

## Exact Evidence

| Tool | Use |
|---|---|
| `codedb_symbol` | Definitions; use `body=true max_results=1` for one implementation. The default returns the complete body, direct/tail/literal handoffs, and compact deterministic executable corridor paths until a branch, terminal, or cycle. Use `expand=true` only when the corridor bodies or deeper references are needed. Pass `path`/`path_glob` to disambiguate. |
| `codedb_callers` | Reference/caller sites for one symbol. |
| `codedb_callpath` | Atomic graph chain between exact endpoints; a found path includes complete active bodies for its nodes. |
| `codedb_deps` | File imports, reverse imports, or transitive dependency walk. |
| `codedb_outline` | File symbol outline before reading; small files also expose structural body candidates and literal/resource bridge leads. If several listed members are needed, rerun once with `include_connected_ranges=true`, then read one returned compact joint range. Do not request ranges for every file. |
| `codedb_read` | One exact file/range. A `connected_range=true` command returned by outline reads the full active-code component and closes its contained members; do not reopen them individually. Prefer symbol bodies for isolated members. |
| `codedb_search` | Exact text or regex search. |
| `codedb_word` | Exact identifier lookup. |

## Discovery

| Tool | Use |
|---|---|
| `codedb_find` | Fuzzy path lookup; follow with exact evidence. |
| `codedb_glob` | Path glob with central summaries; set `include_paths=true` only when needed. |
| `codedb_ls` | One exact directory. |
| `codedb_tree` | Bounded tree orientation. |
| `codedb_hot` | Recent indexed files. |
| `codedb_module_atlas` | Dependency-connected module/file atlas JSON for inventory or visualization. |

## Runtime

| Tool | Use |
|---|---|
| `codedb_status` | Health/freshness. |
| `codedb_changes` | Changed files since sequence. |
| `codedb_query` | Small find/search/filter/deps/outline/read pipeline. |
| `codedb_index` | Index a local source folder. |
| `codedb_snapshot` | Full JSON snapshot; large. |
| `codedb_edit` | Read-only compatibility stub. |
| `codedb_remote` | Remote compatibility stub. |
| `codedb_diagnostics` | Cached diagnostics when available. |

Policy: start with an unscoped atlas, then project only the entry leaf. A `parent: child(count)` atlas row means scope to `parent/child/**`, not the broad parent. Once an exact body is found, follow its exact handoffs across directories and open another scoped flow only when there is no next path. There is no call quota. `task` is an opaque label; do not derive keywords, synonyms, morphology, language rules, or domain boosts from it. `structural body followups` are the next-body menu for selected files. `body qualified tail call leads` are exact cross-file calls extracted from active source and should be followed before search/find. Literal bridge rows are navigation from exact source strings, not behavior proof; verify the returned body/path. Close the normal main chain before optional failure/reconnect/legacy branches. Do not use shell after the codedb lock starts or treat discovery rows as behavior without exact body/deps/caller/callpath evidence.

Keep calls atomic: consume one scoped flow or exact body before choosing the next graph edge. Do not prefetch multiple regions or unrelated sibling bodies.

Use only symbol names returned verbatim by codedb evidence. After one missing
symbol in a known file, call `codedb_outline` once; never probe a sequence of
invented lifecycle names. The final requested readiness/show body is a hard
stop: answer instead of starting upstream or alternate-readiness verification.
