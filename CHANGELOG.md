# Changelog

[中文版本](CHANGELOG.zh-CN.md)

## Unreleased - 2026-05-28

### Changed

- Changed source scanning so nested Git worktrees/submodules under the target root are indexed as normal source directories. `respect_gitignore=true` still honors project `.gitignore` files, but `.git/info/exclude`, global gitignore, and nested Git repository boundaries no longer define the codebase boundary.

### Fixed

- Fixed missing indexes for source files that live inside submodule directories under the main project root.

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
- Recorded multi-language smoke coverage for C#, Java, Rust, Python, TypeScript, C, and C++ paths.
- Recorded warm MCP tool timings for `codedb_search`, `codedb_callers`, `codedb_deps`, `codedb_outline`, `codedb_find`, `codedb_query`, `codedb_analyze`, and `codedb_bundle`.
- Validated `code-module-atlas` on `u3dclient`, generating 16,361 file nodes, 62,771 dependency edges, and 1,374 modules for the viewer dataset.

### Packaging

- Packaged these skills for standalone copying:
  - `skills/codedb-mcp`
  - `skills/deepwiki`
  - `skills/code-module-atlas`
- Ensured project-specific generated atlas files, Vite build output, and `node_modules` stay ignored.
