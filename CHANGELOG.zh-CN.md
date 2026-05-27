# 更新日志

[English version](CHANGELOG.md)

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
- 记录 C#、Java、Rust、Python、TypeScript、C、C++ 路径的多语言 smoke 覆盖。
- 记录 warm MCP 工具耗时：`codedb_search`、`codedb_callers`、`codedb_deps`、`codedb_outline`、`codedb_find`、`codedb_query`、`codedb_analyze`、`codedb_bundle`。
- 在 `u3dclient` 上验证 `code-module-atlas`，生成 16,361 个文件节点、62,771 条依赖边和 1,374 个模块。

### 打包

- 将以下 skills 打包为可独立复制的目录：
  - `skills/codedb-mcp`
  - `skills/deepwiki`
  - `skills/code-module-atlas`
- 确保项目特定的 atlas 生成文件、Vite 构建输出和 `node_modules` 都保持 ignored，不进入仓库提交。
