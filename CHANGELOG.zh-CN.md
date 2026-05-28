# 更新日志

[English version](CHANGELOG.md)

## Unreleased - 2026-05-28

### 新增

- 新增 Lua 语言支持：接入 `tree-sitter-lua`，支持 `.lua` 扫描、`require()` import 抽取、常见 Lua 函数 outline 抽取，并补充 Lua 注释识别用于 compact search 输出。

### 变更

- 调整源码扫描逻辑：目标 root 下的嵌套 Git worktree/submodule 会作为普通源码目录索引。`respect_gitignore=true` 仍然读取项目内 `.gitignore`，但 `.git/info/exclude`、全局 gitignore 和嵌套 Git 仓库边界不再决定 codebase 边界。
- 降低大工程 warm index 内存：identifier hits 改为紧凑 file id；大工程图层保留 file/namespace/dependency 级别，symbol 仍保留在 outline/search/callers 专用索引；BM25 构建改为不保留完整临时 token 语料；缓存内不再保存完整文件源码正文，工具按需读取文件内容。
- 进一步降低 cache hit 内存：graph、反向依赖、BM25 postings、embedding vectors、Model2Vec/vector store 都改成懒加载。符号形态的 `codedb_search` 现在走 BM25 + symbol 增强，不加载 embeddings。
- 将常驻 Vicinity HNSW 向量索引替换为自然语言搜索时懒加载的 flat cosine 文件向量扫描，移除 HNSW 依赖和对应图内存。
- 压缩重复内存元数据：symbol kind 和源码 language 改为小枚举；chunk 文件路径改为 file id，避免每个 chunk 重复保存路径字符串。
- 将正向依赖图移动到懒加载的 `deps.bin` sidecar，search/status/callers 不再常驻依赖图；dependency 和 graph/module 工具按需加载。
- 将 cache v18 拆成小 JSON manifest、二进制源码 fingerprint、紧凑 hot `index.bin`、懒加载 BM25 postings、懒加载 word-index sidecar、懒加载依赖和懒加载 embedding。单次 `codedb_status`、`codedb_find`、`codedb_deps` 现在可以直接从 sidecar 返回，不反序列化完整 index。
- 为业务短语搜索增加 BM25 候选足够时的 fast path，常见多词 query 可直接返回 lexical 结果而不加载 Model2Vec；同时在格式化 search preview 时复用同一文件内容读取。
- 为 definition-anchored `codedb_callers` 增加懒生成的 `callers.bin` sidecar。未缓存 target 第一次仍走完整 caller 路径并写入 sidecar；重复 one-shot 查询可直接从 sidecar 返回，不加载完整 index。

### 修复

- 修复主工程目录下的子模块源码不会被索引的问题。
- 修复极小工程在 embedding 输出为空时 vector store 构建维度为 0 的问题，改为使用配置模型维度作为 fallback。

### Benchmark 与验证

- cache v18 后重新测量 `u3dclient`：19,035 个 indexed files、129,858 个 chunks、277,213 个 symbols，graph 估算为 19,941 个 nodes / 166,132 条 edges，并只在 graph/module 工具需要时构建。
- 重新测量 `u3dclient` fast one-shot wall time 和峰值内存：`codedb_status` 0.252s、14.1 MB WS / 7.9 MB private，`codedb_find PoolManager` 0.283s、14.4 MB WS / 8.2 MB private，`codedb_deps PoolManager.cs` 0.303s、34.8 MB WS / 28.3 MB private，`codedb_search PoolManager` 0.739s、151.5 MB WS / 154.8 MB private，`codedb_callers PoolManager` sidecar hit 0.243s、14.2 MB WS / 7.8 MB private。
- 修正 `gameserver` 显式模型路径后重新测量 Java benchmark：6,940 个 files、55,057 个 chunks、245,238 个 symbols，重建 10.477s，cache hit 重新打开 1.027s。
- 更新 README 里的 `rg` 对比：cache v18 为降低内存不再常驻完整文件正文，所以未限定范围的大 regex 会按需读源码，可能比 `rg` 慢；path-scoped regex、符号搜索、引用、依赖、outline 和 bundle 仍保持低延迟。
- 验证移除常驻完整文件源码正文后的按需读文件工具：`codedb_search PoolManager`、基于定义锚点的 `codedb_callers PoolManager`、`codedb_read PoolManager.cs`。

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
