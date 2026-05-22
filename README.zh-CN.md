# codebase-mcp

[English README](README.md)

`codebase-mcp` 是一个 Rust 实现的本地 MCP Server，提供兼容 `codedb_*` 的工具接口，用于在大型代码库上做代码搜索、符号大纲、引用查找、依赖分析、图分析和本地 DeepWiki 生成。

核心目标是：数据本地化、配置显式化、索引可复用、工具响应快，并且可以作为独立 skill 复制到其他环境中使用。

## 主要能力

- 通过 MCP 暴露 `codedb_search`、`codedb_callers`、`codedb_deps`、`codedb_outline`、`codedb_find`、`codedb_query`、`codedb_bundle`、`codedb_graph`、`codedb_communities` 等工具。
- 所有配置都来自目标项目内的 `.codedb-mcp/codedb-mcp.toml`，不依赖环境变量切换行为。
- 所有生成数据都放在目标项目的 `.codedb-mcp` 目录下；删除该目录即可清理本地索引、缓存、Louvain 缓存和 DeepWiki 输出。
- 使用统一 tree-sitter 解析层支持 C#、Java、Python、JavaScript、TypeScript/TSX、C、C++。
- C#/Java 的 typed callers 和 deps 额外实现 namespace/package import、qualified name、using alias、static using、annotation、attribute suffix 等规则，准确性最强。
- 使用 Minish 生态的 `model2vec-rs` 和 `minishlab/potion-code-16M` 生成本地文件级向量。
- 使用 BM25、精确 identifier 倒排索引、Vicinity HNSW 向量索引做混合搜索。
- 构建 graphify 风格代码图，并对 `codedb_communities` 懒计算 Louvain community。
- MCP 模式下监听配置内的源码扩展，文件改动后 debounce 并后台重建索引。

## 技术架构

1. **显式配置层**：读取 `.codedb-mcp/codedb-mcp.toml`，配置扫描扩展、文件大小上限、gitignore 行为、include paths、skip dirs、embedding 模型、watch 和 storage。
2. **本地存储层**：索引 payload、manifest、Louvain cache、DeepWiki 证据和文档都写入 `.codedb-mcp`。数据跟随项目目录，不写全局数据库。
3. **扫描层**：基于配置遍历代码库。Unity 项目中可以跳过大部分 `Library`，同时显式包含 `Library/PackageCache`。
4. **语言解析层**：所有语言统一走 tree-sitter grammar，输出同一套 `FileEntry` 和 `Symbol` 结构。解析时只遍历声明层，避免大型方法体拖慢索引。
5. **代码语义增强层**：C#/Java 上继续做 namespace/package import、别名、静态 using、注解、属性后缀、限定名引用等轻量语义推断。
6. **搜索索引层**：构建 chunks、identifier word hits、symbol definition chunks、BM25、Model2Vec embeddings、Vicinity HNSW。
7. **依赖与图层**：构建文件、namespace/package、symbol、dependency、reference 等节点和边；Louvain community 懒加载并缓存。
8. **MCP 工具层**：工具运行在 warm in-process index 上，支持 batch 和 bundle，减少 MCP 往返成本。
9. **Skill 打包层**：`skills/codedb-mcp` 内包含最新 `codebase-mcp.exe`、setup 脚本、配置模板和 MCP 安装说明；复制目录即可使用。

## 配置

默认配置路径：

```text
<repo-root>/.codedb-mcp/codedb-mcp.toml
```

关键配置：

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

`include_paths` 会覆盖被跳过的父目录，例如 Unity 项目中可以跳过 `Library`，但保留 `Library/PackageCache`。

## 构建

```powershell
cargo build --release
```

构建产物：

```text
target/release/codebase-mcp.exe
```

当前最新 exe 已同步到：

```text
skills/codedb-mcp/assets/codebase-mcp.exe
```

## 安装到目标项目

使用 skill 的 setup 脚本初始化目标项目：

```powershell
powershell -ExecutionPolicy Bypass -File skills\codedb-mcp\scripts\setup.ps1 -ProjectRoot u3dclient
```

脚本会：

- 创建 `<repo-root>/.codedb-mcp`
- 写入 `<repo-root>/.codedb-mcp/codedb-mcp.toml`
- 打印 MCP 注册命令
- 迁移旧 `.codebase-mcp` 数据到 `.codedb-mcp`
- 将旧 `codebase-mcp.toml` 改名为 `codedb-mcp.toml`

