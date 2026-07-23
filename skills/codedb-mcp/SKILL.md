---
name: codedb-mcp
description: >-
  Use the configured codedb MCP server for generic repository understanding,
  source lookup, symbols, references, dependency graphs, exact code search, and
  call tracing in repositories with .codedb-mcp. Use it for lifecycle tracing,
  implementation discovery, architecture questions, and cross-file behavior.
---

# codedb-mcp

Use codedb as the source lookup path for indexed repositories. The server is
English-first. When the user's task is not English, translate the complete task
once into concise English before the first codedb call. Preserve identifiers,
paths, quoted literals, error messages, uncertainty, and code exactly.

The server does not interpret natural-language task text. `task` is an opaque
label. Repository selection comes from the persisted file graph, communities,
dependency/call edges, and exact evidence chosen by the agent.

## Workflow

1. Call `codedb_flow` with the complete task and no `path_glob` to get the
   compact graph atlas: source prefixes, focused groups, and dependency links.
2. In an atlas row written as `parent: child(count)`, choose the entry leaf
   `parent/child/**`, not the broad `parent/**`. Call `codedb_flow` again with
   the same complete task plus that leaf scope.
3. Read the scoped pack as a graph projection: structural roots, community
   boundaries, weighted bridges, dependency edges, call edges, traces, and
   active symbol bodies.
4. Follow exact returned paths and symbols with `codedb_symbol body=true
   max_results=1`, `codedb_callpath`, `codedb_callers`, or `codedb_deps`.
   Prefer a small compact `codedb_read` only when no exact symbol body fits.
   When both an exact entry and requested final endpoint are known, try one
   `codedb_callpath` first. A found path includes complete active bodies; fill
   only real gaps instead of manually expanding every intermediate helper.
   The default exact body also includes compact deterministic executable
   corridor paths. Traversal continues without a fixed depth until a real
   branch, terminal, or cycle. Use `codedb_callpath` when both endpoints are
   known; use `expand=true` only when a corridor identifies a needed branch
   whose bodies or deeper references are still missing.
5. Once an exact body is available, follow its qualified/tail/literal handoff
   across directories. Do not pre-project later atlas leaves. Repeat scoped
   `codedb_flow` only when the current evidence has no exact next path. There is
   no call quota; stop when the chain is supported.
6. Answer from the shortest complete evidence chain. State real gaps explicitly.

Same-file atomicity: when the base outline shows several answer-critical
members in one file, rerun that outline once with
`include_connected_ranges=true`, then read one returned joint range with
compact `codedb_read`. Do not request connected ranges preemptively for every
file, and do not turn one cohesive local component into sequential
`codedb_symbol` calls. Keep individual symbol bodies for isolated members and
cross-file handoffs. A returned `connected_range=true` read is a complete
same-file evidence closure: do not reopen its contained members or overlapping
ranges unless the active code contradicts another exact body.

Never invent lifecycle symbol names such as `OnInit`, `OnCreate`, `Ready`, or
`CheckReady`. Call `codedb_symbol` only with a name returned verbatim by a
previous codedb result. If one exact name is missing in a known file, call
`codedb_outline` for that file once and choose from its real symbols; do not try
more guessed siblings.

A lifecycle trace is complete when every requested phase has one exact active
body, every adjacent pair has a direct handoff or graph path, and the final
visible/readiness lifecycle callback has an exact body. Answer immediately at
that point. More calls are allowed, but evidence already satisfying this rule is
not improved by descending into generic framework internals.

Traverse phases once in execution order. Keep a small evidence ledger of
`phase -> active body -> next handoff`. After a phase is closed, do not return to
it for extra detail, constants, assets, compile variants, or additional callers
unless downstream evidence contradicts the established transition. Once the
final phase closes, draft the answer instead of starting a second verification
pass.

The final requested callback/readiness body is a hard stop. Do not inspect
upstream FSM update plumbing, later server readiness variants, callers,
compile-time alternatives, or another implementation after it unless the body
contradicts the established chain or the user explicitly requested that extra
branch.

