# codebase-mcp

[中文 README](README.zh-CN.md)

Rust MCP server with a `codedb_*` compatible tool surface and a local Minish-style search core for tree-sitter indexed codebases.

## Demo

![Code Module Atlas demo](docs/assets/code-module-atlas.gif)

[Watch the MP4 demo](docs/assets/code-module-atlas.mp4)

The intended distribution model is setup-guide first: give an agent `setup-for-agent.md`, let it create `.codedb-mcp`, use the default HuggingFace cache when it already exists, fall back to a second-drive cache when it does not, and then ask the human whether this specific agent should register the MCP server. The `codedb-mcp` skill is for using the tools after setup, not for installing them.

## Benchmark Snapshot

Benchmark target: `u3dclient`.

Current index status with the Unity C# benchmark config:

- Indexed files: 19,030.
- Chunks: 129,790.
- Symbols: 277,008.
- Graph: 296,941 nodes and 691,419 edges.
- Vector index: Vicinity HNSW over Model2Vec `minishlab/potion-code-16M`.
- Storage: `u3dclient\.codedb-mcp`.

Index timings on this machine:

| Scenario | Cache | Internal total | Notes |
|---|---|---:|---|
| Cold tree-sitter `.codedb-mcp` build | miss | 66.061s wall | scan, tree-sitter declaration parse, embeddings, graph, BM25, HNSW, cache save |
| Reopen with unchanged files/config | hit | 30.8s wall | reuses parsed files, chunks, semantic units, embeddings; rebuilds runtime graph/BM25/HNSW |
| One-shot `codedb_status` CLI | hit | 31.0s wall | includes process startup and index load; persistent MCP is the intended mode |
| One-shot Rust `codedb_module_atlas` export | hit | 42.057s wall | includes cache-hit index load plus atlas JSON export |
| Warm Rust module atlas generation | ready | 9.746s internal | 1,374 modules and 16,361 plotted files from dependency-connected file graph |

Java smoke benchmark on `gameserver`:

| Scenario | Files | Chunks | Symbols | Time |
|---|---:|---:|---:|---:|
| Cold tree-sitter build | 6,940 | 55,057 | 245,238 | 16.919s |
| Reopen with unchanged files/config | 6,940 | 55,057 | 245,238 | 11.527s |

Multi-language smoke benchmark with C#, Java, Python, TypeScript, and C++ files: 5 files, 5 chunks, 12 symbols, 1.147s.
Rust smoke check on this repository: 20 indexed files including 17 `.rs` files, 341 chunks, 604 symbols; `codedb_outline`, `codedb_search`, and `codedb_deps` all returned Rust results.

Warm persistent MCP tool timings below do not include server startup or index load.

## MCP vs rg

For exact text and regex search, `codedb_search regex=true` and `rg` can both answer the query. The `rg` baseline used `--no-ignore` because this Unity project intentionally includes `Library/PackageCache`.

| Scenario | MCP tool | MCP hits | MCP time | rg baseline | rg hits | rg time |
|---|---|---:|---:|---|---:|---:|
| Exact `PoolManager` | `codedb_search regex=true` | 154 | 0.2234s | `rg --no-ignore -n -i -F` | 154 | 1.7201s |
| Exact `Joystick` | `codedb_search regex=true` | 938 | 0.2343s | `rg --no-ignore -n -i -F` | 938 | 1.9419s |
| Exact `NetworkListenerManager` | `codedb_search regex=true` | 14 | 0.1973s | `rg --no-ignore -n -i -F` | 14 | 1.7486s |
| Exact `GameObjectPoolMgr` | `codedb_search regex=true` | 8 | 0.2210s | `rg --no-ignore -n -i -F` | 8 | 2.1606s |
| Exact `AllianceManager` | `codedb_search regex=true` | 16 | 0.2190s | `rg --no-ignore -n -i -F` | 16 | 1.7719s |
| Scoped `Joystick` in Joystick Pack | `codedb_search regex=true path_glob=...` | 46 | 0.0063s | scoped `rg --no-ignore -n -i -F` | 46 | 0.0415s |
| Scoped `NetworkListenerManager` in UnityNativeTools | `codedb_search regex=true path_glob=...` | 14 | 0.0064s | scoped `rg --no-ignore -n -i -F` | 14 | 0.0414s |
| Alliance UI/proto regex in `Assets/Scripts` | `codedb_search regex=true path_glob=...` | 409 | 0.0635s | scoped `rg --no-ignore -n -i` | 409 | 0.4137s |
| Alliance UI `.cs` file glob | `codedb_glob` | 52 | 0.0044s | `rg --files --no-ignore -g` | 52 | 0.5748s |

