<div align="center">

<h1>codebase-mcp</h1>

<p><strong>本地优先的 MCP 代码智能工具集：快速代码搜索、依赖感知模块发现、可视化代码 atlas 网页，以及 DeepWiki-style 仓库文档生成。</strong></p>

<p>
  <img alt="Rust MCP" src="https://img.shields.io/badge/Rust-MCP-000000?logo=rust">
  <img alt="tree-sitter indexing" src="https://img.shields.io/badge/tree--sitter-indexing-2f6f9f">
  <img alt="local first" src="https://img.shields.io/badge/local--first-.codedb--mcp-3b82f6">
  <img alt="graph first" src="https://img.shields.io/badge/retrieval-graph--first-7c3aed">
</p>

<p>简体中文 | <a href="README.md">English</a></p>

<p>
  <a href="#项目介绍">项目介绍</a> •
  <a href="#mcp-工具">MCP 工具</a> •
  <a href="#codex-token-观察">Token 观察</a> •
  <a href="#code-module-atlas">Code Module Atlas</a> •
  <a href="#deepwiki">DeepWiki</a> •
  <a href="#benchmark-速览">Benchmark</a> •
  <a href="#推荐-setup-流程">Setup</a> •
  <a href="#skills">Skills</a>
</p>

</div>

## 项目介绍

`codebase-mcp` 会把一个本地仓库变成常驻 MCP 代码智能服务。它把 tree-sitter 索引源码、符号、引用、依赖、图元数据和精确词法索引都放在目标仓库的 `.codedb-mcp` 目录下。

