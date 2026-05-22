# codebase-mcp

[中文 README](README.zh-CN.md)

Rust MCP server with a `codedb_*` compatible tool surface and a local Minish-style search core for tree-sitter indexed codebases.

## What It Does

- Exposes local MCP tools for code search, outlines, symbols, typed callers, dependencies, file discovery, graph analysis, batching, and exports.
- Indexes configured source languages through one explicit config file: `<repo-root>/.codedb-mcp/codedb-mcp.toml`.
- Stores generated data inside the target repo under `.codedb-mcp`. Delete that directory to remove local cache and generated wiki/index data.
- Uses a unified tree-sitter parser layer, not Roslyn/JDT. C#, Java, Python, JavaScript, TypeScript/TSX, C, and C++ all emit the same `FileEntry`/`Symbol` model. C#/Java typed callers and dependencies remain the strongest path because their namespace/package import rules are implemented on top of that shared AST output.
- Uses Minish ecosystem pieces: `model2vec-rs` with `minishlab/potion-code-16M`, file-level semantic units, BM25 lexical ranking, exact identifier indexes, and Vicinity HNSW vectors.
- Builds a graphify-style code graph and computes Louvain communities lazily for `codedb_communities`.
- Watches configured source extensions in MCP mode and rebuilds after a debounce.

## Technology Architecture

1. **Explicit project-local config**: all behavior comes from `.codedb-mcp/codedb-mcp.toml`. There are no environment-variable switches for indexing behavior.
2. **Project-local storage**: cache payloads, manifests, Louvain caches, and DeepWiki output live under `.codedb-mcp`. Deleting that directory removes all generated data for the repo.
3. **Scanner**: walks the repo with explicit extensions, max file size, gitignore behavior, skip dirs, and include paths. Unity `Library/PackageCache` can be included while the rest of `Library` is skipped.
4. **Unified language layer**: extension dispatch selects a tree-sitter grammar for C#, Java, Python, JavaScript, TypeScript/TSX, C, or C++. The parser emits the same `FileEntry`/`Symbol` model for every language and visits declarations without descending into large method bodies.
5. **Code-aware references**: C#/Java namespace/package imports, qualified names, aliases, static using, annotations, and attribute suffixes feed typed callers and dependency edges. Other languages currently provide indexed search, outlines, imports/includes, and graph nodes, but not Roslyn/JDT-level semantic binding.
6. **Search indexes**: builds chunks, exact identifier hits, symbol-definition chunk hits, dependency references, BM25 lexical search, Model2Vec file embeddings, and a Vicinity HNSW vector index.
7. **Graph layer**: builds a graphify-style graph with file, namespace/package, symbol, dependency, and reference edges. Louvain communities and subcommunities are computed lazily on first request and cached under `.codedb-mcp`.
8. **MCP runtime**: tools operate against a warm in-process index. Batch-capable tools and `codedb_bundle` reduce MCP round trips.
9. **Skills package**: `skills/codedb-mcp` is standalone and includes the executable, setup script, config template, install notes, and tool guidance. `skills/deepwiki` builds local DeepWiki-style docs from MCP evidence plus the active agent's reasoning.

## Configuration

Default config path:

```text
<repo-root>/.codedb-mcp/codedb-mcp.toml
```

The repo includes a working example at `.codedb-mcp/codedb-mcp.toml` and a distributable template at `skills/codedb-mcp/assets/codedb-mcp.toml.template`.

Important defaults:

```toml
[scan]
extensions = ["cs", "java", "py", "pyw", "js", "jsx", "mjs", "cjs", "ts", "tsx", "c", "h", "cc", "cpp", "cxx", "hpp", "hh", "hxx"]
max_file_bytes = 50000000
respect_gitignore = true
include_paths = ["Library/PackageCache"]

[embedding]
model = "minishlab/potion-code-16M"

[storage]
enabled = true
dir = ".codedb-mcp"
```

There are no environment-variable toggles. Edit the config file explicitly.

## Benchmarks: MCP vs rg on u3dclient

Benchmark target: `F:\workspace\main\Unicorn\u3dclient`.

Current index status with the existing u3dclient C#/Java config:

- Indexed files: 18,975.
- Chunks: 129,165.
- Symbols: 275,878.
- Graph: 295,753 nodes and 688,566 edges.
- Vector index: Vicinity HNSW over Model2Vec `minishlab/potion-code-16M`.
- Storage: `F:\workspace\main\Unicorn\u3dclient\.codedb-mcp`.

