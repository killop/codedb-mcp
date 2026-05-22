---
name: deepwiki
description: Build or refresh a DeepWiki-style local repository wiki using the active agent plus local codedb-mcp tools, without configuring an external model API. Use when Codex needs business-module-first architecture docs, cited code references, dependency-aware page planning, or a .codedb-mcp/deepwiki documentation set for a local codebase.
---

# DeepWiki

## Principle

Use the active agent's reasoning and local `codedb_*` MCP tools. Do not introduce a separate LLM API configuration. The wiki should explain the repository as engineers understand it: business modules first, infrastructure pages smaller and simpler.

## Output

Write generated wiki files under the target repo's `.codedb-mcp/deepwiki` directory unless the user asks for another location. Keep citations as repo-relative file paths with line numbers where possible.

## Workflow

1. Ensure `codedb-mcp` is configured and healthy for the repo. If not, use the `codedb-mcp` skill first.
2. Call `codedb_status`, `codedb_tree` or `codedb_ls`, and `codedb_analyze` to understand size, languages, and graph shape.
3. Build an initial page plan from business/domain signals, not folder names alone. Use `codedb_search`, `codedb_find`, `codedb_deps`, `codedb_callers`, and `codedb_graph` to find cross-folder modules with real call/dependency cohesion.
4. Treat `codedb_communities` as a hint, not the source of truth. Split or merge modules based on code evidence and the agent's domain reasoning.
5. Use `codedb_bundle` for repeated outlines, reads, dependency checks, and searches.
6. Write concise pages with code citations: module responsibility, key entry points, main flows, dependencies, extension points, and risks.

## Page Shape

- `index.md`: repo overview, business module map, and high-value reading order.
- `business/<module>.md`: one page per business module. These should be the most detailed pages.
- `flows/<flow>.md`: cross-module runtime workflows when important.
- `infrastructure/<topic>.md`: build, framework, storage, networking, generated code, and utility layers. Keep these short unless they drive business behavior.
- `glossary.md`: domain terms, aliases, and important symbol names.

## Evidence Rules

- Prefer `codedb_callers` for "who uses this symbol" questions.
- Prefer `codedb_deps` and `codedb_graph` for module relationships.
- Prefer `codedb_outline` before reading full files.
- Prefer `codedb_search regex=true` plus `rg` only when validating exact raw text counts.
- Record uncertainty explicitly when code evidence is weak or a module boundary is inferred.

See `references/deepwiki-workflow.md` for a fuller generation checklist.