常驻 MCP 进程内的 warm tool 调用目标是毫秒级响应。实测数据见 [Benchmark 速览](#benchmark-速览) 和 [MCP 工具 Benchmark 矩阵](#mcp-工具-benchmark-矩阵)，里面包含 warm 耗时和 `rg` 对比。

## 功能概览

| 领域 | 能力 |
|---|---|
| 快速 MCP 工具 | 一套声明式 property-graph 语言统一表达聚类、依赖、caller、call path、状态和控制证据，并配合精确 body/outline/range。 |
| 模块发现 | 先按依赖连通文件组件划分，再做 dependency-weighted label propagation；路径和术语用于可解释标签和证据。 |
| Code Module Atlas | 打包 meet-blog 风格 3D viewer：一个源码文件一个星点，支持模块/文件列表、依赖边、文件聚焦和详情。 |
| DeepWiki | 基于 MCP 证据和当前 agent 推理生成本地仓库文档，强调业务模块优先、代码引用和源码证据。 |
| Codex token 观察 | 内置 transcript 观察脚本，读取 Codex JSONL 会话，统计 codedb 工具输出 token，并标记高输出检索模式。 |
| 本地部署 | 显式 `.codedb-mcp/codedb-mcp.toml`、项目本地存储、可复制 skills，不依赖隐藏环境变量行为。 |

## MCP 工具

服务会把 tree-sitter 索引和项目本地数据放在 `.codedb-mcp` 下，并暴露五个原子 MCP 工具：

- `codedb_graph_query`：Cypher-like `MATCH / SHORTEST / WHERE / RETURN / ORDER BY / LIMIT` 图查询语言，支持标量/属性比较、community、可排序文件图指标、typed directed edge、有限路径、调用参数、参数绑定、分支效果、guard 和共享状态 join；
- `codedb_symbol`：读取一个精确 definition/body；
- `codedb_outline`：查看一个精确文件；
- `codedb_read`：读取一个精确源码范围；
- `codedb_status`：显式检查健康度和 freshness。

caller、call-path、dependency、flow-atlas、lexical、composite 和 administrative wrapper 保持 CLI/internal；`codedb_bundle` 不对 MCP 暴露。

## Codex Token 观察

`codedb-mcp` skill 内置轻量 Codex transcript 观察脚本：

```powershell
node skills\codedb-mcp\scripts\codex-observe.mjs --project u3dclient --since 24h --top 12
```

它会逐行扫描 `~/.codex/sessions`，按 session `cwd` 过滤目标项目，并输出模型 token、工具输出 token 估算、codedb 调用数量、高输出调用、过宽 read/search、非 codedb 源码查找，以及遗漏的紧凑上下文机会。这个脚本只做诊断，不修改 transcript，也不影响 MCP runtime。

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

推荐的部署方式是 setup-guide first：先把 `setup-for-agent.md` 交给 agent，让它创建 `.codedb-mcp` 并完成 index check；之后询问人类是否要给某个特定 agent 注册 MCP。整个过程不需要下载查询模型。`codedb-mcp` skill 只负责安装后的工具使用，不负责安装。

## Benchmark 速览

测试目标：`u3dclient`。

下面的 agent benchmark 是 2026-07-23 完成的、property-query 改造之前的历史基线。它们保留作为 control，不代表新的 `codedb_graph_query` planner 成绩；统一 MCP/RG 重跑仍待完成。MCP 没有自然语言 task router、source-call 配额或强制输出 clamp；agent 直接提交显式 graph pattern。

当前 Unity C# benchmark 配置索引状态：

- 16,153 个 runtime indexed files
- 27,772 chunks
- 16,153 outlines
- 375,816 indexed symbols
- 17,018 graph nodes
- 98,804 graph edges
- 1,259 cached communities
- 存储目录：`u3dclient\.codedb-mcp`
- cache v32 sidecar：generation 命名的紧凑 `index.*.bin`、`fingerprints.*.bin`、offset-addressed `outlines.*.bin` / `outlines_index.*.bin`、懒加载 `word_index.bin`/`word_hits.bin`、懒加载 `text_search_index.bin`、懒加载 `callers.bin`、懒加载 `deps.*.bin`、持久化 `graph.bin`，并采用 manifest-last 提交。v32 会重建用于 receiver 和 shared-state 消歧的 C# field facts。

从游戏启动到主界面就绪：同一英文 prompt、同一模型的一组完整 MCP/no-MCP 对照：

| 变体 | Effective tokens | 耗时 | 源码工具调用 | 工具输出字符 |
|---|---:|---:|---:|---:|
| codedb-mcp | 122,851 | 414.6s | 41 次 MCP | 244,291 |
| shell / `rg` | 224,922 | 732.8s | 47 条命令 | 742,368 |

在这条生命周期链上，codedb-mcp 的 effective tokens 降低 45.4%，耗时降低 43.4%，工具输出字符降低 67.1%。答案连接了启动入口、AOT/热更新状态切换、框架初始化、登录/全量数据 ready、城市/场景切换、精确 MainPanel open 和最终 `OnShow` readiness body。

下面是更宽的功能分析参考样本，使用相同英文 prompt 和模型。MCP 行取自纯图系列中已经完整结束且未回归的运行；RG control 在禁用 codedb 后一起执行。这些数据用于展示当前质量和成本，不是确定性 SLA：宽多分支问题仍存在 agent 收敛波动。

| 场景 | codedb-mcp | shell / `rg` | Effective-token 变化 | 工具输出变化 |
|---|---:|---:|---:|---:|
| 大地图行军 | 228,915 tokens / 796.4s / 161 calls / 479,094 chars | 253,905 tokens / 787.5s / 102 commands / 1,023,944 chars | 降低 9.8% | 降低 53.2% |
| 英雄属性和战力 | 207,282 tokens / 569.6s / 129 calls / 514,907 chars | 227,248 tokens / 656.9s / 104 commands / 668,652 chars | 降低 8.8% | 降低 23.0% |
| 联盟集结/创建/加入 | 171,449 tokens / 454.0s / 113 calls / 397,873 chars | 196,057 tokens / 619.1s / 37 commands / 851,689 chars | 降低 12.6% | 降低 53.3% |

准确率复核显示，已完成的 MCP 答案实质覆盖了所要求的端到端行为，并能区分 active runtime 与注释/遗留路径。当前主要问题仍是宽多分支问题的调用次数和收敛波动。最后一次同模型合并重跑无法完成，因为 Codex 账号开始拒绝 `gpt-5.6-sol`，提示该模型不受支持；这些失败运行没有被当作“零 token 胜利”计入表格。

索引和 cache 基线：

| 场景 | 耗时 | 峰值 WS / private | 说明 |
|---|---:|---:|---|
| cold rebuild | 13.818s | 226.9 / 220.2 MB | graph-only cache v28 之前的历史结果，等待重跑 |
| cache-hit index open | 0.741s | 107.8 / 106.1 MB | 文件和配置不变，manifest cache hit |
| status open | 0.454s | 14.4 / 8.1 MB | 已有 cache 下的 count/status 命令 |
| trigram text sidecar size | 107.7 MB | n/a | sorted trigram lookup + contiguous file-id postings |

同一份 `u3dclient` runtime C# 配置下的增量 cache 维护：

| 场景 | 耗时 | Cache 结果 | 说明 |
|---|---:|---|---|
| 新增 1,000 个小 `.cs` 文件 | 1.508s warm apply | `live-incremental` | 只解析新增文件并更新 compact cache path |
| 修改 1,000 个小 `.cs` 文件 | 1.544s warm apply | `live-incremental` | 复用刚 parse 出来的源码内容刷新依赖 |
| 删除 1,000 个小 `.cs` 文件 | 0.504s warm apply | `live-incremental` | 过滤当前文件和依赖 sidecar，不做全量 rebuild |

## MCP 工具 Benchmark 矩阵

这个表刻意压成 3 列，避免 GitHub README 页面出现横向滚动条。标注 `after load` 的行不包含同一进程里的首次 lazy sidecar 加载。`codedb_text_search` 使用同 query warm 样本，也就是先 warm 一次再测；第一次未见过的 full-result text query 仍需要扫描候选文件取行文本，在 `u3dclient` 上通常是 15-30ms。

| Tool / 用途 | MCP 实测 | rg 对比 |
|---|---|---|
| `codedb_graph_query`<br>typed property-graph 证据链 | property-query benchmark 待重跑；正确性 smoke 已覆盖 shortest call/dispatch、call-site guard、argument-to-parameter、branch `PREVENTS/REACHES` 和 shared-state producer join | 没有等价单条 `rg` 命令 |
| `codedb_index`<br>构建/重建本地索引 | cold rebuild 13.818s；峰值 226.9 / 220.2 MB | 无 |
| `codedb_status`<br>健康状态、数量、扫描状态 | one-shot status open 0.454s；cache hit | 无 |
| `codedb_version`<br>server/package 版本 | trivial response；不加载项目索引 | 无 |
| `codedb_tree`<br>索引树，含语言、行数、符号数 | 8.782ms | 只能部分列文件 |
| `codedb_outline`<br>单文件符号大纲 | 抽样文件 0.088-0.351ms；100-call p95 0.214ms after first load | 无 |
| `codedb_symbol`<br>按符号名找定义 | 2.021ms | regex 只能近似文本 |
| `codedb_text_search`<br>trigram 全文和 regex 搜索 | 同 query warm exact `PoolManager` 0.202ms，`Joystick` 0.442ms；scoped `NetworkListenerManager` 0.103ms；Alliance regex 0.148ms | 等价 `rg`：5.007s、5.859s、77.011ms、103.702ms；这些 warm text 路径 codedb 约快 24,840x / 13,250x / 748x / 701x |
| `codedb_search`<br>符号/word-trigram 词法排序 | graph-only 版本等待重跑 | regex route 委托 `codedb_text_search` |
| `codedb_context`<br>图 atlas 或 scoped 图证据 | graph-only 版本等待重跑 | 替代大范围 search/outline/deps 准备循环 |
| `codedb_context`<br>预算化源码上下文片段 | `PoolManager` 7.050ms，`max_chars=10000`；业务短语 29.037ms，`max_chars=12000`；抽样输出约 2.5k-3.0k tokens | 替代反复 read；输出由 `max_chars` 硬限制 |
| `codedb_word`<br>精确 identifier 倒排索引 | 101.526ms，包含 lazy word sidecar 访问 | 只能部分 word grep |
| `codedb_callers`<br>定义锚定引用 | `PoolManager` 平均 12.722ms、steady 样本约 8ms；`Joystick` 17.968ms | 无语义锚定 |
| `codedb_hot`<br>最近修改的索引文件 | 2.116ms | 无 |
| `codedb_deps`<br>正向/反向/传递文件依赖 | depends_on 0.096ms；imported_by 首次 sidecar 路径 170.495ms，之后 sub-ms 抽样 | 无 |
| `codedb_read`<br>读索引文件或行范围 | 0.562ms | 只能部分打印文件 |
| `codedb_edit`<br>只读兼容 stub | trivial error response | 无 |
| `codedb_changes`<br>按 sequence 查变更文件 | 7.849ms | 无 |
| `codedb_snapshot`<br>files/symbols/deps JSON 快照 | 1.213s | 无 |
| `codedb_remote`<br>remote 兼容 stub | trivial error response | 无 |
| `codedb_projects`<br>当前 server process 的项目列表 | 0.013ms | 无 |
| `codedb_find`<br>模糊文件名/路径查找 | 验证样本 20-26ms；矩阵样本 47.150ms | 无 fuzzy ranking |
| `codedb_query`<br>find/search/filter/limit/outline pipeline | 验证样本 9-60ms；矩阵样本 63.546ms | 无等价单工具 |
| `codedb_glob`<br>索引路径 glob 匹配 | Alliance `.cs` glob 4.465ms | `rg --files -g` 52.961ms；codedb 快 11.9x |
| `codedb_ls`<br>索引目录直接子项 | 1.421ms | 只能部分列文件 |
| `codedb_module_atlas`<br>module/file atlas JSON 导出 | warm export 7.278s；full sampled export 7.960s，峰值 286.2 / 289.1 MB，18,852 file points，1,742 modules | 无 |

Java 工程 `gameserver`：

| 场景 | Files | Chunks | Symbols | 耗时 | 峰值内存 |
|---|---:|---:|---:|---:|---:|
| graph-only cache v28 之前的历史 cold build | 6,940 | 13,966 | 245,238 | 3.939s | 212.7 / 212.4 MB |
| cache-hit index open | 6,940 | 13,966 | 245,238 | 0.394s | 73.7 / 69.3 MB |

多语言 smoke benchmark：C#、Java、Rust、Python、Lua、TypeScript、C、C++ 共 8 个文件，8 chunks，14 symbols，0.069s，峰值 2.0 / 0.5 MB。
Rust smoke check：当前仓库 32 个索引文件，1,183 chunks，2,214 symbols；index 0.372s，峰值 170.9 / 162.1 MB；`codedb_outline`、`codedb_search`、`codedb_deps` 都能返回 Rust 结果。

## 推荐 Setup 流程

1. 把 `setup-for-agent.md` 交给目标 agent。
2. agent 创建 `<repo-root>\.codedb-mcp`。
3. agent 从模板写入 `<repo-root>\.codedb-mcp\codedb-mcp.toml`，并告诉人类当前扫描范围。
4. 人类可以在第一次索引前修改 `extensions`、`root_paths`、`include_paths`、`exclude_paths` 和 `skip_dirs`。
5. agent 跑一次 index 检查；不需要下载模型。
6. agent 询问人类是否要给当前特定 agent 配置 MCP；确认后才按该 agent 的方式配置。
7. 重启或 reload agent MCP session，然后检查 `/mcp`。

MCP 命令形态：

```text
<package-root>\skills\codedb-mcp\assets\codebase-mcp.exe --config <repo-root>\.codedb-mcp\codedb-mcp.toml mcp <repo-root>
```

这个项目刻意保持安装显式化：setup 只初始化项目本地文件，agent/user 决定何时、在哪里注册 MCP。

## 主要能力

- 通过 MCP 暴露一套只读图查询语言，以及精确 symbol、outline、原子源码 range 和 status。caller、dependency、call path、聚类、共享状态和控制流统一表达为图 pattern，而不是独立 MCP wrapper。
- 所有配置都来自目标项目内的 `.codedb-mcp/codedb-mcp.toml`，不依赖环境变量切换行为。
- 所有生成数据都放在目标项目的 `.codedb-mcp` 目录下；删除该目录即可清理本地索引、缓存和生成输出。
- 使用统一 tree-sitter 解析层支持 C#、Java、Rust、Python、Lua、JavaScript、TypeScript/TSX、C、C++。
- C#/Java 的 typed callers 和 deps 额外实现 namespace/package import、qualified name、using alias、static using、annotation、attribute suffix 等规则，准确性最强。
- CLI/internal 显式搜索使用 BM25、word/trigram 和精确 identifier 索引；MCP 发现通过 `Community`/`File` 图查询完成，server 不加载查询模型。
- 构建 graph-backed flow/callpath 证据；`codedb_module_atlas` 是 Rust 原生模块视图，先按依赖连通文件图划分，再在连通块内做 dependency-weighted label propagation，并输出依赖内聚度、入口点、关键符号和 c-TF-IDF-like 标签。
- MCP 模式默认用文件系统事件队列收集变更，并每 5 秒把队列内的文件作为一个批次应用，避免大工程每轮全量扫描。

## 技术架构

1. **显式配置层**：读取 `.codedb-mcp/codedb-mcp.toml`，配置扫描扩展、文件大小上限、gitignore 行为、root paths、include paths、exclude globs、skip dirs、watch 和 storage。
2. **本地存储层**：索引 payload、manifest、生成证据和文档都写入 `.codedb-mcp`。数据跟随项目目录，不写全局数据库。
3. **扫描层**：基于配置遍历代码库，读取项目内 `.gitignore`，但目标 root 下的嵌套 Git worktree/submodule 会作为普通源码目录继续索引。Unity runtime 可以把 root paths 限定到 `Assets`、`Packages`、`Library/PackageCache`，并用 `**/Editor/**` 排除 Editor-only 代码。
4. **语言解析层**：所有语言统一走 tree-sitter grammar，输出同一套 `FileEntry` 和 `Symbol` 结构。当前支持 C#、Java、Rust、Python、Lua、JavaScript、TypeScript/TSX、C、C++，解析时只遍历声明层，避免大型方法体拖慢索引。
5. **代码语义增强层**：C#/Java 上继续做 namespace/package import、别名、静态 using、注解、属性后缀、限定名引用等轻量语义推断；Lua 会抽取 `require()` 并生成轻量文件依赖。
6. **搜索索引层**：cold index 阶段构建 chunk 元数据、symbol definition hits 和 dependency references。`codedb_text_search` 会生成 codedb-style trigram sidecar，采用 sorted lookup entries 和 contiguous file-id postings 做 exact/regex 文本搜索。`codedb_search` 使用 BM25、symbol 和 word evidence，不加载查询模型。
7. **内存友好的增量缓存层**：cache v32 延续 bounded content cache：完整正文、重复路径/字符串、word hits、caller、deps 和 graph 对象都不默认全部常驻；工具按需读取 sidecar。watcher 只解析变更文件并复用未变数据。
8. **依赖与图层**：把 dependency community、可排序 file graph 指标与 lazy semantic provider 组合，生成 precise call、interface dispatch、call site、argument、parameter binding、condition、control action、preprocessor guard 和 shared-state read/write facts。只读 Cypher-like planner 把 pattern、recursive expansion、filter、projection、comparison 和 ordering 分层；`MATCH SHORTEST` 使用双向 connector search。
9. **模块 atlas 层**：`codedb_module_atlas` 先按依赖图弱连通分量切开文件，再在每个连通块内部做依赖加权 label propagation。路径和 token 只用于命名、证据展示和过大连通块拆分，不作为主要聚类依据。`codedb_module_atlas` 导出 Embedding Atlas 可视化数据。
10. **MCP 工具层**：基于 Rust `rmcp` SDK 的 stdio server 实现；工具运行在 warm in-process index 上，通过原子 graph/source 操作渐进获取证据。
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
root_paths = []
include_paths = ["Library/PackageCache"]
exclude_paths = []

[logging]
enabled = false
file = ".codedb-mcp/codedb-mcp.log"
queue_capacity = 8192
flush_interval_ms = 500

[storage]
enabled = true
dir = ".codedb-mcp"
```

`root_paths` 可以把扫描限定在指定源码根目录，例如 Unity runtime 常用 `Assets`、`Packages`、`Library/PackageCache`；`include_paths` 会额外纳入路径并覆盖被跳过的父目录；`exclude_paths` 支持 `**/Editor/**` 这类 glob，用于排除 Editor-only 代码。`respect_gitignore=true` 会读取项目内 `.gitignore`，但目标 root 下的嵌套 Git worktree/submodule 仍会被当作源码目录索引，除非被 `skip_dirs`、`exclude_paths` 或扩展名规则排除。`[logging]` 默认关闭；开启后，MCP tool 调用和文件监听消化批次都会通过有界非阻塞队列交给后台线程写入。tool 日志包含 `elapsed_ms`、`status`，失败时包含 `failure_reason`；watcher 消化日志包含队列事件数、changed/deleted 数、消化状态、耗时和 cache 统计。队列满时丢弃日志，不拖慢索引或 MCP 主流程。

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

MCP 模式会先完成协议握手，再在后台构建默认项目索引；如果索引还没完成，早期工具调用会等待首次构建结束。`[watch] enabled = true` 默认开启，`poll_interval_seconds = 5` 表示文件系统事件会先进入队列，并在每个 tick 合并成一个批次应用。这个 tick 也会检查配置文件 hash；如果 MCP 运行中修改了扫描范围、扩展名、include/exclude、文件大小上限、gitignore 或 storage 配置，codedb-mcp 会继续用旧索引服务，同时后台 full reindex，成功后原子切换到新索引并重建 watcher roots；解析或重建失败时保留旧索引。重建由进程内单一锁串行化，新一轮 tick 会等待当前更新结束，不会打断正在写入的 index。cache 提交采用 manifest-last 和 generation sidecar：如果 index 过程中进程被强杀，旧 manifest 仍指向旧可用 cache，新一代半成品会被忽略；下次启动会清理没有被 manifest 引用的旧 generation 文件。text search、word hits、callers 等 lazy sidecar 会按 source 校验并在需要时重建，不在 reload 关键路径同步删除。

## 工具简介

| Tool | 用途 |
|---|---|
| `codedb_text_search` | trigram 加速全文/regex 搜索；支持 `queries` batch、`path_glob` 和 scope |
| `codedb_search` | BM25 + 符号/word-trigram 词法搜索；regex fallback 走 `codedb_text_search`；支持 `queries` batch |
| `codedb_context` | 面向答案的上下文构建；返回排序文件、命中原因、关键符号和依赖信号，不 dump 大段源码 |
| `codedb_context` | 预算化源码上下文；按 query 或显式 path 返回 outline、依赖和行号片段，受 `max_chars` 限制 |
| `codedb_callers` | LSP-like 引用查找；支持 definition path/line 锚定和 `targets` batch |
| `codedb_deps` | 文件依赖和反向依赖；支持 transitive |
| `codedb_outline` | 返回预计算符号大纲，不在请求时重新 parse |
| `codedb_symbol` | 按符号名找定义 |
| `codedb_word` | 精确 identifier 倒排索引查询 |
| `codedb_read` | 读取索引文件或行范围；支持 `paths` batch，数组项可以覆盖行号和 compact |
| `codedb_find` | 模糊文件名/路径查找 |
| `codedb_query` | find/search/filter/limit/outline 小型 pipeline |
| `codedb_version` | 返回 server/package 版本，不加载项目索引 |
| `codedb_module_atlas` | 导出 module/file atlas JSON，供 `skills/code-module-atlas` 生成网页 |
| `codedb_hot` | 最近修改的索引文件 |
| `codedb_status` | 索引健康状态和统计 |

## Skills

`skills/` 目录可以作为独立包复制。

- `setup-for-agent.md`：给 agent 用的安装指导，写入项目本地扫描/存储配置并完成 index check，不下载查询模型。
- `skills/codedb-mcp`：包含 `assets/codebase-mcp.exe`、配置模板、MCP 注册参考、工具使用建议，以及用于 Codex transcript token 诊断的 `scripts/codex-observe.mjs`；不负责安装。
- `skills/deepwiki`：使用本地 `codedb_*` 工具和当前 agent 的推理能力生成 DeepWiki-style 文档，强调业务模块边界，而不是只按文件夹或 community 分组。
- `skills/code-module-atlas`：调用 `codedb_module_atlas` 生成本地 3D 模块/文件 atlas 网页；项目特定 JSON 是生成物，不提交。

## 发布说明

- 推荐发布时带上 `setup-for-agent.md` 和整个 `skills/` 目录；先由 setup guide 初始化项目，再按需安装/使用 skill。
- `.codedb-mcp/index.bin`、`.codedb-mcp/manifest.json`、`.codedb-mcp/*.bin` 是项目本地生成物，不建议提交。
- 旧 `.codebase-mcp` 名称已经迁移为 `.codedb-mcp`；配置文件名也统一为 `codedb-mcp.toml`。

## 致谢

- [meet-blog.buyixiao.xyz](https://meet-blog.buyixiao.xyz/) 启发了 Code Module Atlas 的视觉风格和 viewer 体验。
- [justrach/codedb](https://github.com/justrach/codedb) 启发了最初的 MCP 工具接口方向。

