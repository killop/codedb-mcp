---
name: codedb-mcp
description: >-
  Use the configured codedb MCP server whenever an indexed repository needs
  lifecycle tracing, implementation discovery, call/dispatch analysis,
  shared-state producer lookup, branch evidence, architecture explanation, or
  exact source navigation. Prefer declarative graph queries over repeated
  wrapper reads or task-keyword search.
---

# codedb-mcp

Use codedb as the repository source path when `.codedb-mcp` is configured. The
server is English-first. Translate a non-English question once into concise
English, but preserve identifiers, paths, literals, errors, and code exactly.

The server does not route from task keywords. `task` is an opaque label.
Accuracy comes from exact graph facts and source bodies, not synonyms, language
boosts, query models, call quotas, or output clamps.

## Workflow

The MCP server is already bound to the repository.

1. Start with `codedb_graph_query`. If no exact anchor is known, query
   `EntryFile` and `BoundaryFile`; use `Community` nodes only to choose a
   structural region. Do not list every `File` to discover an entry.
2. Query cross-community `DEPENDS_ON` edges to identify structural bridges,
   then use the returned exact files/symbols as anchors. Do not turn the task
   wording into graph predicates.
3. Use one declarative pattern to retrieve each evidence chain instead of
   opening forwarding wrappers separately. Use `MATCH SHORTEST` for a finite
   connector between exact endpoints. Plain
   variable-length `MATCH` returns matching paths and is appropriate only when
   all such paths are actually required.
4. Read an exact `codedb_symbol body=true max_results=1` only for local behavior
   not already represented by graph facts. Use `codedb_read` for an exact range
   when no symbol body fits.
5. Answer from the shortest complete evidence chain. State a genuine missing
   dynamic edge explicitly; do not replace it with keyword search or a guessed
   lifecycle symbol.

## Graph query model

`codedb_graph_query` is an atomic, read-only Cypher-like subset:

- `MATCH`, optional `SHORTEST`, `WHERE`, `RETURN`, `ORDER BY`, optional
  `LIMIT`;
- scalar comparisons `=`, `!=`, `<`, `<=`, `>`, `>=`, plus
  property-to-property comparisons;
- node labels such as `Community`, `File`, `Symbol`, `SharedState`,
  `CallSite`, `Value`, `Parameter`, `Condition`, and `ControlAction`;
- directed typed edges and finite `*min..max` paths;
- deterministic projection, de-duplication, and output ordering.

The planner starts a path from the more selective endpoint. Write the natural
edge direction even for reverse lookup, for example
`(caller)-[:REFERENCES]->(exactTarget)`; an exact target `name/path` predicate
avoids a global caller scan.

Discovery properties:

- `Community`: `id`, `name`, `size`, `boundary_links`,
  `representative_path`;
- `File`: `path`, `community`, `degree`, `boundary_degree`,
  `incoming_degree`, `outgoing_degree`;
- `EntryFile` is a `File` with zero incoming and nonzero outgoing dependency
  degree; `BoundaryFile` crosses communities; `SinkFile` has incoming but no
  outgoing dependency;
- file `DEPENDS_ON.count` is `1`; community `DEPENDS_ON.file_edges` is the
  aggregated cross-community edge count.

Core relations:

- calls and dispatch: `CALLS`, `DISPATCHES_TO`;
- explicit call facts: `HAS_CALLSITE`, `TARGET`, `ARGUMENT`, `BINDS_TO`;
- control facts: `HAS_PARAMETER`, `USED_IN`, `TRUE`, `FALSE`, `PREVENTS`,
  `REACHES`;
- state/dependency facts: `READS`, `WRITES`, `CONTAINS`, `DEPENDS_ON`,
  `REFERENCES`.

Structural discovery without task keywords:

```cypher
MATCH (file:EntryFile)
RETURN file.path, file.community, file.outgoing_degree, file.boundary_degree
ORDER BY file.outgoing_degree DESC, file.boundary_degree DESC
```

```cypher
MATCH (community:Community)
RETURN community.id, community.size, community.boundary_links,
       community.representative_path
ORDER BY community.size DESC
```

```cypher
MATCH (community:Community)-[:CONTAINS]->(file:File)
WHERE community.id=7
RETURN file.path, file.degree, file.boundary_degree,
       file.incoming_degree, file.outgoing_degree
ORDER BY file.boundary_degree DESC, file.degree DESC
```

```cypher
MATCH (source:File)-[dependency:DEPENDS_ON]->(target:File)
WHERE source.community != target.community
RETURN source.path, source.community, dependency,
       target.path, target.community
ORDER BY source.boundary_degree DESC
```

Examples:

