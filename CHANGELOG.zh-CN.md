# 更新日志

[English version](CHANGELOG.md)

## Unreleased - 2026-06-03

### 新增

- 新增 `codedb_version` MCP 工具，直接返回编译进可执行文件的 server/package 版本，不加载项目索引。

### 变更

- 将 crate、CLI `--version` 和 MCP serverInfo 使用的版本源统一提升到 `0.5.0`。
- 将当前 `codedb_search` 的 lexical 路径替换为固定的 symbol/word-trigram 文本命中 + lazy Model2Vec 向量融合。cold index 不再构建旧的 lexical ranker，regex 和 exact text search 继续使用 codedb-style trigram sidecar。
- 为 `codedb_text_search` 和 regex route 的 `codedb_search` 行命中增加有上限的进程内结果缓存，对齐 README warm-tool benchmark 口径，同时不让完整源码常驻内存。
- 新增 `codedb_context` 和 `codedb_explore`，让 agent 可以一次拿到排序后的答案上下文和预算化源码片段，而不是手动串联 search、outline、deps 和 read。
- 新增 `skills/codedb-mcp/scripts/codex-observe.mjs`：流式读取 `~/.codex/sessions` 的 Codex transcript，按项目 cwd 过滤，并统计 codedb 工具输出 token、bundle 子工具分布、高输出调用、过宽 read/search、非 codedb 源码查找，以及遗漏的 bundle/context 使用机会。
- 更新 MCP tool description 和 `skills/codedb-mcp` 使用说明，写清楚新的搜索路径，并继续要求多步查询优先通过 `codedb_bundle` 合并。

### Benchmark 与验证

- 重新跑完 README benchmark suite，目标 `u3dclient`：18,852 个 indexed files、31,428 chunks、274,606 symbols、19,746 graph nodes、162,823 graph edges、1,356 cached communities。
- 重新测量 cold/index/cache：`u3dclient` cold rebuild 13.818s，峰值 226.9 MB WS / 220.2 MB private；cache-hit index open 0.741s，峰值 107.8 MB WS / 106.1 MB private；status open 0.454s，峰值 14.4 MB WS / 8.1 MB private。
- result-cache 修复后重新测量 focused warm text-search 和 `rg` 对比：同 query warm `PoolManager` 0.202ms vs `rg` 5.007s，`Joystick` 0.442ms vs 5.859s，scoped `NetworkListenerManager` 0.103ms vs 77.011ms，Alliance regex 0.148ms vs 103.702ms。第一次未见过的 full-result text query 仍需要扫描候选文件取行文本，在 `u3dclient` 上通常是 15-30ms。
- 在 `u3dclient` 上测量新 context 工具：warm `codedb_context PoolManager` 6.573ms，业务短语 context 在 vector load 后 29.670ms；warm `codedb_explore PoolManager` 7.050ms（`max_chars=10000`），业务短语 explore 29.037ms（`max_chars=12000`）。
- 重新测量 `u3dclient` 的 1,000 文件增量维护：新增 1.508s，修改 1.544s，删除 0.504s，均走 `live-incremental` path。
- 重新测量语言和 atlas smoke：`gameserver` Java cold rebuild 3.939s，峰值 212.7 MB WS / 212.4 MB private；当前仓库 Rust index 0.372s；多语言 smoke 0.069s；module atlas full sampled export 7.960s，峰值 286.2 MB WS / 289.1 MB private。
- 在最近的 `u3dclient` Codex 会话上验证 transcript 观察脚本：20 个候选 JSONL 文件扫描 185.1ms，匹配 19 个 session，识别 301 次 codedb 调用、134 次 bundle 调用和 834 个 bundle 子工具 section，并生成高输出 codedb/源码查找问题列表。
- 移除 feature 调用次数限制和强制 bundle 输出 clamp 后，重新测量 `联盟集结和加入集结` Codex benchmark：启用 codedb-mcp 为 128,865 tokens / 300.4s，禁用 codedb-mcp 为 185,448 tokens / 485.2s，节省 56,583 tokens（30.5%），耗时快 38.1%。README 合计更新为启用 335,940 tokens / 920.9s，对比禁用 590,834 tokens / 1,482.9s，节省 254,894 tokens（43.1%），耗时快 37.9%。

## Unreleased - 2026-05-28

### 新增

