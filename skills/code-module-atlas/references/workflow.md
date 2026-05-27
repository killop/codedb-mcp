# Code Module Atlas Workflow

The atlas viewer is a packaged copy of the meet-blog-style 3D graph UI adapted for codedb data.

## Data Pipeline

1. `codedb_module_atlas` writes:
   - `<repo>/.codedb-mcp/module-atlas-data.json`
   - `<repo>/.codedb-mcp/module-atlas-points.json`
2. `scripts/build-module-atlas.mjs` copies those two files into:
   - `skills/code-module-atlas/assets/viewer/public/module-atlas-data.json`
   - `skills/code-module-atlas/assets/viewer/public/module-atlas-points.json`
3. The viewer script `assets/viewer/scripts/build-meet-dataset.mjs` converts file points into:
   - `assets/viewer/public/data/dataset.json`
4. `assets/viewer/scripts/prepare-meet-assets.mjs` patches the vendored frontend to:
   - load local `./data/dataset.json`
   - disable the original login UI
   - show module and file lists
   - filter graph nodes by selected module
   - focus a selected file

## Common Commands

Generate data only:

```powershell
node skills/code-module-atlas/scripts/build-module-atlas.mjs <repo-root>
```

Generate and serve:

```powershell
node skills/code-module-atlas/scripts/build-module-atlas.mjs <repo-root> --serve --port 5174
```

Serve existing prepared data:

```powershell
cd skills/code-module-atlas/assets/viewer
npm run dev -- --port 5174 --strictPort
```

## Notes

- `npm install` runs automatically inside `assets/viewer` if `node_modules` is missing.
- The script expects the sibling `codedb-mcp` skill to exist at `skills/codedb-mcp`.
- Use `--codedb-exe <path>` if the executable is elsewhere.
- Use `--config <path>` if the target repository config is not at `.codedb-mcp/codedb-mcp.toml`.
