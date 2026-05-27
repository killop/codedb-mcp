---
name: code-module-atlas
description: Generate and serve a local 3D code module/file atlas webpage from a repository by calling the existing codedb-mcp codedb_module_atlas tool, then converting the result into the bundled meet-blog-style viewer. Use when the user wants a visual module/file graph, module list, file list, or publishable local atlas page.
---

# code-module-atlas

Use this skill to create a local web page that visualizes a codebase as a 3D file dependency atlas grouped by the modules detected by `codedb-mcp`.

## Rules

- Do not reimplement indexing or module detection in this skill.
- Call `codedb-mcp` through `skills/codedb-mcp/assets/codebase-mcp.exe tool codedb_module_atlas`.
- Keep generated atlas data in the target repo's `.codedb-mcp` directory first, then copy it into the bundled viewer.
- The viewer source lives in `assets/viewer`; it is self-contained under this skill.
- Do not commit project-specific generated files from `assets/viewer/public/module-atlas-data.json`, `assets/viewer/public/module-atlas-points.json`, or `assets/viewer/public/data/dataset.json`.

## Workflow

1. Confirm the target repo already has `.codedb-mcp/codedb-mcp.toml`.
2. If the repo is not configured, use the package-level `setup-for-agent.md` or the `codedb-mcp` skill first.
3. Generate and prepare the atlas:

```powershell
node skills/code-module-atlas/scripts/build-module-atlas.mjs <repo-root>
```

4. Start the viewer:

```powershell
cd skills/code-module-atlas/assets/viewer
npm run dev -- --port 5174 --strictPort
```

Or generate and serve in one foreground process:

```powershell
node skills/code-module-atlas/scripts/build-module-atlas.mjs <repo-root> --serve --port 5174
```

## Output

The page shows:

- one star node per code file
- file-to-file dependency edges
- a left module list generated from `codedb_module_atlas`
- a file list for the selected module
- module filtering: selecting a module redraws the graph with only that module's files
- file focusing: selecting a file flies to that file node and opens the detail panel

## Validation

After generation, verify at least:

- `assets/viewer/public/data/dataset.json` exists
- the page title is `codedb File Atlas`
- the left panel contains `模块列表` and `文件列表`
- selecting a module changes the stats from all files to that module's file count
- selecting a file opens the detail panel