- 新增 `codedb_text_search`：对齐 codedb 思路的 trigram 加速全文 MCP 工具，支持 exact、regex、path scope、compact、scope 和 batch 查询。
- `codedb_read` 新增 `paths` batch 支持；数组项可以是路径字符串，也可以是覆盖行号、compact 和 hash 参数的对象。
- 新增 Lua 语言支持：接入 `tree-sitter-lua`，支持 `.lua` 扫描、`require()` import 抽取、常见 Lua 函数 outline 抽取，并补充 Lua 注释识别用于 compact search 输出。

### 变更

- 调整源码扫描逻辑：目标 root 下的嵌套 Git worktree/submodule 会作为普通源码目录索引。`respect_gitignore=true` 仍然读取项目内 `.gitignore`，但 `.git/info/exclude`、全局 gitignore 和嵌套 Git 仓库边界不再决定 codebase 边界。
- 降低大工程 warm index 内存：identifier hits 改为紧凑 file id；大工程图层保留 file/namespace/dependency 级别，symbol 仍保留在 outline/search/callers 专用索引；BM25 构建改为不保留完整临时 token 语料；缓存内不再保存完整文件源码正文，工具按需读取文件内容。
- 进一步降低 cache hit 内存：graph、反向依赖、BM25 postings、embedding vectors、Model2Vec/vector store 都改成懒加载。符号形态的 `codedb_search` 现在走 BM25 + symbol 增强，不加载 embeddings。
- 将常驻 Vicinity HNSW 向量索引替换为自然语言搜索时懒加载的 flat cosine 文件向量扫描，移除 HNSW 依赖和对应图内存。
- 压缩重复内存元数据：symbol kind 和源码 language 改为小枚举；chunk 文件路径改为 file id，避免每个 chunk 重复保存路径字符串。
- 将正向依赖图移动到懒加载的 `deps.bin` sidecar，search/status/callers 不再常驻依赖图；dependency 和 graph/module 工具按需加载。
- 将 cache v20 拆成小 JSON manifest、二进制源码 fingerprint、紧凑 hot `index.bin`、spill-to-disk BM25 postings、懒加载 word-index sidecar、懒加载依赖和按需生成 embedding。单次 `codedb_status`、`codedb_find`、`codedb_deps` 现在可以直接从 sidecar 返回，不反序列化完整 index。
- 将 cache 维护升级到 cache v21 generation 文件和 manifest-last 提交。MCP watcher refresh 现在会复用未变化文件 metadata、只解析新增/修改文件、合并旧依赖 sidecar，并 remap BM25 postings，不再因为源码有变化就退化为全量 source reindex。
- 为业务短语搜索增加 BM25 候选足够时的 fast path，常见多词 query 可直接返回 lexical 结果而不加载 Model2Vec；同时在格式化 search preview 时复用同一文件内容读取。
- 为 definition-anchored `codedb_callers` 增加懒生成的 `callers.bin` sidecar。未缓存 target 第一次仍走完整 caller 路径并写入 sidecar；重复 one-shot 查询可直接从 sidecar 返回，不加载完整 index。
- 将 cold index 重构为 cache v20：每个文件 tree-sitter 解析并生成 chunk 元数据后立即释放源码正文；依赖和 BM25 按需重读源码；BM25 构建将 doc-term 记录 spill 到磁盘；Model2Vec embeddings 改为懒生成；cache save 前不再 clone 第二份 file/symbol 元数据。
- 优化 `codedb_module_atlas`：默认 skill 路径下把拆分后的 file-point JSON 流式写盘，不再额外保留完整 points 数组；point id 构建时复用路径引用；布局计算改为 grid/direct hybrid。atlas 导出现在会保留每一个 indexed file 作为 viewer node，同时把依赖孤立的单文件组件按目录合并，避免它们被静默丢弃，也避免膨胀成几千个单文件模块。`code-module-atlas` skill 也改为直接调用内置 Node 转换脚本，避免每次通过 `npm run` 额外启动 npm。
- 扩展扫描过滤配置：新增显式 `root_paths` 和基于 glob 的 `exclude_paths`，同时保持 `include_paths` 与 `skip_dirs` 兼容。Unity runtime 扫描现在可以配置为 `root_paths = ["Assets", "Packages", "Library/PackageCache"]`，并通过 `exclude_paths = ["**/Editor", "**/Editor/**"]` 排除 Editor-only 代码。
- 将 warm watcher 维护改为按 `poll_interval_seconds` 批处理的文件系统事件队列，大量编辑时不再每轮扫描完整源码树。
- 增加 cache v23 outline sidecar：用 offset/length 索引预计算 outline，one-shot `codedb_outline` 可以直接 seek 到目标文件的大纲记录。
- warm BM25 增量从“重写 postings 文件”改为 base postings + live overlay。overlay 会 remap 未变化的 base doc id，并把变更/新增 doc 保存在内存层，避免 MCP watcher 更新时重写完整 postings。
- 将 live dependency refresh 收窄到变更文件出现过的符号名，并复用刚 parse 出来的源码内容生成依赖和 BM25 replacement token。
- 将 raw text search 从混合语义搜索里拆出来：`codedb_text_search` 通过懒加载的 `text_search_index.bin` trigram sidecar 负责 exact/regex 行匹配；`codedb_search` 保留 BM25/symbol/vector 混合搜索，并把 regex fallback 委托给 text index。
- 更新 `setup-for-agent.md`：setup 时会给项目根目录 `AGENTS.md` 和 `CLAUDE.md` 追加 codedb-mcp 使用约定，要求后续 agent 在自然语义搜索、精确文本搜索、符号引用、文件上下文、依赖查询和批量查询时优先使用索引化 MCP 工具。