Feature comparison:

| Capability | codedb-mcp | rg |
|---|---|---|
| Raw exact grep | yes, indexed via `codedb_search regex=true` | yes |
| Regex line search | yes, indexed source corpus | yes, direct filesystem scan |
| Scoped file/path filtering | yes, `path_glob`, `codedb_find`, `codedb_query` | yes, `-g`, shell paths |
| Fuzzy file lookup | yes, `codedb_find` | no direct fuzzy ranking |
| Hybrid lexical + vector search | yes, BM25 + Model2Vec + Vicinity | no |
| Symbol outline | yes, `codedb_outline` from precomputed tree-sitter symbols | no |
| Definition-anchored callers | yes, `codedb_callers` | no semantic anchor |
| File dependency graph | yes, `codedb_deps` | no |
| Code graph export/analysis | yes, `codedb_graph`, `codedb_analyze`, `codedb_export` | no |
| Batch calls in one MCP round trip | yes, batch params and `codedb_bundle` | no MCP batching |
| Arbitrary non-indexed binary/text corpus | no, source extensions from config | yes |

MCP-only measured features:

| Scenario | MCP tool | Results | Time | rg equivalent |
|---|---|---:|---:|---|
| Hybrid search for `PoolManager` related chunks | `codedb_search` | 20 | 0.0198s | none |
| Hybrid search for `Joystick` related chunks | `codedb_search` | 20 | 0.0666s | none |
| Hybrid search for `NetworkListenerManager` related chunks | `codedb_search` | 20 | 0.0271s | none |
| Business semantic search: `alliance member ranking donation gift` under `Assets/Scripts` | `codedb_search path_glob=...` | 20 | 0.0358s | none |
| Definition-anchored references for `PoolManager` at `PoolManager.cs:26` | `codedb_callers` | 7 | 0.0045s | none |
| Definition-anchored references for `Joystick` at `Joystick.cs:8` | `codedb_callers` | 7 | 0.0069s | none |

`rg` remains the better tool for ad hoc raw filesystem grep across arbitrary file types. `codedb-mcp` is for repeated code-aware work inside a configured source corpus.

## Warm Tool Validation

These calls were measured through one already-started MCP process on `u3dclient`; exact regex searches were checked against `rg --no-ignore` on the same scoped corpus.

| Scenario | Tool | Accuracy Check | avg | p95 |
|---|---|---:|---:|---:|
| Scoped exact `PoolManager` | `codedb_search regex=true` | MCP 52 = rg 52 | 5.813ms | 5.953ms |
| Scoped exact `Joystick` | `codedb_search regex=true` | MCP 46 = rg 46 | 6.371ms | 6.853ms |
| Scoped exact `NetworkListenerManager` | `codedb_search regex=true` | MCP 14 = rg 14 | 6.486ms | 6.707ms |
| Hybrid `PoolManager` | `codedb_search` | expected text present | 20.826ms | 21.723ms |
| Hybrid `Joystick` | `codedb_search` | expected text present | 84.755ms | 84.621ms |
| Business phrase `alliance member ranking donation gift` | `codedb_search` | Alliance results present | 39.849ms | 41.138ms |
| Definition refs for `PoolManager` | `codedb_callers` | 7 refs | 4.518ms | 5.464ms |
| Definition refs for `Joystick` | `codedb_callers` | 7 refs | 7.726ms | 8.692ms |
| `GameObjectPoolMgr.cs depends_on` | `codedb_deps` | 7 files, expected deps present | 0.244ms | 0.318ms |
| `NetworkListenerManager.cs imported_by` | `codedb_deps` | 3 files, expected importers present | 0.193ms | 0.212ms |
| `NetworkListenerManager.cs transitive imported_by` | `codedb_deps` | 16 files | 0.192ms | 0.230ms |
| `NetworkListenerManager.cs` path lookup | `codedb_find` | top1 correct | 20.259ms | 21.108ms |
| `Joystick Pack Base Joystick` path lookup | `codedb_find` | top1 correct | 17.710ms | 18.054ms |
| `ResTypDef` typo-ish lookup | `codedb_find` | target rank 3 | 19.109ms | 20.027ms |
| `find NetworkListenerManager -> outline` | `codedb_query` | expected outline present | 20.173ms | 20.505ms |
| `filter Joystick Pack -> limit 3 -> outline` | `codedb_query` | expected outlines present | 8.017ms | 9.206ms |
| `filter UnityNativeTools -> search NetworkListenerManager` | `codedb_query` | expected results present | 9.650ms | 10.755ms |
| `find GameObjectPoolMgr -> search PoolManager` | `codedb_query` | expected results present | 22.019ms | 23.469ms |

