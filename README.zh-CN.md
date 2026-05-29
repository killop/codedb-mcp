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

Benchmark 于 2026-05-29 在 Windows 上重跑。`warm` 耗时来自一个已加载的 MCP 进程；`one-shot` 耗时来自单独启动的 CLI 子进程，包含进程启动和 cache load。峰值内存口径是 MB Working Set / Private Bytes。

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

索引和 cache 基线：

| 场景 | 耗时 | 峰值内存 | 说明 |
|---|---:|---:|---|
| cache v20 冷重建 | 30.258s wall | 255.8 / 249.6 MB | tree-sitter 声明解析、按需源码依赖、spill-to-disk BM25、懒 embedding、compact cache save |
| cache-hit index open | 0.873s internal / 1.132s wall | 134.9 / 136.0 MB | 进程启动、源码 fingerprint 校验和 cache load |
| `codedb_index` cache-hit 工具调用 | 1.556s wall | 141.5 / 140.4 MB | cache 已有效时的显式工具调用 |

## MCP 工具 Benchmark 矩阵

这个表刻意压成 3 列，避免 GitHub README 页面出现横向滚动条。内存口径是 MB Working Set / Private Bytes。

| Tool / 用途 | MCP 实测 | rg 对比 |
|---|---|---|
| `codedb_index`<br>构建/重建本地索引 | cold 30.258s，255.8 / 249.6 MB<br>cache-hit tool 1.556s，141.5 / 140.4 MB | 无 |
| `codedb_status`<br>健康状态、数量、扫描状态 | one-shot 0.561s，14.2 / 7.9 MB | 无 |
| `codedb_tree`<br>索引树，含语言、行数、符号数 | warm 11.891ms<br>one-shot 1.018s，142.0 / 141.0 MB | 只能部分列文件 |
| `codedb_outline`<br>单文件符号大纲 | warm 0.074ms<br>one-shot 1.279s，140.2 / 140.3 MB | 无 |
| `codedb_symbol`<br>按符号名找定义 | warm 2.106ms<br>one-shot 1.034s，140.7 / 140.0 MB | regex 只能近似文本 |
| `codedb_search`<br>混合搜索、regex、batch 查询 | warm scoped regex 7.120ms<br>one-shot 1.097s，142.3 / 140.5 MB | scoped `rg` 0.047s，warm MCP 快 6.6x<br>broad raw grep 慢 1.5-1.8x |
| `codedb_word`<br>精确 identifier 倒排索引 | warm 首次 lazy load 94.403ms<br>one-shot 1.033s，167.3 / 172.6 MB | 只能部分 word grep |
| `codedb_callers`<br>定义锚定引用 | warm 3.422ms<br>one-shot 1.309s，168.5 / 173.0 MB | 无语义锚定 |
| `codedb_hot`<br>最近修改的索引文件 | warm 7.069ms<br>one-shot 1.454s，141.4 / 140.5 MB | 无 |
| `codedb_deps`<br>正向/反向/传递文件依赖 | warm 0.098ms<br>one-shot 0.528s，29.5 / 23.0 MB | 无 |
| `codedb_read`<br>读索引文件或行范围 | warm 0.757ms<br>one-shot 1.307s，141.7 / 140.1 MB | 只能部分打印文件 |
| `codedb_edit`<br>只读兼容 stub | one-shot 0.128s，4.8 / 1.2 MB | 无 |
| `codedb_changes`<br>按 sequence 查变更文件 | warm 10.818ms<br>one-shot 0.871s，144.7 / 145.8 MB | 无 |
| `codedb_snapshot`<br>files/symbols/deps JSON 快照 | one-shot 2.421s，634.0 / 715.8 MB | 无 |
| `codedb_bundle`<br>一次 MCP 请求内最多 100 个工具调用 | warm 100 fast ops 57.725ms<br>one-shot 20 searches 1.107s，143.3 / 141.5 MB | 无 MCP batching |
| `codedb_remote`<br>remote 兼容 stub | one-shot 0.136s，5.4 / 1.3 MB | 无 |
| `codedb_projects`<br>当前 server process 的项目列表 | one-shot 0.114s，3.8 / 1.0 MB | 无 |
| `codedb_find`<br>模糊文件名/路径查找 | warm 18.019-20.230ms<br>one-shot 0.406s，14.1 / 7.8 MB | 无 fuzzy ranking |
| `codedb_query`<br>find/search/filter/limit/outline pipeline | warm 6.786-25.139ms<br>one-shot 1.149s，141.6 / 140.6 MB | 无等价单工具 |
| `codedb_glob`<br>索引路径 glob 匹配 | warm 4.231ms<br>one-shot 0.956s，140.7 / 140.1 MB | `rg --files -g` 0.045s<br>warm MCP 快 10.6x |
| `codedb_ls`<br>索引目录直接子项 | warm 4.027ms<br>one-shot 0.940s，139.3 / 138.8 MB | 只能部分列文件 |
| `codedb_graph`<br>图摘要/导出 | one-shot 1.988s，389.4 / 396.8 MB | 无 |
| `codedb_explain`<br>解释图节点和出入边 | warm 首次 graph explain 845.369ms<br>one-shot 1.854s，392.8 / 397.6 MB | 无 |
| `codedb_path`<br>图最短路径 | warm after graph load 13.073ms<br>one-shot 1.790s，392.6 / 397.2 MB | 无 |
| `codedb_communities`<br>lazy Louvain communities | warm 265.593ms<br>one-shot 1.905s，390.8 / 400.1 MB | 无 |
| `codedb_module_map`<br>DeepWiki 模块规划 | warm 1.679s<br>one-shot 2.236s，214.4 / 215.3 MB | 无 |
| `codedb_module_atlas`<br>module/file atlas JSON 导出 | one-shot 2.917s，236.2 / 237.9 MB | 无 |
| `codedb_analyze`<br>图统计和建议问题 | warm graph analysis 830.637ms<br>one-shot 2.936s，392.2 / 397.5 MB | 无 |
| `codedb_export`<br>导出 JSON/GraphML/Cypher | warm after graph load 10.313ms<br>one-shot 1.963s，390.0 / 397.0 MB | 无 |

Java 工程 `gameserver`：

| 场景 | Files | Chunks | Symbols | 耗时 | 峰值内存 |
|---|---:|---:|---:|---:|---:|
| 配置/model path 变更后的 cold build | 6,940 | 55,057 | 245,238 | 10.477s | 656.0 / 664.4 MB |
| 文件和配置不变的 cache hit | 6,940 | 55,057 | 245,238 | 1.027s | 129.4 / 176.4 MB |

多语言 smoke benchmark：C#、Java、Rust、Python、Lua、TypeScript、C、C++ 共 8 个文件，8 chunks，14 symbols，0.219s。
Rust smoke check：当前仓库 29 个索引文件，1,752 chunks，1,901 symbols；`codedb_outline`、`codedb_search`、`codedb_deps` 都能返回 Rust 结果。

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