```cypher
MATCH SHORTEST p=(entry:Symbol)-[:CALLS|DISPATCHES_TO*1..8]->(leaf:Symbol)
WHERE entry.name='LoadReady' AND leaf.path='exact/path/HostImpl.cs'
RETURN p
```

```cypher
MATCH
  (caller:Symbol)-[:HAS_CALLSITE]->(call:CallSite),
  (call)-[argument:ARGUMENT]->(value:Value)-[:BINDS_TO]->(parameter:Parameter)
WHERE caller.name='LoadReady' AND call.name='GetDownloadListByFilterTags'
RETURN call.line, call.guard, value.index, value.expression, parameter.name
```

```cypher
MATCH (owner:Symbol)-[:HAS_CALLSITE]->(call:CallSite)-[:ARGUMENT]->(value:Value)
WHERE value.expression='EventDefine.OnInitEnd'
RETURN owner.name, owner.path, call.name, call.line, call.text, value.index
ORDER BY owner.path
```

```cypher
MATCH
  (leaf:Symbol)-[:HAS_PARAMETER]->(parameter:Parameter)-[use:USED_IN]->(condition:Condition),
  (condition)-[:TRUE]->(skip:ControlAction)-[:PREVENTS]->(append:CallSite),
  (condition)-[:FALSE]->(fallthrough:ControlAction)-[:REACHES]->(append)
WHERE leaf.path='exact/path/HostImpl.cs' AND parameter.name='tags'
RETURN use.via, condition.text, condition.negated, skip.kind, append.text
```

```cypher
MATCH
  (consumer:Symbol)-[read:READS]->(state:SharedState)<-[write:WRITES]-(producer:Symbol)
WHERE consumer.name='GetDownTaskData' AND state.name='DownloadTaskMap'
RETURN consumer, read, state, producer, write
```

## Evidence rules

- A `CALLS` edge proves a handoff, not the callee's selection, filtering,
  fallback, retry, or inclusion semantics.
- Preserve argument roles through `ARGUMENT -> Value -> BINDS_TO -> Parameter`.
  Do not call an input collection “selected” or “downloaded” until control facts
  or the exact leaf body establish the branch outcome.
- `HAS_CALLSITE` can retain a syntax-certain qualified call with
  `resolution='syntax'` when its callee is not uniquely resolvable. Its
  `ARGUMENT` facts are valid source evidence, but require `TARGET` before
  claiming an exact callee or using `BINDS_TO`.
- Keep interface implementations separate. `DISPATCHES_TO` lists possible
  implementations; construction/assignment evidence selects the active one.
- A preprocessor guard belongs only to the call edge carrying it. A sibling
  guarded call does not guard an independent caller.
- For `if (...) continue/return`, use the explicit branch facts. `PREVENTS`
  shows the branch that cannot reach the later operation; `REACHES` shows the
  fallthrough branch that can.
- Prefer reverse graph patterns for callers or producers: start from the exact
  target/state and traverse an incoming edge rather than scanning every symbol.
- Treat comments, disabled code, generated legacy sources, and deprecated paths
  as historical unless active evidence selects them.
- Never invent sibling lifecycle names. After one missing exact name in a known
  file, inspect that file's outline once.

## Exact source tools

- `codedb_symbol`: one exact definition/body. The body is local evidence; use
  graph queries for multi-hop navigation, dispatch, arguments, guards, state,
  and control flow. The MCP schema intentionally has no `expand` option.
- `codedb_outline`: symbols in one exact file. Request connected ranges only
  when several answer-critical members in that file must be read together.
- `codedb_read`: one exact file/range, not a discovery tool.
- `codedb_status`: health/freshness only when setup or cache state is asked.

Caller, call-path, dependency, flow-atlas, lexical, composite, and
administrative wrappers remain CLI/internal. Express graph traversal through
`codedb_graph_query`.

Read `references/tools.md` when syntax or properties are unclear.

## Query discipline

- Do not derive keyword bags, synonyms, filename guesses, morphology, or
  framework/language boosts from the user's task.
- Do not use shell/rg after codedb source analysis starts unless the user is
  explicitly benchmarking the no-MCP control.
- Do not reconstruct files through overlapping reads.
- Reach argument values from an anchored `CallSite` through `ARGUMENT`, or seed
  `Value` with an exact `expression` copied from graph/body evidence. Never do
  an unconstrained `Value` scan.
- There is no source-call quota. Token reduction should come from finding the
  evidence chain earlier.

## Setup boundary

Generated configuration and index data stay under `.codedb-mcp`. This skill is
for using an installed server, not registering it.

```text
<skill-root>\assets\codebase-mcp.exe --config <repo-root>\.codedb-mcp\codedb-mcp.toml mcp <repo-root>
```