Additional tool timings:

| Tool / Scenario | Result | Time |
|---|---:|---:|
| `codedb_deps` `GameObjectPoolMgr.cs depends_on` | 7 files | 0.0002s |
| `codedb_deps` `NetworkListenerManager.cs imported_by` | 3 files | 0.0002s |
| `codedb_deps` `AndroidPlatform.cs depends_on` | 3 files | 0.0002s |
| `codedb_outline` `NetworkListenerManager.cs` | 1 symbol | 0.3ms |
| `codedb_outline` `Joystick.cs` | 17 symbols | 0.3ms |
| `codedb_outline` `PoolManager.cs` | 32 symbols | 0.2ms |
| `codedb_outline` `NEON_AArch64.cs` | 2,211 symbols | 1.4ms |
| 100 `codedb_outline compact=true` calls | p95 | 0.3ms |
| `codedb_analyze` on `u3dclient` | graph analysis | about 0.93s |

`codedb_bundle` runs up to 100 inner operations in one MCP request. Requests above 100 execute the first 100 and include a truncation notice.

| Scenario | Inner ops requested | Repeats | Inner ops executed | Time |
|---|---:|---:|---:|---:|
| Fast mixed metadata/deps/outline/read bundle | 100 | 1 | 100 | 0.0895s |
| Overflow bundle | 120 | 1 | 100 + truncation notice | 0.0924s |
| Repeated fast bundle | 100 | 50 | 5,000 total | avg 0.0913s, p95 0.1084s |
| Mixed search/callers/deps/outline bundle | 100 | 1 | 100 | 2.3174s |
| Heavy regex search bundle | 100 | 1 | 100 | 26.0085s |

## Recommended Setup Flow

1. Give the target agent `setup-for-agent.md`.
2. The agent creates `<repo-root>\.codedb-mcp` and `<repo-root>\.codedb-mcp\models`.
3. On Windows, the agent checks the default HuggingFace hub cache first. If `minishlab/potion-code-16M` already has a valid snapshot there, config points to that snapshot. If the hub cache exists but the model is missing, the agent downloads to `C:\Users\<user>\.cache\huggingface\hub\codedb-mcp\models\potion-code-16M`. If the default hub cache does not exist, it uses the second available drive, such as `D:\codedb-mcp-cache\models\potion-code-16M`.
4. The agent writes `<repo-root>\.codedb-mcp\codedb-mcp.toml` from the demo config, writes the model as an absolute path, and shows the human which languages are configured.
5. The human can edit `extensions`, `include_paths`, `skip_dirs`, and the model path before first indexing.
6. The agent runs an index check.
7. The agent asks whether this specific agent should register MCP. If yes, it uses its own MCP mechanism.
8. Restart or reload the agent MCP session and check `/mcp`.

The MCP command shape is:

```text
<package-root>\skills\codedb-mcp\assets\codebase-mcp.exe --config <repo-root>\.codedb-mcp\codedb-mcp.toml mcp <repo-root>
```

This project intentionally keeps installation explicit: setup prepares local project files, while the agent/user chooses when and where to register MCP.

## What It Does

