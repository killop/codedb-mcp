# Changelog

[中文版本](CHANGELOG.zh-CN.md)

## Unreleased - 2026-05-28

### Added

- Added Lua language support through `tree-sitter-lua`, including `.lua` scanning, `require()` import extraction, common Lua function outline extraction, and Lua comment handling for compact search output.

### Changed

- Changed source scanning so nested Git worktrees/submodules under the target root are indexed as normal source directories. `respect_gitignore=true` still honors project `.gitignore` files, but `.git/info/exclude`, global gitignore, and nested Git repository boundaries no longer define the codebase boundary.
- Reduced warm-index memory on large projects by storing identifier hits as compact file ids, keeping large-project graph nodes at file/namespace/dependency level while preserving symbol data in outline/search/callers indexes, building BM25 postings without a full temporary token corpus, and moving cached full-file source bodies to on-demand reads.
- Reduced warm cache-hit memory further by making the graph, reverse dependencies, BM25 postings, embedding vectors, and Model2Vec/vector store lazy. Symbol-shaped `codedb_search` queries now stay on the BM25/symbol path and avoid loading embeddings.
- Replaced the resident Vicinity HNSW vector index with a lazy flat-cosine file-vector scan for natural-language search, removing the HNSW dependency and its graph memory.
- Compacted repeated in-memory metadata by storing symbol kinds and source languages as small enums and storing chunk file paths as file ids instead of duplicating path strings per chunk.
- Moved forward dependencies into a lazy `deps.bin` sidecar so search/status/callers do not keep the dependency graph resident; dependency and graph/module tools load it on demand.
- Split cache v20 into a small JSON manifest plus binary source fingerprints, a compact hot `index.bin`, spilled BM25 postings, lazy word-index sidecars, lazy dependencies, and on-demand embeddings. One-shot `codedb_status`, `codedb_find`, and `codedb_deps` can now answer from sidecars without deserializing the full index.
- Added a BM25-enough fast path for business phrase searches so common multi-token queries can return lexical results without loading Model2Vec, and reused file-content reads while formatting search previews.
- Added a lazy `callers.bin` sidecar for definition-anchored `codedb_callers` results. The first uncached target still uses the full caller path and writes the sidecar; repeated one-shot lookups can return directly from the sidecar without loading the full index.
- Reworked cold indexing as cache v20: per-file source bodies are dropped immediately after tree-sitter parsing and chunk metadata generation, dependencies and BM25 reread source on demand, BM25 construction spills doc-term records to disk, Model2Vec embeddings are generated lazily, and cold indexing no longer clones file/symbol metadata into a second map before cache save.

### Fixed

- Fixed missing indexes for source files that live inside submodule directories under the main project root.
- Fixed very small indexed projects failing vector-store construction when embedding output was empty by using the configured model dimension as the vector-store fallback.

### Benchmark And Validation

- Re-measured `u3dclient` after cache v20: 19,035 indexed files, 31,949 chunks, 277,213 symbols, and an estimated 19,941-node / 166,132-edge graph that is built lazily for graph/module tools.
- Re-ran the cache v20 cold rebuild with peak-memory sampling on `u3dclient`: 26.335s internal / 26.621s wall, 256.4 MB working set, and 250.2 MB private bytes.
- Re-ran cache-hit index open after cache v20: 0.873s internal / 1.132s wall, 134.9 MB working set, and 136.0 MB private bytes.
- Re-ran fast one-shot wall-time and peak-memory checks on `u3dclient`: `codedb_status` 0.252s at 14.1 MB WS / 7.9 MB private, `codedb_find PoolManager` 0.283s at 14.4 MB WS / 8.2 MB private, `codedb_deps PoolManager.cs` 0.303s at 34.8 MB WS / 28.3 MB private, `codedb_search PoolManager` 0.739s at 151.5 MB WS / 154.8 MB private, and `codedb_callers PoolManager` sidecar hit 0.243s at 14.2 MB WS / 7.8 MB private.
- Re-ran the `gameserver` Java benchmark after fixing its explicit model path: 6,940 files, 55,057 chunks, and 245,238 symbols rebuild in 10.477s, then reopen from cache in 1.027s.
- Updated the README `rg` comparison to reflect the cache v20 memory tradeoff: broad unscoped regex reads source files on demand and is slower than `rg`, while path-scoped regex, symbol search, callers, deps, outlines, and bundle calls remain low-latency.
- Verified source-on-demand tools after removing resident full-file bodies: `codedb_search PoolManager`, definition-anchored `codedb_callers PoolManager`, and `codedb_read PoolManager.cs`.

