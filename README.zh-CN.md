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

常驻 MCP 进程内的 warm tool 调用目标是毫秒级响应。实测数据见 [Benchmark 速览](#benchmark-速览)、[MCP vs rg](#mcp-vs-rg) 和 [Warm MCP 工具验证](#warm-mcp-工具验证)。

## 功能概览

| 领域 | 能力 |
|---|---|
| 快速 MCP 工具 | 索引化 exact/regex 搜索、词法和向量混合搜索、outline、definition、callers、deps、模糊文件查找、query pipeline 和 100-call bundle。 |
| 模块发现 | 先按依赖连通文件组件划分，再做 dependency-weighted label propagation；路径和术语用于可解释标签和证据。 |
| Code Module Atlas | 打包 meet-blog 风格 3D viewer：一个源码文件一个星点，支持模块/文件列表、依赖边、文件聚焦和详情。 |
| DeepWiki | 基于 MCP 证据和当前 agent 推理生成本地仓库文档，强调业务模块优先、代码引用和源码证据。 |
| 本地部署 | 显式 `.codedb-mcp/codedb-mcp.toml`、项目本地存储、可复制 skills，不依赖隐藏环境变量行为。 |

## MCP 工具

服务会把 tree-sitter 索引和项目本地数据放在 `.codedb-mcp` 下，并提供这些 MCP 能力：

- 快速 exact/regex 搜索，以及词法和向量混合搜索；
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

当前 Unity C# benchmark 配置索引状态：

- 19,030 indexed files
- 129,790 chunks
- 277,008 symbols
- 296,941 graph nodes
- 691,419 graph edges
- Model2Vec `minishlab/potion-code-16M`
- Vicinity HNSW file vectors
- 存储目录：`u3dclient\.codedb-mcp`

索引耗时：

| 场景 | Cache | 耗时 | 说明 |
|---|---|---:|---|
| tree-sitter cold build | miss | 66.061s | scan、tree-sitter 声明解析、embedding、graph、BM25、HNSW、cache save |
| 不变文件再次打开 | hit | 30.8s | 复用 parsed files、chunks、semantic units、embeddings；重建运行时 graph/BM25/HNSW |
| 单次 CLI `codedb_status` | hit | 31.0s | 包含进程启动和 index load；真实使用应保持 MCP 常驻 |
| 单次 Rust `codedb_module_atlas` 导出 | hit | 42.057s | 包含 cache-hit index load 和 atlas JSON export |
| warm Rust module atlas 生成 | ready | 9.746s internal | 依赖连通文件图生成 1,374 个模块、16,361 个文件点 |

Java 工程 `gameserver`：

| 场景 | Files | Chunks | Symbols | 耗时 |
|---|---:|---:|---:|---:|
| cold build | 6,940 | 55,057 | 245,238 | 16.919s |
| cache hit | 6,940 | 55,057 | 245,238 | 11.527s |

多语言 smoke benchmark：C#、Java、Python、TypeScript、C++ 共 5 个文件，5 chunks，12 symbols，1.147s。
Rust smoke check：当前仓库 20 个索引文件，其中 17 个 `.rs` 文件，341 chunks，604 symbols；`codedb_outline`、`codedb_search`、`codedb_deps` 都能返回 Rust 结果。

下面的 warm MCP 工具耗时都不包含 MCP 进程启动和 index load。

## MCP vs rg

对于精确文本和 regex 搜索，`codedb_search regex=true` 和 `rg` 都能回答。`rg` 基准使用 `--no-ignore`，因为这个 Unity 项目刻意包含 `Library/PackageCache`。

| 场景 | MCP tool | MCP hits | MCP 耗时 | rg baseline | rg hits | rg 耗时 |
|---|---|---:|---:|---|---:|---:|
| Exact `PoolManager` | `codedb_search regex=true` | 154 | 0.2234s | `rg --no-ignore -n -i -F` | 154 | 1.7201s |
| Exact `Joystick` | `codedb_search regex=true` | 938 | 0.2343s | `rg --no-ignore -n -i -F` | 938 | 1.9419s |
| Exact `NetworkListenerManager` | `codedb_search regex=true` | 14 | 0.1973s | `rg --no-ignore -n -i -F` | 14 | 1.7486s |
| Exact `GameObjectPoolMgr` | `codedb_search regex=true` | 8 | 0.2210s | `rg --no-ignore -n -i -F` | 8 | 2.1606s |
| Exact `AllianceManager` | `codedb_search regex=true` | 16 | 0.2190s | `rg --no-ignore -n -i -F` | 16 | 1.7719s |
| Joystick Pack 内 scoped `Joystick` | `codedb_search regex=true path_glob=...` | 46 | 0.0063s | scoped `rg --no-ignore -n -i -F` | 46 | 0.0415s |
| UnityNativeTools 内 scoped `NetworkListenerManager` | `codedb_search regex=true path_glob=...` | 14 | 0.0064s | scoped `rg --no-ignore -n -i -F` | 14 | 0.0414s |
| `Assets/Scripts` 内 Alliance UI/proto regex | `codedb_search regex=true path_glob=...` | 409 | 0.0635s | scoped `rg --no-ignore -n -i` | 409 | 0.4137s |
| Alliance UI `.cs` 文件 glob | `codedb_glob` | 52 | 0.0044s | `rg --files --no-ignore -g` | 52 | 0.5748s |

功能对比：

| 能力 | codedb-mcp | rg |
|---|---|---|
| 原始精确 grep | 支持，`codedb_search regex=true` 走索引 | 支持 |
| Regex 行搜索 | 支持，限定在配置源码语料 | 支持，直接扫文件系统 |
| 路径/文件范围过滤 | 支持，`path_glob`、`codedb_find`、`codedb_query` | 支持，`-g` 和路径参数 |
| 模糊文件查找 | 支持，`codedb_find` 排名 | 不直接支持 |
| 词法 + 向量混合搜索 | 支持，BM25 + Model2Vec + Vicinity | 不支持 |
| 符号大纲 | 支持，`codedb_outline` 读预计算 tree-sitter symbols | 不支持 |
| 定义锚定引用查找 | 支持，`codedb_callers` | 不支持语义锚定 |
| 文件依赖图 | 支持，`codedb_deps` | 不支持 |
| 代码图分析/导出 | 支持，`codedb_graph`、`codedb_analyze`、`codedb_export` | 不支持 |
| 一次 MCP 请求内批量调用 | 支持，batch 参数和 `codedb_bundle` | 不适用 |
| 任意未索引文件/二进制/普通文本 | 不适合，只索引配置里的源码扩展 | 适合 |

MCP-only 实测能力：

| 场景 | MCP tool | 结果 | 耗时 | rg 等价能力 |
|---|---|---:|---:|---|
| `PoolManager` 相关 chunk 混合搜索 | `codedb_search` | 20 | 0.0198s | 无 |
| `Joystick` 相关 chunk 混合搜索 | `codedb_search` | 20 | 0.0666s | 无 |
| `NetworkListenerManager` 相关 chunk 混合搜索 | `codedb_search` | 20 | 0.0271s | 无 |
| `Assets/Scripts` 下业务语义搜索：`alliance member ranking donation gift` | `codedb_search path_glob=...` | 20 | 0.0358s | 无 |
| `PoolManager.cs:26` 定义锚定引用 | `codedb_callers` | 7 | 0.0045s | 无 |
| `Joystick.cs:8` 定义锚定引用 | `codedb_callers` | 7 | 0.0069s | 无 |

结论：`rg` 仍然适合临时扫任意文件；`codedb-mcp` 适合在配置好的源码语料上做反复、低延迟、代码感知的查询。

## Warm MCP 工具验证

这些调用在一个已经启动的 MCP 进程里测量；精确 regex 搜索都用同范围的 `rg --no-ignore` 做了命中数校验。

| 场景 | Tool | 准确性 | avg | p95 |
|---|---|---:|---:|---:|
| scoped `PoolManager` exact | `codedb_search regex=true` | MCP 52 = rg 52 | 5.813ms | 5.953ms |
| scoped `Joystick` exact | `codedb_search regex=true` | MCP 46 = rg 46 | 6.371ms | 6.853ms |
| scoped `NetworkListenerManager` exact | `codedb_search regex=true` | MCP 14 = rg 14 | 6.486ms | 6.707ms |
| `PoolManager` hybrid | `codedb_search` | 预期文本存在 | 20.826ms | 21.723ms |
| `Joystick` hybrid | `codedb_search` | 预期文本存在 | 84.755ms | 84.621ms |
| `alliance member ranking donation gift` | `codedb_search` | Alliance 结果存在 | 39.849ms | 41.138ms |
| `PoolManager` refs | `codedb_callers` | 7 refs | 4.518ms | 5.464ms |
| `Joystick` refs | `codedb_callers` | 7 refs | 7.726ms | 8.692ms |
| `GameObjectPoolMgr.cs depends_on` | `codedb_deps` | 7 files | 0.244ms | 0.318ms |
| `NetworkListenerManager.cs imported_by` | `codedb_deps` | 3 files | 0.193ms | 0.212ms |
| `NetworkListenerManager.cs transitive imported_by` | `codedb_deps` | 16 files | 0.192ms | 0.230ms |
| `NetworkListenerManager.cs` path lookup | `codedb_find` | top1 correct | 20.259ms | 21.108ms |
| `Joystick Pack Base Joystick` path lookup | `codedb_find` | top1 correct | 17.710ms | 18.054ms |
| `ResTypDef` typo-ish lookup | `codedb_find` | 目标 rank 3 | 19.109ms | 20.027ms |
| `find NetworkListenerManager -> outline` | `codedb_query` | outline 存在 | 20.173ms | 20.505ms |
| `filter Joystick Pack -> limit 3 -> outline` | `codedb_query` | outline 存在 | 8.017ms | 9.206ms |
| `filter UnityNativeTools -> search NetworkListenerManager` | `codedb_query` | 结果存在 | 9.650ms | 10.755ms |
| `find GameObjectPoolMgr -> search PoolManager` | `codedb_query` | 结果存在 | 22.019ms | 23.469ms |

其他工具耗时：

| Tool / 场景 | 结果 | 耗时 |
|---|---:|---:|
| `codedb_deps` `GameObjectPoolMgr.cs depends_on` | 7 files | 0.0002s |
| `codedb_deps` `NetworkListenerManager.cs imported_by` | 3 files | 0.0002s |
| `codedb_deps` `AndroidPlatform.cs depends_on` | 3 files | 0.0002s |
| `codedb_outline` `NetworkListenerManager.cs` | 1 symbol | 0.3ms |
| `codedb_outline` `Joystick.cs` | 17 symbols | 0.3ms |
| `codedb_outline` `PoolManager.cs` | 32 symbols | 0.2ms |
| `codedb_outline` `NEON_AArch64.cs` | 2,211 symbols | 1.4ms |
| 100 次 `codedb_outline compact=true` | p95 | 0.3ms |
| `codedb_analyze` on `u3dclient` | graph analysis | 约 0.93s |

`codedb_bundle` 一次 MCP 请求最多执行 100 个内部操作；超过 100 个时只执行前 100 个并返回 truncation notice。

| 场景 | 请求内部操作数 | 重复次数 | 实际执行 | 耗时 |
|---|---:|---:|---:|---:|
| 快速 metadata/deps/outline/read 混合 bundle | 100 | 1 | 100 | 0.0895s |
| overflow bundle | 120 | 1 | 100 + truncation notice | 0.0924s |
| repeated fast bundle | 100 | 50 | 5,000 total | avg 0.0913s, p95 0.1084s |
| search/callers/deps/outline 混合 bundle | 100 | 1 | 100 | 2.3174s |
| heavy regex search bundle | 100 | 1 | 100 | 26.0085s |

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
- 使用统一 tree-sitter 解析层支持 C#、Java、Rust、Python、JavaScript、TypeScript/TSX、C、C++。
- C#/Java 的 typed callers 和 deps 额外实现 namespace/package import、qualified name、using alias、static using、annotation、attribute suffix 等规则，准确性最强。
- 使用 Minish 生态的 `model2vec-rs` 和显式路径配置的 `minishlab/potion-code-16M` 生成本地文件级向量。
- 使用 BM25、精确 identifier 倒排索引、Vicinity HNSW 向量索引做混合搜索。
- 构建 graphify 风格代码图，并对 `codedb_communities` 懒计算 Louvain community；`codedb_module_map` 和 `codedb_module_atlas` 是 Rust 原生模块视图，先按依赖连通文件图划分，再在连通块内做 dependency-weighted label propagation，并输出依赖内聚度、跨目录证据、语义近邻、入口点、关键符号和 c-TF-IDF-like 标签。
- MCP 模式下监听配置内的源码扩展，文件改动后 debounce 并后台重建索引。

## 技术架构

1. **显式配置层**：读取 `.codedb-mcp/codedb-mcp.toml`，配置扫描扩展、文件大小上限、gitignore 行为、include paths、skip dirs、embedding 模型、watch 和 storage。
2. **本地存储层**：索引 payload、manifest、Louvain cache、DeepWiki 证据和文档都写入 `.codedb-mcp`。数据跟随项目目录，不写全局数据库。
3. **扫描层**：基于配置遍历代码库。Unity 项目中可以跳过大部分 `Library`，同时显式包含 `Library/PackageCache`。
4. **语言解析层**：所有语言统一走 tree-sitter grammar，输出同一套 `FileEntry` 和 `Symbol` 结构。当前支持 C#、Java、Rust、Python、JavaScript、TypeScript/TSX、C、C++，解析时只遍历声明层，避免大型方法体拖慢索引。
5. **代码语义增强层**：C#/Java 上继续做 namespace/package import、别名、静态 using、注解、属性后缀、限定名引用等轻量语义推断。
6. **搜索索引层**：构建 chunks、identifier word hits、symbol definition chunks、BM25、Model2Vec embeddings、Vicinity HNSW。
7. **依赖与图层**：构建文件、namespace/package、symbol、dependency、reference 等节点和边；Louvain community 懒加载并缓存。
8. **模块 atlas 层**：`codedb_module_map` 和 `codedb_module_atlas` 在 Rust 里运行。它们先按依赖图弱连通分量切开文件，再在每个连通块内部做依赖加权 label propagation。路径和 token 只用于命名、证据展示和过大连通块拆分，不作为主要聚类依据。`codedb_module_atlas` 导出 Embedding Atlas 可视化数据。
9. **MCP 工具层**：基于 Rust `rmcp` SDK 的 stdio server 实现；工具运行在 warm in-process index 上，支持 batch 和 bundle，减少 MCP 往返成本。
10. **Setup guide 和 Skill 打包层**：`setup-for-agent.md` 负责安装指导。`skills/codedb-mcp` 只负责工具使用，内含最新 `codebase-mcp.exe`、配置模板、MCP 注册参考和工具说明。`skills/code-module-atlas` 调用 `codedb_module_atlas`，并打包本地 meet-blog 风格的模块/文件图网页。

## 配置

默认配置路径：

```text
<repo-root>/.codedb-mcp/codedb-mcp.toml
```

关键配置：

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

`include_paths` 会覆盖被跳过的父目录，例如 Unity 项目中可以跳过 `Library`，但保留 `Library/PackageCache`。模型路径是显式绝对路径；Windows setup 会优先复用默认 HuggingFace cache，不存在时才走第二盘符。

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
