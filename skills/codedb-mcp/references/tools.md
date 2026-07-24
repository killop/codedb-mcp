# codedb-mcp Tools

The server is repository-bound. Use exact graph/source evidence, not task
keywords. `codedb_graph_query` is both the structural discovery language and
the evidence-chain continuation language.

## Exposed tools

| Tool | Use |
|---|---|
| `codedb_graph_query` | Primary atomic read-only Cypher-like language. Supports `MATCH`, optional `SHORTEST`, scalar/property comparisons, `RETURN`, `ORDER BY`, optional `LIMIT`, typed directions, finite `*min..max`, and selective-endpoint path planning. Labels: `EntryFile`, `BoundaryFile`, `SinkFile`, `Community`, `File`, `Symbol`, concrete symbol kinds, `SharedState`, `CallSite`, `Value`, `Parameter`, `Condition`, `ControlAction`. Relations: `CONTAINS`, `DEPENDS_ON`, `CALLS`, `DISPATCHES_TO`, `HAS_CALLSITE`, `TARGET`, `ARGUMENT`, `BINDS_TO`, `HAS_PARAMETER`, `USED_IN`, `TRUE`, `FALSE`, `PREVENTS`, `REACHES`, `READS`, `WRITES`, `REFERENCES`. Reach `Value` through `ARGUMENT` or seed it with an exact `expression`; unconstrained `Value` scans are rejected. |
| `codedb_symbol` | One exact definition/body. Use `body=true max_results=1`. The body is local evidence; use graph query for multi-hop paths, dispatch, argument binding, guards, shared state, and branch facts. |
| `codedb_outline` | Symbols in one exact file. Use `include_connected_ranges=true` only after the base outline shows several answer-critical members. |
| `codedb_read` | One exact file/range. Not a discovery operation. |
| `codedb_status` | Health/freshness only for setup or diagnostics. |

Key properties:

- `Community`: `id`, `name`, `size`, `boundary_links`,
  `representative_path`;
- `File`: `path`, `community`, `degree`, `boundary_degree`,
  `incoming_degree`, `outgoing_degree`;
- `EntryFile`: zero incoming and nonzero outgoing dependency degree;
- `BoundaryFile`: at least one cross-community graph neighbor;
- `SinkFile`: incoming but no outgoing dependency;
- file `DEPENDS_ON.count=1`; community `DEPENDS_ON.file_edges` is aggregated.
- `HAS_CALLSITE` includes syntax-certain qualified calls even when the callee
  cannot be resolved uniquely. Such nodes have `resolution='syntax'` and keep
  their `ARGUMENT` facts; only precise callsites expose `TARGET` and parameter
  `BINDS_TO` edges.

## Query patterns

Largest graph communities:

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

High-connectivity files inside one community:

```cypher
MATCH (community:Community)-[:CONTAINS]->(file:File)
WHERE community.id=7
RETURN file.path, file.degree, file.boundary_degree,
       file.incoming_degree, file.outgoing_degree
ORDER BY file.boundary_degree DESC, file.degree DESC
```

Cross-community bridges using property-to-property comparison:

```cypher
MATCH (source:File)-[dependency:DEPENDS_ON]->(target:File)
WHERE source.community != target.community
RETURN source.path, source.community, dependency,
       target.path, target.community
ORDER BY source.boundary_degree DESC
```

Shortest call/dispatch connector:

```cypher
MATCH SHORTEST p=(start:Symbol)-[:CALLS|DISPATCHES_TO*1..8]->(leaf:Symbol)
WHERE start.name='Entry' AND leaf.path='exact/path/Implementation.cs'
RETURN p
```

Call arguments bound to callee parameters:

```cypher
MATCH
  (caller:Symbol)-[:HAS_CALLSITE]->(call:CallSite),
  (call)-[argument:ARGUMENT]->(value:Value)-[:BINDS_TO]->(parameter:Parameter)
WHERE caller.name='Entry' AND call.name='Select'
RETURN call.line, call.guard, value.index, value.expression, parameter.name
```

Exact argument-expression reverse lookup:

```cypher
MATCH (owner:Symbol)-[:HAS_CALLSITE]->(call:CallSite)-[:ARGUMENT]->(value:Value)
WHERE value.expression='EventDefine.OnInitEnd'
RETURN owner.name, owner.path, call.name, call.line, call.text, value.index
ORDER BY owner.path
```

Branch outcome around a later operation:

```cypher
MATCH
  (leaf:Symbol)-[:HAS_PARAMETER]->(parameter:Parameter)-[use:USED_IN]->(condition:Condition),
  (condition)-[:TRUE]->(action:ControlAction)-[:PREVENTS]->(operation:CallSite),
  (condition)-[:FALSE]->(fallthrough:ControlAction)-[:REACHES]->(operation)
WHERE leaf.path='exact/path/Leaf.cs' AND parameter.name='tags'
RETURN use.via, condition.text, condition.negated, action.kind, operation.text
```

Shared-state producer:

```cypher
MATCH (consumer:Symbol)-[read:READS]->(state:SharedState)<-[write:WRITES]-(producer:Symbol)
WHERE consumer.name='Consume' AND state.name='_task'
RETURN consumer, read, state, producer, write
```

Use incoming patterns when the target is known:

```cypher
MATCH (target:Symbol {name:'Ready'})<-[call:CALLS]-(caller:Symbol)
RETURN caller.path, call.line, call.text, call.guarded, call.guard
```

Flow-atlas, caller, call-path, dependency wrapper, composite,
lexical/path-discovery, change-feed, and administrative operations remain
CLI/internal. `codedb_bundle` is not exposed.