Latest index timings on this machine:

| Scenario | Cache | Internal total | Notes |
|---|---|---:|---|
| Cold tree-sitter `.codedb-mcp` build | miss | 44.876s wall | Includes scan, tree-sitter declaration parse, embeddings, graph, BM25, HNSW, cache save |
| Reopen with unchanged files/config | hit | 39.546s wall | Reuses parsed files, chunks, semantic units, embeddings; rebuilds runtime graph/BM25/HNSW |
| One-shot `codedb_status` CLI | hit | 39.4s wall | Includes process startup and index load; persistent MCP is the intended mode |

Java smoke benchmark on `F:\workspace\main\Unicorn\gameserver` with 6,940 Java files:

| Scenario | Files | Chunks | Symbols | Time |
|---|---:|---:|---:|---:|
| Cold tree-sitter build | 6,940 | 55,057 | 245,238 | 16.919s |
| Reopen with unchanged files/config | 6,940 | 55,057 | 245,238 | 11.527s |

Multi-language smoke benchmark with C#, Java, Python, TypeScript, and C++ files: 5 files, 5 chunks, 12 symbols, 1.147s.

Warm persistent MCP tool timings below do not include server startup or index load.

### Latest Warm Tool Validation

These calls were measured through one already-started MCP process on `u3dclient`; `codedb_search regex=true` was checked against `rg --no-ignore` on the same scoped corpus.

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

### Shared Exact Search Features

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

### MCP-Only Features

`rg` remains the right tool for ad hoc raw filesystem grep across arbitrary file types. The MCP layer adds indexed code-aware features that `rg` does not model:

| Scenario | MCP tool | Results | Time | rg equivalent |
|---|---|---:|---:|---|
| Hybrid search for `PoolManager` related chunks | `codedb_search` | 20 | 0.0198s | none |
| Hybrid search for `Joystick` related chunks | `codedb_search` | 20 | 0.0666s | none |
| Hybrid search for `NetworkListenerManager` related chunks | `codedb_search` | 20 | 0.0271s | none |
| Business semantic search: `alliance member ranking donation gift` under `Assets/Scripts` | `codedb_search path_glob=...` | 20 | 0.0358s | none |
| Definition-anchored references for `PoolManager` at `PoolManager.cs:26` | `codedb_callers` | 7 | 0.0045s | none |
| Definition-anchored references for `Joystick` at `Joystick.cs:8` | `codedb_callers` | 7 | 0.0069s | none |

### Dependency, Query, Outline, Analyze

`codedb_deps` reads the precomputed dependency graph:

| Scenario | Result | Time |
|---|---:|---:|
| `GameObjectPoolMgr.cs depends_on` | 7 files | 0.0002s |
| `NetworkListenerManager.cs imported_by` | 3 files | 0.0002s |
| `AndroidPlatform.cs depends_on` | 3 files | 0.0002s |

`codedb_find` and `codedb_query`:

| Scenario | Tool | Result | Time |
|---|---|---:|---:|
| Exact file-path lookup `NetworkListenerManager.cs` | `codedb_find` | 1 path, top1 correct | 0.0203s |
| Exact file-path lookup `GameObjectPoolMgr` | `codedb_find` | 1 path | 0.0197s |
| Human phrase `Joystick Pack Base Joystick` | `codedb_find` | 1 path, top1 correct | 0.0177s |
| Typo-ish query `ResTypDef` | `codedb_find` | 5 ranked paths, expected target rank 3 | 0.0191s |
| `find NetworkListenerManager.cs -> outline` | `codedb_query` | 1 outline | 0.0202s |
| `filter Joystick Pack -> limit 3 -> outline` | `codedb_query` | 3 outlines | 0.0080s |
| `filter UnityNativeTools -> search NetworkListenerManager` | `codedb_query` | 16 chunks | 0.0097s |
| `find GameObjectPoolMgr -> search PoolManager` | `codedb_query` | 17 chunks | 0.0220s |
| 105 mixed `codedb_query` calls | `codedb_query` | avg/p95 | 0.0186s / 0.0297s |

`codedb_outline` reads precomputed symbols and does not reparse files at request time:

| File | Size | Lines | Symbols | Time |
|---|---:|---:|---:|---:|
| `NetworkListenerManager.cs` | 609 B | 28 | 1 | 0.3ms |
| `Joystick.cs` | 5.2 KB | 162 | 17 | 0.3ms |
| `PoolManager.cs` | 20 KB | 534 | 32 | 0.2ms |
| `GameObjectPoolMgr.cs` | 21 KB | 482 | 21 | 0.2ms |
| `NEON_AArch64.cs` | 1.33 MB | 12,398 | 2,211 | 1.4ms |

Batching 100 `codedb_outline compact=true` calls in one MCP process produced avg 0.191ms, p50 0.2ms, p95 0.3ms, max 0.4ms.

`codedb_analyze` on `u3dclient` costs about 0.93s per call; 100 mixed calls averaged 0.9446s with p95 1.0073s.

### Bundle Stress Test

`codedb_bundle` runs up to 100 inner operations in one MCP request. Requests above 100 execute the first 100 and include a truncation notice.

| Scenario | Inner ops requested | Repeats | Inner ops executed | Time |
|---|---:|---:|---:|---:|
| Fast mixed metadata/deps/outline/read bundle | 100 | 1 | 100 | 0.0895s |
| Overflow bundle | 120 | 1 | 100 + truncation notice | 0.0924s |
| Repeated fast bundle | 100 | 50 | 5,000 total | avg 0.0913s, p95 0.1084s |
| Mixed search/callers/deps/outline bundle | 100 | 1 | 100 | 2.3174s |
| Heavy regex search bundle | 100 | 1 | 100 | 26.0085s |

## Usage

Build:

```powershell
cargo build --release
```

Create or edit the target repo config:

```powershell
New-Item -ItemType Directory -Force F:\workspace\main\Unicorn\u3dclient\.codedb-mcp
Copy-Item .codedb-mcp\codedb-mcp.toml F:\workspace\main\Unicorn\u3dclient\.codedb-mcp\codedb-mcp.toml
```

Run MCP:

```powershell
target\release\codebase-mcp.exe --config F:\workspace\main\Unicorn\u3dclient\.codedb-mcp\codedb-mcp.toml mcp F:\workspace\main\Unicorn\u3dclient
```

Quick CLI checks:

```powershell
target\release\codebase-mcp.exe --config F:\workspace\main\Unicorn\u3dclient\.codedb-mcp\codedb-mcp.toml index F:\workspace\main\Unicorn\u3dclient
target\release\codebase-mcp.exe --config F:\workspace\main\Unicorn\u3dclient\.codedb-mcp\codedb-mcp.toml search "network listener manager" F:\workspace\main\Unicorn\u3dclient -k 5
target\release\codebase-mcp.exe --config F:\workspace\main\Unicorn\u3dclient\.codedb-mcp\codedb-mcp.toml --root F:\workspace\main\Unicorn\u3dclient tool codedb_status "{}"
```

MCP mode watches indexed extensions by default. When a configured source file changes, the server debounces events, rebuilds the project index in the background, and swaps in the new index after it is ready. Use `--no-watch` for static benchmark runs.

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
target\release\codebase-mcp.exe --config F:\workspace\main\Unicorn\u3dclient\.codedb-mcp\codedb-mcp.toml --root F:\workspace\main\Unicorn\u3dclient tool codedb_communities "{`"community_limit`":10}"
target\release\codebase-mcp.exe --config F:\workspace\main\Unicorn\u3dclient\.codedb-mcp\codedb-mcp.toml --root F:\workspace\main\Unicorn\u3dclient tool codedb_communities "{`"community_id`":0,`"children`":true,`"community_limit`":20}"
```

Overview calls return community IDs, labels, member counts, and cohesion. Add `children=true` or `subcommunities=true` with a `community_id` to split only that community's subgraph; child clusters are cached in `.codedb-mcp/louvain-subcommunities.bin`.

## Skills

The `skills/` directory is intended to be copied as a standalone package.

- `skills/codedb-mcp`: includes `assets/codebase-mcp.exe`, a setup script, config template, MCP install reference, and tool guidance.
- `skills/deepwiki`: creates DeepWiki-style local documentation using local `codedb_*` tools plus the active agent's reasoning. It emphasizes business module boundaries over folder-only or community-only grouping.

Recommended setup from a copied skill:

```powershell
powershell -ExecutionPolicy Bypass -File <skill-root>\scripts\setup.ps1 -ProjectRoot <repo-root>
```

Then register MCP manually in the agent's MCP config using the command printed by the script.
