<div align="center">

<h1>codebase-mcp</h1>

<p><strong>本地优先的 MCP 代码智能工具集：快速代码搜索、依赖感知模块发现、可视化代码 atlas 网页，以及 DeepWiki-style 仓库文档生成。</strong></p>

<p>
  <img alt="Rust MCP" src="https://img.shields.io/badge/Rust-MCP-000000?logo=rust">
  <img alt="tree-sitter indexing" src="https://img.shields.io/badge/tree--sitter-indexing-2f6f9f">
  <img alt="local first" src="https://img.shields.io/badge/local--first-.codedb--mcp-3b82f6">
  <img alt="Minish Model2Vec" src="https://img.shields.io/badge/Minish-Model2Vec-7c3aed">
</p>

<p>简体中文 | <a href="README.md">English</a></p>

<p>
  <a href="#项目介绍">项目介绍</a> •
  <a href="#mcp-工具">MCP 工具</a> •
  <a href="#code-module-atlas">Code Module Atlas</a> •
  <a href="#deepwiki">DeepWiki</a> •
  <a href="#benchmark-速览">Benchmark</a> •
  <a href="#推荐-setup-流程">Setup</a> •
  <a href="#skills">Skills</a>
</p>

</div>

## 项目介绍

`codebase-mcp` 会把一个本地仓库变成常驻 MCP 代码智能服务。它把 tree-sitter 索引源码、符号、引用、依赖、图元数据、词法索引和向量搜索数据都放在目标仓库的 `.codedb-mcp` 目录下。