## Unreleased - 2026-05-27

### Added

- Added the `skills/code-module-atlas` skill. It calls the existing `codedb_module_atlas` MCP tool, converts the exported module/file graph into the bundled meet-blog-style viewer dataset, and serves a local 3D code atlas webpage.
- Added a self-contained code atlas viewer under `skills/code-module-atlas/assets/viewer`, including vendored frontend assets, dataset conversion, frontend patching, Vite build, and run scripts.
- Added `setup-for-agent.md` as the explicit setup guide. Setup now lives outside the `codedb-mcp` skill and tells agents to create project-local `.codedb-mcp` config, resolve the model path, and ask before registering MCP for a specific agent.
- Added README demo assets:
  - `docs/assets/code-module-atlas.gif`
  - `docs/assets/code-module-atlas.mp4`
- Added Rust language support to the unified tree-sitter language layer and documented the current multi-language support matrix.

### Changed

- Consolidated all module-atlas webpage code into `skills/code-module-atlas`; the rest of the repository now treats `codedb_module_atlas` as the Rust/MCP data export layer only.
- Updated `skills/codedb-mcp` to focus on operating an already configured MCP server. It no longer owns setup or agent-specific MCP registration.
- Updated `skills/deepwiki` to keep DeepWiki planning separate from visual atlas generation. DeepWiki uses `codedb_module_map` for page planning and delegates visual module/file graph generation to `code-module-atlas`.
- Updated the module planning flow to prefer dependency-connected file components and dependency-weighted label propagation, with paths and terms used as labels/evidence rather than the primary grouping basis.
- Updated config guidance to keep all behavior explicit in `.codedb-mcp/codedb-mcp.toml`, including language extensions, include paths, storage, and absolute model path.
- Updated scan defaults and documentation for large source files, multi-language extensions, and Unity `Library/PackageCache` inclusion through `include_paths`.
- Updated README and README.zh-CN with architecture notes, benchmark tables, MCP vs `rg` comparison, skill packaging guidance, and the new Code Module Atlas demo.

### Removed

- Removed the old `skills/codedb-mcp/scripts/setup.ps1` setup path from the skill packaging model.
- Removed the duplicate DeepWiki `module-atlas-workflow.md` reference so module-atlas workflow documentation has one owner.
- Removed the old external `tools/module-atlas-viewer` maintenance path; generated viewer data is ignored and not committed.

### Benchmark And Validation

- Recorded Unity C# benchmark data for `u3dclient`: 19,030 indexed files, 129,790 chunks, 277,008 symbols, 296,941 graph nodes, and 691,419 graph edges.
- Recorded Java benchmark data for `gameserver`: 6,940 files, 55,057 chunks, and 245,238 symbols.
- Recorded multi-language smoke coverage for C#, Java, Rust, Python, Lua, TypeScript, C, and C++ paths.
- Recorded warm MCP tool timings for `codedb_search`, `codedb_callers`, `codedb_deps`, `codedb_outline`, `codedb_find`, `codedb_query`, `codedb_analyze`, and `codedb_bundle`.
- Validated `code-module-atlas` on `u3dclient`, generating 16,361 file nodes, 62,771 dependency edges, and 1,374 modules for the viewer dataset.

### Packaging

- Packaged these skills for standalone copying:
  - `skills/codedb-mcp`
  - `skills/deepwiki`
  - `skills/code-module-atlas`
- Ensured project-specific generated atlas files, Vite build output, and `node_modules` stay ignored.