### 修复

- 修复主工程目录下的子模块源码不会被索引的问题。
- 修复极小工程在 embedding 输出为空时 vector store 构建维度为 0 的问题，改为使用配置模型维度作为 fallback。

### Benchmark 与验证

- cache v20 后重新测量 `u3dclient`：19,035 个 indexed files、31,949 个 chunks、277,213 个 symbols，graph 估算为 19,941 个 nodes / 166,132 条 edges，并只在 graph/module 工具需要时构建。
- 对 `u3dclient` 重新执行 cache v20 冷重建峰值内存采样：26.335s internal / 26.621s wall、256.4 MB working set、250.2 MB private bytes。
- 重新测量 cache-hit index open：0.873s internal / 1.132s wall、134.9 MB working set、136.0 MB private bytes。
- 重新测量 `u3dclient` fast one-shot wall time 和峰值内存：`codedb_status` 0.252s、14.1 MB WS / 7.9 MB private，`codedb_find PoolManager` 0.283s、14.4 MB WS / 8.2 MB private，`codedb_deps PoolManager.cs` 0.303s、34.8 MB WS / 28.3 MB private，`codedb_search PoolManager` 0.739s、151.5 MB WS / 154.8 MB private，`codedb_callers PoolManager` sidecar hit 0.243s、14.2 MB WS / 7.8 MB private。
- 修正 `gameserver` 显式模型路径后重新测量 Java benchmark：6,940 个 files、55,057 个 chunks、245,238 个 symbols，重建 10.477s，cache hit 重新打开 1.027s。
- 更新 README 里的 `rg` 对比：cache v20 为降低内存不再常驻完整文件正文，所以未限定范围的大 regex 会按需读源码，可能比 `rg` 慢；path-scoped regex、符号搜索、引用、依赖、outline 和 bundle 仍保持低延迟。
- 验证移除常驻完整文件源码正文后的按需读文件工具：`codedb_search PoolManager`、基于定义锚点的 `codedb_callers PoolManager`、`codedb_read PoolManager.cs`。
- 重新测量 `u3dclient` 上的 `code-module-atlas`：Rust `codedb_module_atlas` full default export 现在导出 19,035 个 file nodes 和 1,850 个 modules，耗时 8.548s，采样峰值 319.8 MB WS / 323.5 MB private；完整 skill 路径为 10.870s wall time，进程树采样峰值 371.8 MB WS / 369.9 MB private。旧 atlas export 只有 16,365 个 nodes，因为 `min_files=2` 会过滤 singleton dependency components。
- 在 `u3dclient` 上验证 Unity runtime 扫描配置：`Assets`、`Packages`、`Library/PackageCache` 被索引；`**/Editor/**/*.cs` 无 indexed matches；PackageCache runtime 文件仍可搜索。
- 在 `u3dclient` Unity runtime C# 配置上重新测量 live overlay/event-queue 后的增量维护：cache-hit `codedb_status` one-shot 0.392s；新增 1,000 个小 `.cs` 文件 warm apply 0.856s；修改 1,000 个文件 warm apply 0.811s；删除 1,000 个文件 warm apply 0.375s；warm path 返回 `cache: live-incremental`。
- README 工具 benchmark 改为 warm in-process 口径，不再展示 one-shot 工具耗时。当前 `u3dclient` warm text-search 样本：`codedb_text_search PoolManager` 1.041ms、`Joystick` 1.790ms、regex `class\\s+Joystick` 25.767ms；同 root 且排除 Editor 的 `rg` 分别为 1238.513ms、451.176ms、3312.850ms。

