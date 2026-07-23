# codebase-mcp setup for an agent

Use this guide to prepare one repository for the packaged `codedb-mcp` server.
The setup is local and explicit: project configuration and generated indexes
stay under `<repo-root>/.codedb-mcp`. No model download or external model API is
required.

The `codedb-mcp` skill explains tool usage after setup. This guide prepares the
project and leaves agent-specific MCP registration to the active agent and the
human.

## Inputs

- `<repo-root>`: absolute path of the repository to index.
- `<package-root>`: absolute path of this `codebase-mcp` package.
- Server executable:
  `<package-root>\skills\codedb-mcp\assets\codebase-mcp.exe`.

## 1. Create the project-local directory

```powershell
$repoRoot = (Resolve-Path '<repo-root>').Path
$packageRoot = (Resolve-Path '<package-root>').Path
$codedbDir = Join-Path $repoRoot '.codedb-mcp'
New-Item -ItemType Directory -Force -Path $codedbDir | Out-Null
```

## 2. Create the configuration

Copy the packaged template if the project does not already have a config:

```powershell
$template = Join-Path $packageRoot 'skills\codedb-mcp\assets\codedb-mcp.toml.template'
$config = Join-Path $codedbDir 'codedb-mcp.toml'
if (-not (Test-Path $config)) {
  Copy-Item -LiteralPath $template -Destination $config
}
Write-Host "codedb config: $config"
```

Review these scan settings before the first index:

- `extensions`: source extensions to parse.
- `root_paths`: optional source roots; empty scans the repository root.
- `include_paths`: extra roots that override ignored/skipped parents.
- `exclude_paths`: path globs such as `**/Editor/**`.
- `skip_dirs`: generated, dependency, build, and cache directories to skip.
- `max_file_bytes` and `respect_gitignore`.

The server supports C#, Java, Rust, Python, Lua, JavaScript/TypeScript, C, and
C++. Retrieval does not apply language-specific natural-language query rules.

## 3. Run an index check

```powershell
$exe = Join-Path $packageRoot 'skills\codedb-mcp\assets\codebase-mcp.exe'
& $exe --config $config index $repoRoot
if ($LASTEXITCODE -ne 0) {
  throw "codebase-mcp index check failed with exit code $LASTEXITCODE"
}
```

The first run creates a cache-v28 index and graph sidecars under
`<repo-root>/.codedb-mcp`. Old cache generations are ignored when their version
or scan signature does not match.

## 4. Check status

```powershell
& $exe --config $config --root $repoRoot tool codedb_status '{}'
```

A healthy status reports files, outlines, chunks, graph nodes/edges/communities,
the graph-atlas retrieval mode, configured extensions, cache state, and storage
directory.

## 5. Register MCP only after confirmation

Ask the human whether this specific agent should register the server. Do not
silently edit global MCP settings.

Generic command:

```text
<package-root>\skills\codedb-mcp\assets\codebase-mcp.exe --config <repo-root>\.codedb-mcp\codedb-mcp.toml mcp <repo-root>
```

See `skills/codedb-mcp/references/mcp-install.md` for a Codex-style registration
shape. After registration, restart or reload the MCP session and call
`codedb_status`.

## Runtime behavior

- `[watch] enabled = true` keeps the index current through batched filesystem
  events.
- Scan-scope config changes trigger a background full reindex while the old
  index remains available.
- Cache commits are manifest-last so interrupted writes do not replace a valid
  previous generation.
- Exact text, word, caller, dependency, and graph sidecars are loaded or rebuilt
  on demand.
- Delete `<repo-root>/.codedb-mcp` only when the human explicitly wants to remove
  all project-local generated data.
