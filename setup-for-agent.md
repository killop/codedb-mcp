# setup-for-agent

This file is for agents installing `codedb-mcp` into a target repository.

Do not treat the `codedb-mcp` skill as the installer. The skill explains how to use the MCP tools after the server is already configured. This setup guide prepares project-local files, prefers an existing default HuggingFace cache when present, falls back to a second-drive cache when it is not present, and then leaves agent-specific MCP registration to the active agent.

## Inputs

- `<repo-root>`: target codebase root.
- `<codedb-package-root>`: this repository or a copied standalone package that contains `skills/codedb-mcp/assets/codebase-mcp.exe`.
- `<model-dir>`: the final absolute model directory written to config. On Windows, if the default HuggingFace hub cache exists, use it. If it does not exist, choose the second available drive by sorted drive letter. For C/D/E drives, use `D:\codedb-mcp-cache\models\potion-code-16M`; for C/E/F drives, use `E:\codedb-mcp-cache\models\potion-code-16M`.

The config must contain the resolved absolute model path, not a model ID and not a relative path.

## Setup Steps

1. Create the project-local codedb directory and choose a model directory:

```powershell
New-Item -ItemType Directory -Force -Path "<repo-root>\.codedb-mcp" | Out-Null

function Test-CodedbModelLayout([string]$Path) {
  return (
    (Test-Path -LiteralPath (Join-Path $Path "tokenizer.json")) -and
    (Test-Path -LiteralPath (Join-Path $Path "model.safetensors")) -and
    (
      (Test-Path -LiteralPath (Join-Path $Path "config.json")) -or
      (Test-Path -LiteralPath (Join-Path $Path "config_sentence_transformers.json"))
    )
  )
}

function Get-DefaultPotionSnapshot([string]$Hub) {
  $repo = Join-Path $Hub "models--minishlab--potion-code-16M"
  $refsMain = Join-Path $repo "refs\main"
  if (Test-Path -LiteralPath $refsMain) {
    $commit = (Get-Content -LiteralPath $refsMain -Raw).Trim()
    $snapshot = Join-Path $repo "snapshots\$commit"
    if (Test-CodedbModelLayout $snapshot) {
      return $snapshot
    }
  }
  $snapshots = Join-Path $repo "snapshots"
  if (Test-Path -LiteralPath $snapshots) {
    $candidate = Get-ChildItem -LiteralPath $snapshots -Directory |
      Sort-Object Name |
      Where-Object { Test-CodedbModelLayout $_.FullName } |
      Select-Object -First 1
    if ($candidate) {
      return $candidate.FullName
    }
  }
  return $null
}

$defaultHfHome = Join-Path $env:USERPROFILE ".cache\huggingface"
$defaultHub = Join-Path $defaultHfHome "hub"
$existingSnapshot = if (Test-Path -LiteralPath $defaultHub) { Get-DefaultPotionSnapshot $defaultHub } else { $null }

if ($existingSnapshot) {
  $modelDir = $existingSnapshot
  $hfHome = $defaultHfHome
  $downloadModel = $false
} elseif (Test-Path -LiteralPath $defaultHub) {
  $modelDir = Join-Path $defaultHub "codedb-mcp\models\potion-code-16M"
  $hfHome = $defaultHfHome
  $downloadModel = $true
} else {
  $drives = @(Get-PSDrive -PSProvider FileSystem |
    Where-Object { $_.Root -match '^[A-Z]:\\$' -and (Test-Path -LiteralPath $_.Root) } |
    Sort-Object Name)
  if ($drives.Count -eq 0) {
    throw "No filesystem drive found for codedb model cache."
  }
  $modelDrive = if ($drives.Count -gt 1) { $drives[1].Root } else { $drives[0].Root }
  $modelDir = Join-Path $modelDrive "codedb-mcp-cache\models\potion-code-16M"
  $hfHome = Join-Path $modelDrive "codedb-mcp-cache\huggingface"
  $downloadModel = $true
}

if ($downloadModel) {
  New-Item -ItemType Directory -Force -Path $modelDir | Out-Null
}
New-Item -ItemType Directory -Force -Path $hfHome | Out-Null
$modelForToml = $modelDir.Replace('\', '/')
Write-Host "Selected codedb model directory: $modelForToml"
```

