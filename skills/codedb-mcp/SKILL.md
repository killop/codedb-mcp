---
name: codedb-mcp
description: >-
  Use codedb MCP tools for repository understanding, feature-flow analysis,
  source evidence, references, dependencies, semantic search, and exact text
  search in repositories with .codedb-mcp. Prefer codedb_bundle and compact
  codedb_context/codedb_text_search before any source reading. For broad
  "main logic", "how does this feature work", module, caller, dependency, or
  architecture questions, gather a small evidence map with codedb tools and
  answer from that evidence. Never use shell commands to search, print,
  line-number, or verify source code when codedb tools are available.
  Do not open skill files during normal repository analysis.
---

# codedb-mcp

## Core Rule

Use codedb tools as the only source-code lookup path.

- Do not use shell commands for source search, source printing, line numbering, or source verification.
- Do not use shell commands to inspect repository structure, list files, check current files, or confirm the active project during code analysis.
- Do not use `rg`, `grep`, `findstr`, `Select-String`, `Get-Content`, `type`, `cat`, `sed`, or scripts to inspect source files.
- Do not suggest shell lookup as a cross-check, fallback, gap fill, or verification method.
- Use `codedb_bundle` whenever more than one codedb lookup is needed.
- For feature-flow or "main logic" analysis, do not call direct `codedb_context`, `codedb_search`, `codedb_text_search`, `codedb_read`, `codedb_outline`, `codedb_callers`, or `codedb_deps`; put them inside `codedb_bundle`.
- Do not call `codedb_projects` in normal repository work; the active repo is already implied by the MCP session.
- Do not read this `SKILL.md` or reference files during normal analysis; these instructions are already active.

## Feature Analysis Guidance

For prompts such as "main logic", "analyze this feature", "flow", "architecture", or "how does this module work":

- Prefer `codedb_bundle` for multi-step lookup so related evidence arrives in coherent batches.
- Start with compact discovery and focused source evidence, then continue only when the answer still lacks a concrete link.
- Set `max_output_chars` and `max_child_chars` explicitly when a task needs output control; increase them when the current evidence is too thin.
- Prefer a complete main flow over exhaustive branch coverage.
- Connect the project-appropriate flow: trigger/input, external boundary, state/data, processing/update logic, output or side effects, and consumers when they exist.
- When the prompt names multiple actions, states, or slash-separated paths, preserve each named path as an explicit section in the final answer.
- If a minor detail is still missing after reasonable codedb evidence, state the uncertainty from current codedb evidence and answer.
- Batch related exact terms with one `codedb_text_search` child using `queries`, instead of separate children.
- Do not chase optional automation, compatibility, generated, or alternate UI/configuration branches after the main flow is clear. Mention them briefly as adjacent paths when discovered.

## Bundle Shape

Use this shape. Child arguments go under `arguments`, not `args`:

```json
{
  "timing": true,
  "max_output_chars": 18000,
  "max_child_chars": 6000,
  "ops": [
    {
      "tool": "codedb_context",
      "arguments": {
        "query": "feature words",
        "max_files": 6,
        "max_results": 24
      }
    },
    {
      "tool": "codedb_text_search",
      "arguments": {
        "queries": ["ExactName", "ProtocolName", "UICommandName"],
        "compact": true,
        "max_results": 8
      }
    }
  ]
}
```

If a bundle returns "missing query argument", retry the same bundle once with `arguments`. Do not replace it with many separate calls.

## Default Workflow

1. Discovery bundle: `codedb_context`, plus one batch `codedb_text_search` for exact domain terms or protocol/API names.
2. Focus bundle: one `codedb_explore` with an explicit `max_chars`; add compact search if a key symbol is missing.
3. Relationship bundle: `codedb_deps`, `codedb_callers`, `codedb_outline`, or compact searches that connect entry points to state, processing, output, and consumers.
4. Evidence bundle: use `codedb_read` ranges after candidate files and line ranges are known. Prefer `compact=true`; use `compact=false` only when exact source wording is needed.
5. Gap bundle: optional compact searches or short reads for a critical missing link.

Do not keep expanding every type, UI branch, message, enum, or compatibility layer after the main chain is clear.

## Output Control

- Use `codedb_context` for broad planning and ranking before reading source.
- Use `codedb_explore` for source snippets under a hard `max_chars` budget.
- Use broad `codedb_text_search` with `queries`, `compact=true`, and explicit `max_results`.
- Use direct `codedb_read` only after candidate files and line ranges are known.
- Prefer `codedb_read compact=true` for feature-flow analysis; use `compact=false` only when necessary for exact code wording.
- Never read a whole source file for a feature-analysis task.
- Never read one large file in many chunks for a broad analysis task.
- In the final answer, cite the important files and line starts, but do not dump code blocks unless the user asks.

## Tool Choices

- `codedb_bundle`: combine lookup steps and keep output budgeted.
- `codedb_context`: first choice for feature, flow, onboarding, and architecture questions.
- `codedb_explore`: focused source context after discovery.
- `codedb_text_search`: exact text or regex search inside the indexed corpus.
- `codedb_search`: semantic or fuzzy concept search.
- `codedb_outline`: symbols in a known file before reading source.
- `codedb_read`: short line-scoped source evidence only; supports `paths` batch.
- `codedb_callers`: references/callers; pass `definition_path` and `definition_line` when known.
- `codedb_deps`: file dependencies and reverse dependencies.
- `codedb_find`, `codedb_query`, `codedb_glob`, `codedb_ls`: compact navigation.
- `codedb_version`: server/package version check without loading a project index.
- `codedb_status`, `codedb_changes`, `codedb_hot`: freshness and scan-scope checks when results look stale.

## Setup Boundary

Keep generated config and index data under the target repo's `.codedb-mcp` directory. Do not install MCP from this skill. Use `setup-for-agent.md` when setup is needed, then ask the human before agent-specific MCP registration.

If MCP is already configured, the server command shape is:

```text
<skill-root>\assets\codebase-mcp.exe --config <repo-root>\.codedb-mcp\codedb-mcp.toml mcp <repo-root>
```

## Token Observation

When the human asks to inspect Codex token usage after running Codex in a target repository, run:

```powershell
node <skill-root>\scripts\codex-observe.mjs --project <repo-root> --since 24h --top 12
```

The observer streams Codex JSONL transcripts from `~/.codex/sessions`, filters by project `cwd`, and reports model tokens, tool-output token estimates, codedb calls, bundle child breakdowns, high-output calls, shell source lookup, and missed codedb opportunities.