常驻 MCP 进程内的 warm tool 调用目标是毫秒级响应。实测数据见 [Benchmark 速览](#benchmark-速览) 和 [MCP 工具 Benchmark 矩阵](#mcp-工具-benchmark-矩阵)，里面包含耗时、峰值内存和 `rg` 对比。

## 功能概览

| 领域 | 能力 |
|---|---|
| 快速 MCP 工具 | 索引化 exact/regex 搜索、BM25/符号搜索、懒加载向量搜索、outline、definition、callers、deps、模糊文件查找、query pipeline 和 100-call bundle。 |
| 模块发现 | 先按依赖连通文件组件划分，再做 dependency-weighted label propagation；路径和术语用于可解释标签和证据。 |
| Code Module Atlas | 打包 meet-blog 风格 3D viewer：一个源码文件一个星点，支持模块/文件列表、依赖边、文件聚焦和详情。 |
| DeepWiki | 基于 MCP 证据和当前 agent 推理生成本地仓库文档，强调业务模块优先、代码引用和源码证据。 |
| 本地部署 | 显式 `.codedb-mcp/codedb-mcp.toml`、项目本地存储、可复制 skills，不依赖隐藏环境变量行为。 |

## MCP 工具

服务会把 tree-sitter 索引和项目本地数据放在 `.codedb-mcp` 下，并提供这些 MCP 能力：

- 快速 exact/regex 搜索、BM25/符号搜索，以及懒加载向量搜索；
- 符号大纲和定义查找；
- 基于 definition path/line 锚定的 LSP-like callers；
- 文件正向依赖、反向依赖和 transitive 依赖查询；
- 模糊文件查找、路径 glob、小型 query pipeline 和一次最多 100 个内部调用的 bundle；
- 图摘要、懒计算 Louvain community、模块规划、atlas 导出和 DeepWiki 证据收集。

## Code Module Atlas

![Code Module Atlas 演示](docs/assets/code-module-atlas.gif)

[观看 MP4 演示](docs/assets/code-module-atlas.mp4)

Atlas 网页由 `skills/code-module-atlas` skill 生成。它调用本地 MCP 的模块 atlas 导出，把结果转换成内置 meet-blog 风格 3D viewer 的数据集，并用一个星点表示一个源码文件。

模块边界先从依赖连通文件图开始划分。在每个连通块内部，Rust 模块规划器使用 dependency-weighted label propagation；路径和 distinctive terms 只用于命名、证据展示和过大连通块拆分，不作为主要分组规则。网页提供模块列表、选中模块内的文件列表、文件之间的依赖边，以及文件聚焦和详情展示。

```powershell
node skills\code-module-atlas\scripts\build-module-atlas.mjs u3dclient
cd skills\code-module-atlas\assets\viewer
npm run dev -- --port 5174 --strictPort
```

## DeepWiki

`skills/deepwiki` skill 会基于 MCP 证据和当前 agent 的推理能力生成本地 DeepWiki-style 文档。它从依赖感知的模块候选开始规划页面，然后生成业务模块优先的文档，包含代码引用、入口点、流程、依赖关系和风险说明，不需要单独配置大模型 API。

推荐的部署方式是 setup-guide first：先把 `setup-for-agent.md` 交给 agent，让它创建 `.codedb-mcp`、优先复用已有的默认 HuggingFace cache；如果默认 cache 不存在，再走第二盘符逻辑。之后询问人类是否要给某个特定 agent 注册 MCP。`codedb-mcp` skill 只负责安装后的工具使用，不负责安装。

## Benchmark 速览

测试目标：`u3dclient`。

Benchmark 于 2026-05-28 在 Windows 上重跑。单次 CLI 行包含进程启动和 cache load；warm 行通过一个已加载进程内的 `codedb_bundle timing=true` 测量。内存口径是子进程 sampled peak Working Set / Private Bytes。

当前 Unity C# benchmark 配置索引状态：

- 19,035 indexed files
- 31,949 chunks
- 277,213 symbols
- 19,941 graph nodes
- 166,132 graph edges
- Model2Vec `minishlab/potion-code-16M` 文件向量在首次自然语言搜索时懒生成
- lazy flat cosine file vectors
- 存储目录：`u3dclient\.codedb-mcp`
- cache v20 sidecar：紧凑 `index.bin`、spill-to-disk `bm25.postings`、懒加载 `word_index.bin`/`word_hits.bin`、懒加载 `callers.bin`、懒加载 `deps.bin`、可选旧版 `embeddings.bin`，以及二进制源码 fingerprint。
- 下方峰值内存为子进程 sampled Working Set / Private Bytes。冷重建行是在开启内存采样时测得，因此 wall time 不应直接和未采样的更快冷重建结果对比。

索引耗时：

| 场景 | Cache | 耗时 | 峰值内存 | 说明 |
|---|---|---:|---:|---|
| cache v20 项目本地冷重建 | miss | 26.335s internal / 26.621s wall | 256.4 MB WS / 250.2 MB private | 内存采样冷重建；tree-sitter 声明解析、按需源码依赖、spill-to-disk BM25、懒 embedding、compact sidecar cache save |
| cache-hit index open | hit | 0.873s internal / 1.132s wall | 134.9 MB WS / 136.0 MB private | 包含进程启动、源码 fingerprint 校验和 cache load |
| 单次 CLI `codedb_status` | hit | 0.252s | 14.1 MB WS / 7.9 MB private | manifest/fingerprint fast path，不反序列化完整 index |
| 单次 `codedb_find PoolManager` | hit | 0.283s | 14.4 MB WS / 8.2 MB private | manifest/fingerprint fast path，只扫缓存文件列表 |
| 单次符号 `codedb_search PoolManager` | hit | 0.739s | 151.5 MB WS / 154.8 MB private | 符号形态 query 走 BM25 + 精确符号增强，不加载 embeddings |
| 单次 `codedb_callers PoolManager` | hit | 0.243s | 14.2 MB WS / 7.8 MB private | `callers.bin` sidecar 命中；未缓存 target 第一次会走完整 caller 路径并写入 sidecar |
| 单次 `codedb_deps PoolManager.cs` | hit | 0.303s | 34.8 MB WS / 28.3 MB private | `deps.bin` fast path，不反序列化完整 index |
| 单次业务短语 `codedb_search` | hit | 0.777s | 152.1 MB WS / 154.7 MB private | BM25 候选足够时直接返回 lexical 结果，不加载 Model2Vec |
| 单次包含 20 个符号搜索的 `codedb_bundle` | hit | 0.895s | 154.3 MB WS / 156.9 MB private | 一次进程加载 + 20 个内部符号搜索 |
| 单次 `codedb_module_atlas` 导出 | hit | 12.355s | 471.3 MB WS / 527.9 MB private | 包含 cache-hit index load 和 atlas JSON export |
| warm module atlas 生成 | ready | 7.223s internal | 已加载状态 | 依赖连通文件图生成 1,373 个模块、16,365 个文件点 |

Java 工程 `gameserver`：

| 场景 | Files | Chunks | Symbols | 耗时 | 峰值内存 |
|---|---:|---:|---:|---:|---:|
| 配置/model path 变更后的 cold build | 6,940 | 55,057 | 245,238 | 10.477s | 656.0 MB WS / 664.4 MB private |
| 文件和配置不变的 cache hit | 6,940 | 55,057 | 245,238 | 1.027s | 129.4 MB WS / 176.4 MB private |

多语言 smoke benchmark：C#、Java、Rust、Python、Lua、TypeScript、C、C++ 共 8 个文件，8 chunks，14 symbols，0.219s。
Rust smoke check：当前仓库 29 个索引文件，1,752 chunks，1,901 symbols；`codedb_outline`、`codedb_search`、`codedb_deps` 都能返回 Rust 结果。

下面的 warm MCP 工具耗时不包含 MCP 进程启动和 index load，除非场景明确写了 one-shot 或 cold。`rg` 基准使用 `--no-ignore`，因为这个 Unity 项目刻意包含 `Library/PackageCache`。`rg` 仍然适合临时扫任意文件；MCP 工具适合在配置好的源码语料上做反复、低延迟、代码感知的查询。

峰值内存列里的 `warm baseline` 指上方 cache-hit 进程基线：134.9 MB Working Set / 136.0 MB Private Bytes。标注“额外未单独采样”的行有耗时数据，但还没有逐工具单独峰值内存采样。

## MCP 工具 Benchmark 矩阵

| Tool | 用途 | Benchmark 场景 | MCP 耗时 | 峰值内存 | rg 等价能力 | rg 耗时 | MCP vs rg |
|---|---|---|---:|---:|---|---:|---:|
| `codedb_index` | 构建/重建本地索引 | cold `u3dclient` v20 rebuild | 26.335s internal / 26.621s wall | 256.4 MB WS / 250.2 MB private | 无 | n/a | n/a |
| `codedb_status` | 索引健康、数量、扫描状态、模型路径 | one-shot cache-hit CLI | 0.252s wall | 14.1 MB WS / 7.9 MB private | 无 | n/a | n/a |
| `codedb_tree` | 整个索引树，含语言、行数、符号数 | warm tree summary | 11.891ms | warm baseline；额外未单独采样 | 部分支持 `rg --files`，但没有符号/行数元数据 | n/a | n/a |
| `codedb_outline` | 单文件预计算 tree-sitter 符号大纲 | `PoolManager.cs`，36 symbols | 0.074ms；100-call p95 0.118ms | warm baseline；额外未单独采样 | 无 | n/a | n/a |
| `codedb_symbol` | 按符号名找定义 | `PoolManager` definitions | 2.106ms | warm baseline；额外未单独采样 | 只能部分 regex 模拟，没有 parser-defined symbol model | n/a | n/a |
| `codedb_search` | 混合代码搜索、regex、batch 查询 | scoped regex `Joystick`；符号/NL 查询 | 7.342ms scoped regex；3.9-49.1ms 符号/NL；2.48-2.63s broad regex | 151.5-152.1 MB one-shot lexical search | scoped/broad 原始 grep | 0.133s scoped；1.46-1.67s broad | scoped MCP 快 18.1x；broad MCP 慢 1.5-1.8x |
| `codedb_word` | 精确 identifier 倒排索引查询 | `PoolManager` identifier hits | 94.403ms 首次 lazy word-index load | warm baseline；word sidecar 额外未单独采样 | 可部分用 `rg` word grep，但没有 indexed identifier sidecar | n/a | n/a |
| `codedb_callers` | 定义锚定引用，带 C#/Java type filtering；支持 batch targets | `PoolManager.cs:26` refs | 3.422ms avg / 3.619ms p95 | 14.2 MB WS / 7.8 MB private one-shot sidecar hit | 无语义锚定能力 | n/a | n/a |
| `codedb_hot` | 最近修改的索引文件 | top 5 hot files | 7.069ms | warm baseline；额外未单独采样 | 无 | n/a | n/a |
| `codedb_deps` | 文件依赖、反向依赖、传递依赖 | `GameObjectPoolMgr.cs depends_on` | 0.098ms avg / 0.117ms p95；首次 reverse-deps load 132.938ms | 34.8 MB WS / 28.3 MB private one-shot | 无 | n/a | n/a |
| `codedb_read` | 读取索引文件或行范围，支持 hash | `PoolManager.cs` lines 1-40 | 0.757ms | warm baseline；额外未单独采样 | 部分支持打印文件，但没有 indexed hash/path contract | n/a | n/a |
| `codedb_edit` | 只读兼容 stub | edit attempt 返回错误 | immediate stub | 未采样 | 无 | n/a | n/a |
| `codedb_changes` | 查询某个 sequence 之后变化的文件 | `since=0` change listing | 10.818ms | warm baseline；额外未单独采样 | 无 | n/a | n/a |
| `codedb_snapshot` | files/symbols/dependency graph 的 JSON 快照 | full snapshot，丢弃输出 | 1.303s | warm baseline；snapshot 输出额外未采样 | 无 | n/a | n/a |
| `codedb_bundle` | 一次 MCP 请求内最多执行 100 个 `codedb_*` 调用 | 100 个 fast metadata/deps/outline/read 混合操作 | 57.725ms inner sum；重复 avg 61.005ms | 154.3 MB WS / 156.9 MB private for one-shot 20-search bundle | 无 MCP batching | n/a | n/a |
| `codedb_remote` | remote query 兼容 stub | remote attempt 返回 stub response | immediate stub | 未采样 | 无 | n/a | n/a |
| `codedb_projects` | 列出当前 server process 里的项目 | warm project list | 0.059ms | warm baseline；额外未单独采样 | 无 | n/a | n/a |
| `codedb_find` | 模糊文件名/路径查找 | `NetworkListenerManager`、`Joystick`、typo `ResTypDef` | 18.019-20.230ms avg | 14.4 MB WS / 8.2 MB private one-shot | 无 fuzzy ranking | n/a | n/a |
| `codedb_query` | find/search/filter/limit/outline 小型 pipeline | 4 个已测 pipeline | 6.786-25.139ms avg | warm baseline；额外未单独采样 | 无等价单工具 | n/a | n/a |
| `codedb_glob` | 对索引路径做 glob 匹配 | Alliance UI `.cs` glob，52 hits | 4.231ms avg / 4.254ms p95 | warm baseline；额外未单独采样 | `rg --files --no-ignore -g` | 0.045s avg / 0.051s p95 | MCP 快 10.6x |
| `codedb_ls` | 列出索引目录的直接子项 | root directory listing | 4.027ms | warm baseline；额外未单独采样 | 部分支持 `rg --files`，但没有目录对象视图 | n/a | n/a |
| `codedb_graph` | graphify 风格图摘要或有限导出 | 首次 lazy graph summary | 943.431ms | warm baseline；graph 额外未单独采样 | 无 | n/a | n/a |
| `codedb_explain` | 解释 graph 节点匹配和出入边 | 首次 graph-backed `PoolManager` explain | 845.369ms | warm baseline；graph 额外未单独采样 | 无 | n/a | n/a |
| `codedb_path` | 文件/符号/节点之间的最短图路径 | graph load 后 `PoolManager` 到 `GameObjectPoolMgr` | 13.073ms | warm baseline；额外未单独采样 | 无 | n/a | n/a |
| `codedb_communities` | lazy Louvain communities/subcommunities | graph load 后 top 10 communities | 265.593ms | warm baseline；Louvain 额外未单独采样 | 无 | n/a | n/a |
| `codedb_module_map` | DeepWiki 模块规划，基于依赖连通文件图 | `Assets/Scripts`，20 modules，不含文件列表 | 1.679s | warm baseline；module graph 额外未单独采样 | 无 | n/a | n/a |
| `codedb_module_atlas` | 导出 module/file atlas JSON 供 viewer skill 使用 | one-shot atlas export | 12.355s wall | 471.3 MB WS / 527.9 MB private | 无 | n/a | n/a |
| `codedb_analyze` | 图统计、top nodes、关系计数、建议问题 | graph analysis | 首次 lazy graph build 830.637ms；warm 约 36.8ms | warm baseline；graph 额外未单独采样 | 无 | n/a | n/a |
| `codedb_export` | 导出 JSON、GraphML 或 Cypher | graph load 后 limited JSON export | 10.313ms | warm baseline；输出额外未单独采样 | 无 | n/a | n/a |

## 推荐 Setup 流程

1. 把 `setup-for-agent.md` 交给目标 agent。
2. agent 创建 `<repo-root>\.codedb-mcp` 和 `<repo-root>\.codedb-mcp\models`。
3. Windows 上先检查默认 HuggingFace hub cache。如果 `minishlab/potion-code-16M` 已经有有效 snapshot，配置就指向这个 snapshot。如果默认 hub 存在但模型不存在，就下载到 `C:\Users\<user>\.cache\huggingface\hub\codedb-mcp\models\potion-code-16M`。如果默认 hub 不存在，再按盘符排序选择第二个盘符，例如 `D:\codedb-mcp-cache\models\potion-code-16M`。
4. agent 写入 `<repo-root>\.codedb-mcp\codedb-mcp.toml` demo 配置，模型写绝对路径，并告诉人类当前会遍历哪些语言。
5. 人类可以在第一次索引前修改 `extensions`、`include_paths`、`skip_dirs` 和模型路径。
6. agent 跑一次 index 检查。
7. agent 询问人类是否要给当前特定 agent 配置 MCP；确认后才按该 agent 的方式配置。
8. 重启或 reload agent MCP session，然后检查 `/mcp`。

MCP 命令形态：

```text
<package-root>\skills\codedb-mcp\assets\codebase-mcp.exe --config <repo-root>\.codedb-mcp\codedb-mcp.toml mcp <repo-root>
```

这个项目刻意保持安装显式化：setup 只初始化项目本地文件，agent/user 决定何时、在哪里注册 MCP。

## 主要能力

- 通过 MCP 暴露 `codedb_search`、`codedb_callers`、`codedb_deps`、`codedb_outline`、`codedb_find`、`codedb_query`、`codedb_bundle`、`codedb_graph`、`codedb_communities`、`codedb_module_map`、`codedb_module_atlas` 等工具。
- 所有配置都来自目标项目内的 `.codedb-mcp/codedb-mcp.toml`，不依赖环境变量切换行为。
- 所有生成数据都放在目标项目的 `.codedb-mcp` 目录下；删除该目录即可清理本地索引、缓存、Louvain 缓存和 DeepWiki 输出。
- 使用统一 tree-sitter 解析层支持 C#、Java、Rust、Python、Lua、JavaScript、TypeScript/TSX、C、C++。
- C#/Java 的 typed callers 和 deps 额外实现 namespace/package import、qualified name、using alias、static using、annotation、attribute suffix 等规则，准确性最强。
- 使用 Minish 生态的 `model2vec-rs` 和显式路径配置的 `minishlab/potion-code-16M` 生成本地文件级向量。
- 使用 BM25、精确 identifier 倒排索引；符号形态 query 不加载向量，自然语言 query 才懒加载 Model2Vec 并做 flat cosine 向量检索。
- 构建 graphify 风格代码图，并对 `codedb_communities` 懒计算 Louvain community；`codedb_module_map` 和 `codedb_module_atlas` 是 Rust 原生模块视图，先按依赖连通文件图划分，再在连通块内做 dependency-weighted label propagation，并输出依赖内聚度、跨目录证据、语义近邻、入口点、关键符号和 c-TF-IDF-like 标签。
- MCP 模式下监听配置内的源码扩展，文件改动后 debounce 并后台重建索引。

## 技术架构

1. **显式配置层**：读取 `.codedb-mcp/codedb-mcp.toml`，配置扫描扩展、文件大小上限、gitignore 行为、include paths、skip dirs、embedding 模型、watch 和 storage。
2. **本地存储层**：索引 payload、manifest、Louvain cache、DeepWiki 证据和文档都写入 `.codedb-mcp`。数据跟随项目目录，不写全局数据库。
3. **扫描层**：基于配置遍历代码库，读取项目内 `.gitignore`，但目标 root 下的嵌套 Git worktree/submodule 会作为普通源码目录继续索引。Unity 项目中可以跳过大部分 `Library`，同时显式包含 `Library/PackageCache`。
4. **语言解析层**：所有语言统一走 tree-sitter grammar，输出同一套 `FileEntry` 和 `Symbol` 结构。当前支持 C#、Java、Rust、Python、Lua、JavaScript、TypeScript/TSX、C、C++，解析时只遍历声明层，避免大型方法体拖慢索引。
5. **代码语义增强层**：C#/Java 上继续做 namespace/package import、别名、静态 using、注解、属性后缀、限定名引用等轻量语义推断；Lua 会抽取 `require()` 并生成轻量文件依赖。
6. **搜索索引层**：cold index 阶段构建 chunk 元数据、symbol definition chunks、dependency references 和 spill-to-disk BM25。identifier word hits 与 Model2Vec file embeddings 改为 callers 或自然语言搜索首次需要时懒生成。
7. **内存友好缓存层**：cache v20 吸收 `justrach/codedb` 的 bounded content cache 思路：完整文件正文、chunk 预览正文、重复 chunk 文件路径、重复 language/kind 字符串、BM25 postings、word-index hits、caller 结果、embeddings、正向/反向依赖、graph 对象和 Louvain 结果都不再默认常驻。工具需要时再按需读取精确行、postings、word hits、caller sidecar、embedding、依赖或图数据。
8. **依赖与图层**：graphify 风格代码图懒构建。小仓库保留 file、namespace/package、symbol、dependency、reference 等节点和边；大仓库只有 graph/community/module 工具会触发 graph 构建，symbol 数据仍保留在 outline/search/callers 专用索引；Louvain community 懒加载并缓存。
9. **模块 atlas 层**：`codedb_module_map` 和 `codedb_module_atlas` 在 Rust 里运行。它们先按依赖图弱连通分量切开文件，再在每个连通块内部做依赖加权 label propagation。路径和 token 只用于命名、证据展示和过大连通块拆分，不作为主要聚类依据。`codedb_module_atlas` 导出 Embedding Atlas 可视化数据。
10. **MCP 工具层**：基于 Rust `rmcp` SDK 的 stdio server 实现；工具运行在 warm in-process index 上，支持 batch 和 bundle，减少 MCP 往返成本。
11. **Setup guide 和 Skill 打包层**：`setup-for-agent.md` 负责安装指导。`skills/codedb-mcp` 只负责工具使用，内含最新 `codebase-mcp.exe`、配置模板、MCP 注册参考和工具说明。`skills/code-module-atlas` 调用 `codedb_module_atlas`，并打包本地 meet-blog 风格的模块/文件图网页。

## 配置

默认配置路径：

```text
<repo-root>/.codedb-mcp/codedb-mcp.toml
```

关键配置：

```toml
[scan]
extensions = ["cs", "java", "rs", "py", "pyw", "lua", "js", "jsx", "mjs", "cjs", "ts", "tsx", "c", "h", "cc", "cpp", "cxx", "hpp", "hh", "hxx"]
max_file_bytes = 50000000
respect_gitignore = true
include_paths = ["Library/PackageCache"]

[embedding]
model = "C:/Users/<user>/.cache/huggingface/hub/codedb-mcp/models/potion-code-16M"

[storage]
enabled = true
dir = ".codedb-mcp"
```

`include_paths` 会覆盖被跳过的父目录，例如 Unity 项目中可以跳过 `Library`，但保留 `Library/PackageCache`。`respect_gitignore=true` 会读取项目内 `.gitignore`，但目标 root 下的嵌套 Git worktree/submodule 仍会被当作源码目录索引，除非被 `skip_dirs` 或扩展名规则排除。模型路径是显式绝对路径；Windows setup 会优先复用默认 HuggingFace cache，不存在时才走第二盘符。

## 构建与 CLI

```powershell
cargo build --release
```

直接启动 MCP：

```powershell
target\release\codebase-mcp.exe --config u3dclient\.codedb-mcp\codedb-mcp.toml mcp u3dclient
```

CLI 快速检查：

```powershell
target\release\codebase-mcp.exe --config u3dclient\.codedb-mcp\codedb-mcp.toml index u3dclient
target\release\codebase-mcp.exe --config u3dclient\.codedb-mcp\codedb-mcp.toml --root u3dclient tool codedb_status "{}"
```

MCP 模式会先完成协议握手，再在后台构建默认项目索引；如果索引还没完成，早期工具调用会等待首次构建结束。文件监听默认开启，源码变更后会 debounce 并后台重建索引。

## 工具简介

| Tool | 用途 |
|---|---|
| `codedb_search` | 混合搜索或 regex 行搜索；支持 `queries` batch |
| `codedb_callers` | LSP-like 引用查找；支持 definition path/line 锚定和 `targets` batch |
| `codedb_deps` | 文件依赖和反向依赖；支持 transitive |
| `codedb_outline` | 返回预计算符号大纲，不在请求时重新 parse |
| `codedb_symbol` | 按符号名找定义 |
| `codedb_word` | 精确 identifier 倒排索引查询 |
| `codedb_find` | 模糊文件名/路径查找 |
| `codedb_query` | find/search/filter/limit/outline 小型 pipeline |
| `codedb_bundle` | 一次 MCP 请求里执行最多 100 个工具调用 |
| `codedb_graph` | 图摘要或导出 |
| `codedb_communities` | 懒计算 Louvain community/subcommunity |
| `codedb_module_map` | DeepWiki 模块规划 atlas：依赖内聚度、跨目录证据、入口点、关键符号、语义近邻和 c-TF-IDF-like 标签 |
| `codedb_module_atlas` | 导出 module/file atlas JSON，供 `skills/code-module-atlas` 生成网页 |
| `codedb_analyze` | 图统计、top nodes、关系计数、建议问题 |
| `codedb_hot` | 最近修改的索引文件 |
| `codedb_status` | 索引健康状态和统计 |

## Skills

`skills/` 目录可以作为独立包复制。

- `setup-for-agent.md`：给 agent 用的安装指导，Windows 上优先复用默认 HuggingFace cache，不存在时才选择第二个盘符，并写入带绝对模型路径的项目本地配置。
- `skills/codedb-mcp`：包含 `assets/codebase-mcp.exe`、配置模板、MCP 注册参考和工具使用建议；不负责安装。
- `skills/deepwiki`：使用本地 `codedb_*` 工具和当前 agent 的推理能力生成 DeepWiki-style 文档，强调业务模块边界，而不是只按文件夹或 community 分组。
- `skills/code-module-atlas`：调用 `codedb_module_atlas` 生成本地 3D 模块/文件 atlas 网页；项目特定 JSON 是生成物，不提交。

## 发布说明

- 推荐发布时带上 `setup-for-agent.md` 和整个 `skills/` 目录；先由 setup guide 初始化项目，再按需安装/使用 skill。
- `.codedb-mcp/index.bin`、`.codedb-mcp/manifest.json`、`.codedb-mcp/*.bin` 是项目本地生成物，不建议提交。
- 旧 `.codebase-mcp` 名称已经迁移为 `.codedb-mcp`；配置文件名也统一为 `codedb-mcp.toml`。

## 致谢

- [meet-blog.buyixiao.xyz](https://meet-blog.buyixiao.xyz/) 启发了 Code Module Atlas 的视觉风格和 viewer 体验。
- [justrach/codedb](https://github.com/justrach/codedb) 启发了最初的 MCP 工具接口方向。