- Exposes local MCP tools for code search, outlines, symbols, typed callers, dependencies, file discovery, graph analysis, DeepWiki module planning, module atlas export, batching, and exports.
- Indexes configured source languages through one explicit config file: `<repo-root>/.codedb-mcp/codedb-mcp.toml`.
- Stores generated data inside the target repo under `.codedb-mcp`. Delete that directory to remove local cache and generated wiki/index data.
- Uses a unified tree-sitter parser layer, not Roslyn/JDT. C#, Java, Rust, Python, JavaScript, TypeScript/TSX, C, and C++ all emit the same `FileEntry`/`Symbol` model. C#/Java typed callers and dependencies remain the strongest path because their namespace/package import rules are implemented on top of that shared AST output.
- Uses Minish ecosystem pieces: `model2vec-rs` with explicit-path `minishlab/potion-code-16M`, file-level semantic units, BM25 lexical ranking, exact identifier indexes, and Vicinity HNSW vectors.
- Builds a graphify-style code graph, computes Louvain communities lazily for `codedb_communities`, and exposes Rust-native `codedb_module_map`/`codedb_module_atlas` outputs from a dependency-connected file graph with label propagation, dependency cohesion, cross-folder evidence, semantic-neighbor probes, key symbols, and c-TF-IDF-like labels.
- Watches configured source extensions in MCP mode and rebuilds after a debounce.

## Technology Architecture

1. **Explicit project-local config**: all behavior comes from `.codedb-mcp/codedb-mcp.toml`. There are no environment-variable switches for indexing behavior.
2. **Project-local storage**: cache payloads, manifests, Louvain caches, and DeepWiki output live under `.codedb-mcp`. Deleting that directory removes all generated data for the repo.
3. **Scanner**: walks the repo with explicit extensions, max file size, gitignore behavior, skip dirs, and include paths. Unity `Library/PackageCache` can be included while the rest of `Library` is skipped.
4. **Unified language layer**: extension dispatch selects a tree-sitter grammar for C#, Java, Rust, Python, JavaScript, TypeScript/TSX, C, or C++. The parser emits the same `FileEntry`/`Symbol` model for every language and visits declarations without descending into large method bodies.
5. **Code-aware references**: C#/Java namespace/package imports, qualified names, aliases, static using, annotations, and attribute suffixes feed typed callers and dependency edges. Rust and the other non C#/Java languages currently provide indexed search, outlines, imports/includes/use declarations, and graph nodes, but not Roslyn/JDT-level semantic binding.
6. **Search indexes**: builds chunks, exact identifier hits, symbol-definition chunk hits, dependency references, BM25 lexical search, Model2Vec file embeddings, and a Vicinity HNSW vector index.
7. **Graph layer**: builds a graphify-style graph with file, namespace/package, symbol, dependency, and reference edges. Louvain communities and subcommunities are computed lazily on first request and cached under `.codedb-mcp`.
8. **Module atlas layer**: `codedb_module_map` and `codedb_module_atlas` run in Rust. They first split files by dependency-connected components, then do dependency-weighted label propagation inside each component. Path and token terms are used for naming, evidence, and oversized-component splitting, not as the primary clustering basis. `codedb_module_atlas` exports Embedding Atlas-ready JSON.
9. **MCP runtime**: implemented with the Rust `rmcp` SDK over stdio. Tools operate against a warm in-process index, and batch-capable tools plus `codedb_bundle` reduce MCP round trips.
10. **Setup guide and skills package**: `setup-for-agent.md` owns installation guidance. `skills/codedb-mcp` is standalone for tool usage and includes the executable, config template, MCP reference, and tool guidance. `skills/deepwiki` builds local DeepWiki-style docs from MCP evidence plus the active agent's reasoning. `skills/code-module-atlas` calls `codedb_module_atlas` and packages the local meet-blog-style module/file graph webpage.

## Configuration

Default config path:

```text
<repo-root>/.codedb-mcp/codedb-mcp.toml
```

The repo includes a working example at `.codedb-mcp/codedb-mcp.toml` and a distributable template at `skills/codedb-mcp/assets/codedb-mcp.toml.template`.

Important defaults:

```toml
[scan]
extensions = ["cs", "java", "rs", "py", "pyw", "js", "jsx", "mjs", "cjs", "ts", "tsx", "c", "h", "cc", "cpp", "cxx", "hpp", "hh", "hxx"]
max_file_bytes = 50000000
respect_gitignore = true
include_paths = ["Library/PackageCache"]

[embedding]
model = "C:/Users/<user>/.cache/huggingface/hub/codedb-mcp/models/potion-code-16M"

[storage]
enabled = true
dir = ".codedb-mcp"
```

There are no environment-variable toggles. Edit the config file explicitly. The model path is explicit and absolute; on Windows the setup guide uses the default HuggingFace cache when present, otherwise it falls back to the second available drive.