2. Download `minishlab/potion-code-16M` only when the selected directory does not already contain a valid default-cache snapshot:

```powershell
if ($downloadModel) {
  $env:HF_HOME = $hfHome
  $env:CODEDB_MODEL_DIR = $modelDir
@'
import os
from huggingface_hub import snapshot_download
snapshot_download(
    repo_id="minishlab/potion-code-16M",
    local_dir=os.environ["CODEDB_MODEL_DIR"],
    local_dir_use_symlinks=False,
)
'@ | python -
} else {
  Write-Host "Using existing default HuggingFace cache snapshot: $modelForToml"
}
```

If Python or `huggingface_hub` is not installed, use any available agent-safe download method that creates this final directory:

```text
<absolute-model-dir-selected-in-step-1>
```

The directory must contain the Model2Vec files expected by `model2vec-rs`, such as model config, tokenizer files, and safetensors files.

3. Generate `<repo-root>\.codedb-mcp\codedb-mcp.toml`.

Use this demo configuration as the starting point. Humans can edit it before first indexing.
When writing the actual file, set `[embedding].model` to `$modelForToml` from step 1. The `C:/Users/...` value below is an example for a machine where the default HuggingFace cache exists.

```toml
# codedb-mcp project configuration.
# Keep this file inside the project root under .codedb-mcp so the index,
# cache and MCP behavior travel with the codebase. The model path below must be
# the selected absolute model directory.

[scan]
# Current indexed languages:
# - C#: cs
# - Java: java
# - Rust: rs
# - Python: py, pyw
# - Lua: lua
# - JavaScript/TypeScript: js, jsx, mjs, cjs, ts, tsx
# - C/C++: c, h, cc, cpp, cxx, hpp, hh, hxx
# Humans can remove or add extensions here before indexing.
extensions = ["cs", "java", "rs", "py", "pyw", "lua", "js", "jsx", "mjs", "cjs", "ts", "tsx", "c", "h", "cc", "cpp", "cxx", "hpp", "hh", "hxx"]

# Skip extremely large generated files before parsing or embedding.
max_file_bytes = 50000000

# Respect .gitignore files under this project for the normal tree walk. Nested
# Git worktrees/submodules under the target root are still scanned as source
# directories; .git/info/exclude and global gitignore are not project boundaries.
respect_gitignore = true

# Optional scan roots relative to the project root. Empty means the whole root.
# Unity runtime C# example:
# root_paths = ["Assets", "Packages", "Library/PackageCache"]
root_paths = []

# Extra paths to include even when the normal scan would ignore them.
# Unity projects often need Library/PackageCache while skipping the rest of Library.
include_paths = ["Library/PackageCache"]

# Glob paths to exclude after root/include selection. Unity runtime-only example:
# exclude_paths = ["**/Editor", "**/Editor/**"]
exclude_paths = []

# Directory names to skip. A path listed in include_paths is still scanned even
# when one of its parent directories is skipped here.
skip_dirs = [
  ".git",
  ".hg",
  ".svn",
  ".vs",
  ".idea",
  ".gradle",
  "node_modules",
  "target",
  "dist",
  ".next",
  ".svelte-kit",
  "coverage",
  "out",
  ".codedb-mcp",
  "Library",
  "Temp",
  "Logs",
  "obj",
  "bin",
  "Build",
  "Builds",
  "UserSettings",
]

[embedding]
# Absolute Model2Vec model directory selected during setup.
model = "C:/Users/<user>/.cache/huggingface/hub/codedb-mcp/models/potion-code-16M"

[diagnostics]
# Set timing=true only while benchmarking; it writes stage timings to stderr.
timing = false

# Emit a slow-file parse log for files at or above this many ms. 0 disables it.
slow_file_ms = 0

[watch]
# MCP mode is enabled by default. Filesystem events are queued and applied as
# one live-incremental batch every poll_interval_seconds.
enabled = true
poll_interval_seconds = 5

[storage]
# Store generated data under the target project. Deleting this directory removes
# all local codedb-mcp data for that project.
enabled = true
dir = ".codedb-mcp"
```

4. Run a local index check:

