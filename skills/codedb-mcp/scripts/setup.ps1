param(
    [Parameter(Mandatory = $false)]
    [string]$ProjectRoot = (Get-Location).Path,

    [Parameter(Mandatory = $false)]
    [switch]$Force
)

$ErrorActionPreference = "Stop"

$skillRoot = Resolve-Path (Join-Path $PSScriptRoot "..")
$project = Resolve-Path $ProjectRoot
$configDir = Join-Path $project ".codedb-mcp"
$legacyConfigDir = Join-Path $project ".codebase-mcp"
$template = Join-Path $skillRoot "assets\codedb-mcp.toml.template"
$config = Join-Path $configDir "codedb-mcp.toml"
$legacyConfig = Join-Path $configDir "codebase-mcp.toml"
$exe = Join-Path $skillRoot "assets\codebase-mcp.exe"

if (!(Test-Path -LiteralPath $template)) {
    throw "Missing config template: $template"
}

New-Item -ItemType Directory -Force -Path $configDir | Out-Null

if ((Test-Path -LiteralPath $legacyConfig) -and !(Test-Path -LiteralPath $config) -and !$Force) {
    Move-Item -LiteralPath $legacyConfig -Destination $config
    Write-Host "Migrated config: $legacyConfig -> $config"
}

if (!(Test-Path -LiteralPath $config) -or $Force) {
    Copy-Item -LiteralPath $template -Destination $config -Force
    Write-Host "Wrote config: $config"
} else {
    Write-Host "Config already exists: $config"
}

$configText = Get-Content -LiteralPath $config -Raw
$configText = $configText.Replace("codebase-mcp project configuration", "codedb-mcp project configuration")
$configText = $configText.Replace(".codedb-mcp/codebase-mcp.toml", ".codedb-mcp/codedb-mcp.toml")
$configText = $configText.Replace(".codedb-mcp\codebase-mcp.toml", ".codedb-mcp\codedb-mcp.toml")
$configText = ($configText -split "`r?`n" | Where-Object { $_ -notmatch '^\s*"\.codebase-mcp",\s*$' }) -join "`r`n"
Set-Content -LiteralPath $config -Value $configText

if (Test-Path -LiteralPath $legacyConfigDir) {
    $generatedFiles = @(
        "index.bin",
        "manifest.json",
        "louvain-communities.bin",
        "louvain-subcommunities.bin"
    )
    Get-ChildItem -LiteralPath $legacyConfigDir -Force | ForEach-Object {
        $target = Join-Path $configDir $_.Name
        if (!(Test-Path -LiteralPath $target)) {
            Move-Item -LiteralPath $_.FullName -Destination $target
            Write-Host "Migrated data: $($_.FullName) -> $target"
        } elseif (!$_.PSIsContainer -and $generatedFiles -contains $_.Name) {
            Remove-Item -LiteralPath $_.FullName
            Write-Host "Removed duplicate legacy generated data: $($_.FullName)"
        }
    }
    if (!(Get-ChildItem -LiteralPath $legacyConfigDir -Force)) {
        Remove-Item -LiteralPath $legacyConfigDir
        Write-Host "Removed empty legacy directory: $legacyConfigDir"
    } else {
        Write-Host "Legacy directory still has files that would overwrite .codedb-mcp: $legacyConfigDir"
    }
}

if (!(Test-Path -LiteralPath $exe)) {
    Write-Host "Warning: bundled executable not found: $exe"
}

Write-Host ""
Write-Host "Register this MCP server in the agent's MCP config:"
Write-Host "command = `"$exe`""
Write-Host "args = [`"--config`", `"$config`", `"mcp`", `"$project`"]"
Write-Host ""
Write-Host "The setup script does not install MCP automatically."