## Build And CLI

Build:

```powershell
cargo build --release
```

Run MCP directly:

```powershell
target\release\codebase-mcp.exe --config u3dclient\.codedb-mcp\codedb-mcp.toml mcp u3dclient
```

Quick CLI checks:

```powershell
target\release\codebase-mcp.exe --config u3dclient\.codedb-mcp\codedb-mcp.toml index u3dclient
target\release\codebase-mcp.exe --config u3dclient\.codedb-mcp\codedb-mcp.toml search "network listener manager" u3dclient -k 5
target\release\codebase-mcp.exe --config u3dclient\.codedb-mcp\codedb-mcp.toml --root u3dclient tool codedb_status "{}"
```

MCP mode answers the protocol handshake before the initial index finishes, then builds the default project index in the background. Early tool calls may wait for that first build. It also watches indexed extensions by default; when a configured source file changes, the server debounces events, rebuilds the project index in the background, and swaps in the new index after it is ready. Use `--no-watch` for static benchmark runs.

## Batch Examples

`codedb_search` accepts `queries`:

```json
{
  "max_results": 3,
  "queries": [
    "PoolManager",
    {
      "query": "Joystick",
      "path_glob": "Assets/Plugins/3rdPlugins/Joystick Pack/**"
    },
    {
      "query": "NetworkListenerManager",
      "regex": true,
      "compact": true
    }
  ]
}
```

`codedb_callers` accepts `targets`:

```json
{
  "max_results": 10,
  "targets": [
    {
      "name": "PoolManager",
      "definition_path": "Assets/Scripts/HotFix/3rdExtend/Runtime/PoolManager/PoolManager.cs",
      "definition_line": 26
    },
    {
      "name": "Joystick",
      "definition_path": "Assets/Plugins/3rdPlugins/Joystick Pack/Scripts/Runtime/Base/Joystick.cs",
      "definition_line": 8
    }
  ]
}
```

`codedb_communities` uses lazy Louvain clustering:

```powershell
target\release\codebase-mcp.exe --config u3dclient\.codedb-mcp\codedb-mcp.toml --root u3dclient tool codedb_communities "{`"community_limit`":10}"
target\release\codebase-mcp.exe --config u3dclient\.codedb-mcp\codedb-mcp.toml --root u3dclient tool codedb_communities "{`"community_id`":0,`"children`":true,`"community_limit`":20}"
```

Overview calls return community IDs, labels, member counts, and cohesion. Add `children=true` or `subcommunities=true` with a `community_id` to split only that community's subgraph; child clusters are cached in `.codedb-mcp/louvain-subcommunities.bin`.

`codedb_module_map` is the preferred DeepWiki planning call. It uses the Rust dependency-connected module graph, then adds dependency cohesion, cross-folder roots, semantic-neighbor probes, entry points, key symbols, and c-TF-IDF-like labels:

```powershell
target\release\codebase-mcp.exe --config u3dclient\.codedb-mcp\codedb-mcp.toml --root u3dclient tool codedb_module_map "{`"path_prefix`":`"Assets/Scripts`",`"limit`":40,`"min_files`":2,`"semantic_neighbors`":5}"
```

`codedb_module_atlas` exports module/file graph data. The packaged `code-module-atlas` skill calls this tool, converts the output to the bundled viewer dataset, and prepares the local webpage:

```powershell
node skills\code-module-atlas\scripts\build-module-atlas.mjs u3dclient
cd skills\code-module-atlas\assets\viewer
npm run dev -- --port 5174 --strictPort
```

## Skills

The `skills/` directory is intended to be copied as a standalone package.

- `setup-for-agent.md`: installation guide for agents. It reuses the default HuggingFace cache when present, falls back to the second Windows drive when absent, and writes project-local config with an absolute model path.
- `skills/codedb-mcp`: includes `assets/codebase-mcp.exe`, a config template, MCP registration reference, and tool guidance. It does not own setup.
- `skills/deepwiki`: creates DeepWiki-style local documentation using local `codedb_*` tools plus the active agent's reasoning. It emphasizes business module boundaries over folder-only or community-only grouping.
- `skills/code-module-atlas`: creates a local 3D module/file atlas webpage by calling `codedb_module_atlas`, then adapting the bundled meet-blog-style viewer. Generated repo-specific JSON stays ignored.