## Evidence rules

- Treat `spine source` bodies as already read. Fetch the same body again only
  when a material line outside it is missing.
- `codedb_symbol body=true` already returns the complete active body. Do not add
  `format=full`, repeat the same body, or expand every referenced helper. Use
  `expand=true` only when the body and direct handoffs do not expose the next
  exact lifecycle link.
- Verify a claimed primary/current implementation with its active body and at
  least one active caller, handoff, dependency, or downstream consumer when the
  repository exposes one.
- A direct call in an active body already proves that handoff. Do not add a
  callers/search round merely to re-prove it. Use callers only when direction or
  runtime ownership is unresolved.
- Follow `body qualified tail call leads` before search/find. They resolve exact
  cross-file call tokens taken directly from the active body and are intended to
  expose the next lifecycle stage without keyword discovery.
- Read the `exact body evidence` card first. The listed handoffs are candidates,
  not a checklist: choose only the next requested phase still missing. The body
  already closes the current phase and direct calls already prove those links.
- Treat comments, disabled code, generated legacy sources, and deprecated code
  as historical evidence, not runtime behavior.
- Follow `outline literal bridge leads` and `body literal bridge leads` when
  inspected source contains a string-loaded resource, event, module, registry,
  prefab, or other non-call handoff.
- Preserve source order. Late completion and transition calls remain part of the
  chain even when large commented regions occur earlier in the body.
- Prefer traces corroborated by more than one selected structural root. A long
  branch from one root can be valid but tangential.
- Close the normal entry-to-readiness path before expanding failure, retry,
  reconnect, relaunch, editor, legacy, or alternate-state branches.
- Inspect one representative failure/retry body per answer-critical asynchronous
  boundary when the task asks for failures. Do not enumerate every generic
  resource, configuration, UI manager, or framework helper once its completion
  contract and next state are proven.
- For UI readiness, an exact UI-open call plus the target view's show/ready body
  is sufficient unless the question explicitly asks how the generic UI loader
  works. Do not descend through panel factories, generated view bindings, or
  base UI classes after that boundary is proven.
- For a state machine, registration/order plus each selected state's entry or
  completion body is sufficient. Do not inspect unrelated state helpers or
  editor/test entry points after the active runtime chain connects.
- One supported evidence path per adjacent lifecycle link is sufficient. Do not
  search the same call as text, inspect its callers, and read both endpoints
  when an active body already exposes the transition.
- Continue from exact returned paths, symbols, refs, callers, deps, outlines,
  traces, previews, or callpaths. Keep follow-ups atomic: consume each result
  before choosing the next exact body or graph edge.
- After the first `codedb_*` source lookup, use only `codedb_*` for repository
  source lookup in that answer.

## Query discipline

- Do not derive keyword bags, synonyms, facets, morphology, translations of
  identifiers, or language/framework/domain-specific boosts from the task.
- Do not use `codedb_search` for natural-language task text. Use it only for an
  exact identifier, path, quoted literal, or string already present in returned
  repository evidence.
- Do not treat atlas rows or discovery candidates as behavior without active
  body/read/dependency/caller/callpath evidence.
- Do not reconstruct a file through overlapping reads. Switch to exact symbols,
  dependencies, callers, or callpaths after one focused read.
- Do not use `compact=false` for broad analysis.

## Tool details

Read `references/tools.md` only when a tool argument or output contract is
unclear.

## Setup boundary

Generated config and index data stay under `.codedb-mcp`. This skill explains
usage only.

MCP command shape:

```text
<skill-root>\assets\codebase-mcp.exe --config <repo-root>\.codedb-mcp\codedb-mcp.toml mcp <repo-root>
```

## Token observation

When asked to inspect Codex token usage after a run:

```powershell
node <skill-root>\scripts\codex-observe.mjs --project <repo-root> --since 24h --top 12
```