## Unreleased - 2026-05-27

### 新增

- 新增 `skills/code-module-atlas` skill。它会调用已有的 `codedb_module_atlas` MCP 工具，把导出的模块/文件图转换成内置 meet-blog 风格 viewer 的数据集，并启动本地 3D 代码 atlas 网页。
- 新增自包含的代码 atlas viewer：`skills/code-module-atlas/assets/viewer`，包含 vendored 前端资源、数据转换脚本、前端 patch 脚本、Vite 构建和运行脚本。
- 新增 `setup-for-agent.md` 作为显式 setup 指南。setup 不再放在 `codedb-mcp` skill 内，而是指导 agent 创建项目本地 `.codedb-mcp` 配置、解析模型路径，并在注册特定 agent 的 MCP 前询问用户。
- 新增 README 演示素材：
  - `docs/assets/code-module-atlas.gif`
  - `docs/assets/code-module-atlas.mp4`
- 新增 Rust 语言支持，并补充当前多语言支持矩阵说明。

### 变更

- 将所有 module-atlas 网页相关代码统一收纳到 `skills/code-module-atlas`；仓库其它部分只把 `codedb_module_atlas` 当作 Rust/MCP 数据导出层。
- 更新 `skills/codedb-mcp`，让它专注于操作已经配置好的 MCP server；它不再负责 setup 或特定 agent 的 MCP 注册。
- 更新 `skills/deepwiki`，把 DeepWiki 文档规划和可视化 atlas 生成拆开。DeepWiki 使用 `codedb_module_map` 做页面规划，需要可视化模块/文件图时交给 `code-module-atlas`。
- 更新模块规划流程：优先使用依赖连通文件组件和依赖加权 label propagation；路径和术语只作为命名、解释和证据，不作为主要分组依据。
- 更新配置说明：所有行为都显式写在 `.codedb-mcp/codedb-mcp.toml`，包括语言扩展、include paths、storage 和绝对模型路径。
- 更新扫描默认值和文档，覆盖大文件、多语言扩展，以及 Unity `Library/PackageCache` 通过 `include_paths` 显式纳入索引的用法。
- 更新英文 README 和中文 README：补充技术架构、benchmark、MCP vs `rg` 对比、skill 打包说明和 Code Module Atlas 演示。

### 移除

- 移除旧的 `skills/codedb-mcp/scripts/setup.ps1` setup 路径。
- 移除 DeepWiki 内重复的 `module-atlas-workflow.md`，避免维护第二份 module-atlas 流程文档。
- 移除旧的外部 `tools/module-atlas-viewer` 维护路径；viewer 生成数据被忽略，不提交。

### Benchmark 与验证

- 记录 Unity C# benchmark 数据，目标为 `u3dclient`：19,030 个 indexed files、129,790 个 chunks、277,008 个 symbols、296,941 个 graph nodes、691,419 条 graph edges。
- 记录 Java benchmark 数据，目标为 `gameserver`：6,940 个 files、55,057 个 chunks、245,238 个 symbols。
- 记录 C#、Java、Rust、Python、Lua、TypeScript、C、C++ 路径的多语言 smoke 覆盖。
- 记录 warm MCP 工具耗时：`codedb_search`、`codedb_callers`、`codedb_deps`、`codedb_outline`、`codedb_find`、`codedb_query`、`codedb_analyze`、`codedb_bundle`。
- 在 `u3dclient` 上验证 `code-module-atlas`，生成 16,361 个文件节点、62,771 条依赖边和 1,374 个模块。

### 打包

- 将以下 skills 打包为可独立复制的目录：
  - `skills/codedb-mcp`
  - `skills/deepwiki`
  - `skills/code-module-atlas`
- 确保项目特定的 atlas 生成文件、Vite 构建输出和 `node_modules` 都保持 ignored，不进入仓库提交。