```powershell
<codedb-package-root>\skills\codedb-mcp\assets\codebase-mcp.exe --config <repo-root>\.codedb-mcp\codedb-mcp.toml index <repo-root>
```

If this fails with a model load error, verify that `[embedding].model` points to the selected model directory and that the model files exist there.

5. Update `<repo-root>\AGENTS.md` so future agents actively use `codedb-mcp` for code lookup.

Read the existing file first and preserve its content. If the file does not exist, create it. Append the section below only when it is not already present:

```markdown
## 5. codedb-mcp 检索约定

- 当需要按自然语言语义、业务概念或模糊描述查找代码时，优先使用 `codedb_search`，不要先大范围读取源码树。
- 当需要精确文本、正则或字符串搜索时，优先使用 `codedb_text_search`；只有在验证原始文件系统结果、或搜索未纳入索引的文件时，才补充使用 `rg`。
- 当需要查找符号定义、文件大纲或读取局部代码上下文时，优先使用 `codedb_symbol`、`codedb_outline`、`codedb_read`，并用行号范围控制上下文大小。
- 当需要查找符号引用、调用方或“哪里用了这个类/方法”时，优先使用 `codedb_callers`；如果已知定义位置，传入 `definition_path` 和 `definition_line`。
- 当需要分析文件依赖、反向依赖或跨模块关系时，优先使用 `codedb_deps`。
- 当一次任务需要多个搜索、outline、read 或依赖查询时，优先使用 `codedb_bundle`、`codedb_query` 或工具自带的 batch 参数，减少 MCP 往返和 token 消耗。
- 当怀疑索引不新鲜或监听未生效时，先调用 `codedb_status`、`codedb_changes` 或 `codedb_hot` 检查状态。
```

PowerShell helper:

```powershell
$section = @'
## 5. codedb-mcp 检索约定

- 当需要按自然语言语义、业务概念或模糊描述查找代码时，优先使用 `codedb_search`，不要先大范围读取源码树。
- 当需要精确文本、正则或字符串搜索时，优先使用 `codedb_text_search`；只有在验证原始文件系统结果、或搜索未纳入索引的文件时，才补充使用 `rg`。
- 当需要查找符号定义、文件大纲或读取局部代码上下文时，优先使用 `codedb_symbol`、`codedb_outline`、`codedb_read`，并用行号范围控制上下文大小。
- 当需要查找符号引用、调用方或“哪里用了这个类/方法”时，优先使用 `codedb_callers`；如果已知定义位置，传入 `definition_path` 和 `definition_line`。
- 当需要分析文件依赖、反向依赖或跨模块关系时，优先使用 `codedb_deps`。
- 当一次任务需要多个搜索、outline、read 或依赖查询时，优先使用 `codedb_bundle`、`codedb_query` 或工具自带的 batch 参数，减少 MCP 往返和 token 消耗。
- 当怀疑索引不新鲜或监听未生效时，先调用 `codedb_status`、`codedb_changes` 或 `codedb_hot` 检查状态。
'@
$path = Join-Path "<repo-root>" "AGENTS.md"
if (Test-Path -LiteralPath $path) {
  $content = Get-Content -LiteralPath $path -Raw
  if ($content -notmatch '##\s*(\d+\.\s*)?codedb-mcp 检索约定') {
    Add-Content -LiteralPath $path -Value "`r`n$section`r`n"
  }
} else {
  Set-Content -LiteralPath $path -Value "# AGENTS.md`r`n`r`n$section`r`n"
}
```

6. Ask the human whether this specific agent should register the MCP server.

Do not silently edit agent-wide MCP settings. After the human agrees, configure the current agent using its own MCP mechanism. The command shape is:

```text
<codedb-package-root>\skills\codedb-mcp\assets\codebase-mcp.exe --config <repo-root>\.codedb-mcp\codedb-mcp.toml mcp <repo-root>
```

For Codex-style TOML, the shape is:

```toml
[mcp_servers.codedb-mcp]
command = "<codedb-package-root>\\skills\\codedb-mcp\\assets\\codebase-mcp.exe"
args = [
  "--config",
  "<repo-root>\\.codedb-mcp\\codedb-mcp.toml",
  "mcp",
  "<repo-root>",
]
```

After registration, restart or reload the agent MCP session and call `codedb_status`.