脚本不会自动修改全局 MCP 配置。MCP 注册应由 agent 或用户显式完成。

## MCP 启动

```powershell
target\release\codebase-mcp.exe --config u3dclient\.codedb-mcp\codedb-mcp.toml mcp u3dclient
```

CLI 快速检查：

```powershell
target\release\codebase-mcp.exe --config u3dclient\.codedb-mcp\codedb-mcp.toml index u3dclient
target\release\codebase-mcp.exe --config u3dclient\.codedb-mcp\codedb-mcp.toml --root u3dclient tool codedb_status "{}"
```

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
| `codedb_analyze` | 图统计、top nodes、关系计数、建议问题 |
| `codedb_hot` | 最近修改的索引文件 |
| `codedb_status` | 索引健康状态和统计 |

## Benchmark

测试环境中的大型 Unity 项目：

```text
u3dclient
```

当前 C#/Java 配置索引状态：

- 18,975 indexed files
- 129,165 chunks
- 275,878 symbols
- 295,753 graph nodes
- 688,566 graph edges
- Model2Vec `minishlab/potion-code-16M`
- Vicinity HNSW file vectors

索引耗时：

| 场景 | Cache | 耗时 |
|---|---|---:|
| tree-sitter cold build | miss | 44.876s |
| 不变文件再次打开 | hit | 39.546s |
| 单次 CLI `codedb_status` | hit | 39.4s |

Java 工程：

```text
gameserver
```

| 场景 | Files | Chunks | Symbols | 耗时 |
|---|---:|---:|---:|---:|
| cold build | 6,940 | 55,057 | 245,238 | 16.919s |
| cache hit | 6,940 | 55,057 | 245,238 | 11.527s |

warm MCP 工具耗时，不包含 MCP 进程启动和 index load：

| 场景 | Tool | 准确性 | avg | p95 |
|---|---|---:|---:|---:|
| scoped `PoolManager` exact | `codedb_search regex=true` | MCP 52 = rg 52 | 5.813ms | 5.953ms |
| scoped `Joystick` exact | `codedb_search regex=true` | MCP 46 = rg 46 | 6.371ms | 6.853ms |
| scoped `NetworkListenerManager` exact | `codedb_search regex=true` | MCP 14 = rg 14 | 6.486ms | 6.707ms |
| `PoolManager` refs | `codedb_callers` | 7 refs | 4.518ms | 5.464ms |
| `Joystick` refs | `codedb_callers` | 7 refs | 7.726ms | 8.692ms |
| `GameObjectPoolMgr.cs depends_on` | `codedb_deps` | 7 files | 0.244ms | 0.318ms |
| `NetworkListenerManager.cs imported_by` | `codedb_deps` | 3 files | 0.193ms | 0.212ms |
| `NetworkListenerManager.cs` path lookup | `codedb_find` | top1 correct | 20.259ms | 21.108ms |
| `find -> outline` | `codedb_query` | expected outline | 20.173ms | 20.505ms |

## codedb-mcp Skill

`skills/codedb-mcp` 可以作为独立目录复制到其他机器或项目中。目录包含：

- `assets/codebase-mcp.exe`
- `assets/codedb-mcp.toml.template`
- `scripts/setup.ps1`
- `references/mcp-install.md`
- `references/tools.md`
- `SKILL.md`

安装 MCP 时，脚本只打印命令，不自动写入全局 MCP 配置。这是为了让 agent 显式安装，避免隐藏副作用。

## deepwiki Skill

`skills/deepwiki` 用本地 `codedb_*` 工具和当前 agent 的推理能力生成 DeepWiki-style 文档，不配置独立大模型 API。

生成目录默认是：

```text
<repo-root>/.codedb-mcp/deepwiki
```

设计重点是业务模块优先：基础设施页保持简单，业务模块页更详细，并通过 `codedb_search`、`codedb_deps`、`codedb_callers`、`codedb_graph` 提供代码证据。

## 发布说明

- 推荐发布时带上 `skills/codedb-mcp`，这样用户复制 skill 后即可初始化项目和注册 MCP。
- `.codedb-mcp/index.bin`、`.codedb-mcp/manifest.json`、`.codedb-mcp/*.bin` 是项目本地生成物，不建议提交。
- 旧 `.codebase-mcp` 名称已经迁移为 `.codedb-mcp`；配置文件名也统一为 `codedb-mcp.toml`。
