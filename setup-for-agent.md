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
# - JavaScript/TypeScript: js, jsx, mjs, cjs, ts, tsx
# - C/C++: c, h, cc, cpp, cxx, hpp, hh, hxx
# Humans can remove or add extensions here before indexing.
extensions = ["cs", "java", "rs", "py", "pyw", "js", "jsx", "mjs", "cjs", "ts", "tsx", "c", "h", "cc", "cpp", "cxx", "hpp", "hh", "hxx"]

# Skip extremely large generated files before parsing or embedding.
max_file_bytes = 50000000

# Respect .gitignore files under this project for the normal tree walk. Nested
# Git worktrees/submodules under the target root are still scanned as source
# directories; .git/info/exclude and global gitignore are not project boundaries.
respect_gitignore = true

# Extra paths to include even when the normal scan would ignore them.
# Unity projects often need Library/PackageCache while skipping the rest of Library.
include_paths = ["Library/PackageCache"]

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
# MCP mode watches configured extensions and rebuilds after a short debounce.
enabled = true

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

5. Ask the human whether this specific agent should register the MCP server.

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
