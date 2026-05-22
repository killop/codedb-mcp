# DeepWiki Local Workflow

## Discovery

1. Call `codedb_status` and note language mix, file count, graph size, and storage dir.
2. Use `codedb_ls`/`codedb_glob` to map major source roots.
3. Use `codedb_analyze` and `codedb_graph format=summary` to find high-degree files, relation types, and likely hubs.
4. Use `codedb_search` for domain vocabulary, feature names, UI terms, protocol names, and manager/service names.

## Business Module Planning

Business modules should be based on cohesive behavior and dependency evidence:

- same user-facing feature or domain vocabulary;
- repeated calls among files even when they live in different folders;
- shared entry points, managers, models, generated protocol code, and UI panels;
- meaningful reverse dependencies from other systems.

Do not accept folder communities blindly. Merge split folders when dependency evidence says they are one module. Split large folders when they contain unrelated business flows.

## Page Generation

For each business module:

1. Gather candidate files with `codedb_search`, `codedb_find`, `codedb_glob`, and graph hints.
2. Use `codedb_deps` on the likely entry points.
3. Use `codedb_callers` for central managers, controllers, APIs, or event handlers.
4. Use `codedb_outline` before `codedb_read`.
5. Write responsibility, entry points, main flows, dependencies, extension points, and risks.

For infrastructure pages:

- keep the page short;
- explain how business modules use it;
- cite key APIs and extension points;
- avoid turning utility folders into oversized narrative pages.

## Quality Checks

- Every important page should cite concrete files.
- Cross-folder modules should explain why the files belong together.
- Avoid claiming runtime behavior from names alone.
- Prefer uncertainty notes over overconfident architecture claims.
