---
name: codedb-mcp
description: >-
  Use codedb MCP tools for codebase lookup, feature-flow analysis, source
  evidence, references, dependencies, and semantic or exact code search in
  repositories with .codedb-mcp. Prefer codedb_bundle to combine lookups and
  avoid shell source search. For broad feature analysis, use twelve to eighteen
  codedb MCP calls total, then answer. Never use shell commands to search,
  print, grep, line-number, or verify source code when codedb tools are
  available.
---

# codedb-mcp

## Feature Analysis Limit

For prompts such as "main logic", "analyze this feature", "flow", "architecture", or "how does this module work", the default workflow is complete flow analysis with bounded output:

- Use twelve to eighteen codedb MCP tool invocations total for a normal feature-flow answer.
- Prefer `codedb_bundle` calls: discovery, focus, evidence, connection checks, boundary/consumer checks, final gap check.
- After the eighteenth codedb call, answer immediately from current evidence.
- More than eighteen codedb calls for a broad feature analysis is a failure unless the user explicitly asks for an exhaustive audit.
- If results include backup, generated, legacy, or irrelevant files, filter them mentally from the current result. Do not spend extra calls just to exclude them.
- Do not chase secondary branches after the main entry points, data flow, processing path, output/effect path, and compatibility layer are clear.
- For feature analysis, infer the project-appropriate flow and explicitly connect trigger/input, external boundary, state/data, processing/update logic, output or side effects, and consumers when they exist.

## Hard Rules

- Use codedb tools as the only source-code lookup path.
- Do not use shell commands for source search, source printing, line numbering, or source verification.
- Do not use `rg`, `grep`, `findstr`, `Select-String`, `Get-Content`, `type`, `cat`, `sed`, or scripts to inspect source files.
- Do not suggest `rg` or shell lookup as a cross-check, fallback,补漏, or verification method.
- Do not describe the workflow as "local file search" or "local code search"; describe it as codedb MCP lookup only.
- Shell commands are only for non-source operations such as checking process state, config paths, or generated artifacts.
- Use `codedb_bundle` when more than one codedb lookup is needed.
- Do not call `codedb_projects` in normal repository work; the active repo is already implied by the MCP session.

## Bundle Format

Use this exact `codedb_bundle` shape. Child arguments go under `arguments`, not `args`:

```json
{
  "ops": [
    {
      "tool": "codedb_context",
      "arguments": {
        "query": "feature words",
        "max_files": 8,
        "max_results": 30
      }
    },
    {
      "tool": "codedb_text_search",
      "arguments": {
        "query": "ExactName",
        "compact": true,
        "max_results": 15
      }
    }
  ],
  "timing": true
}
```

If a bundle returns "missing query argument", retry the same bundle once with `arguments`. Do not fall back to many separate tool calls.

## Feature Analysis Workflow

Use this workflow for broad feature analysis:

1. Discovery bundle: one `codedb_bundle` containing `codedb_context` plus at most two compact `codedb_text_search` or `codedb_search` calls.
2. Focus bundle: one `codedb_bundle` containing one `codedb_explore` with `max_chars <= 14000`, plus at most one compact search if a key name is missing.
3. Evidence bundle: one `codedb_bundle` containing at most five `codedb_read` ranges. Each read must use `compact=true`, `line_start`, and `line_end`. Keep each range to 90 lines or less and the total evidence reads to 420 lines or less.
4. Connection bundle: one `codedb_bundle` for `codedb_callers`, `codedb_deps`, `codedb_outline`, or compact searches that connect entry points to processing, state, output, or consumer paths.
5. Boundary/consumer bundle: optional one `codedb_bundle` with compact searches or short reads for project-specific boundaries such as API, protocol, CLI, event, job, UI, test, or integration code.
6. State/effect bundle: optional one `codedb_bundle` with compact searches or short reads for storage, cache, indexing, rendering, background work, file IO, network IO, or other side effects.
7. Gap bundle: optional one small `codedb_bundle` with at most two reads or searches for a missing critical link.

After these 12-18 codedb tool calls, stop looking up code and answer from the evidence. Do not keep expanding every type, branch, message, or file.

If you are missing a minor detail after the budget is spent, state the uncertainty from the current codedb evidence. Do not switch to shell lookup.

## Output Control

- Keep `codedb_context` to `max_files <= 8` and `max_results <= 30` for broad feature analysis.
- Keep broad `codedb_text_search` calls compact with `max_results <= 15`.
- Use `codedb_explore` before reads; it is the normal way to get a focused snippet set.
- Use `codedb_read` only for exact evidence after candidate files and line ranges are known.
- Never read a whole source file for a feature-analysis task.
- Never read one large file in many chunks for a broad analysis task. Use `codedb_outline` or the current evidence and summarize the main path.
- Never run repeated direct `codedb_read` calls after the evidence bundle. If more evidence is truly needed, use one small bundle with no more than two ranges.
- Prefer complete flow understanding over extreme token cutting. The target is roughly half of an unconstrained search session, not the smallest possible output.

## Tool Choices

- `codedb_bundle`: combine multi-step lookup and reduce tool-call overhead.
- `codedb_context`: first choice for feature, flow, onboarding, and architecture questions.
- `codedb_explore`: focused source context after discovery; prefer it over many reads.
- `codedb_text_search`: exact text or regex search; use compact output and low result limits.
- `codedb_search`: semantic or fuzzy concept search.
- `codedb_outline`: symbols in a known file before reading source.
- `codedb_read`: short line-scoped source evidence only.
- `codedb_callers`: references/callers; pass `definition_path` and `definition_line` when known.
- `codedb_deps`: file dependencies and reverse dependencies.
- `codedb_find`, `codedb_query`, `codedb_glob`, `codedb_ls`: compact navigation.
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
