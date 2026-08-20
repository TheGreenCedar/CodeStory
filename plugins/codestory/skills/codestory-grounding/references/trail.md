# `trail` — Follow a Symbol's Call/Reference Graph

Builds a directed graph trail starting from a target symbol. Supports neighborhood exploration, outgoing-reference traversal, and incoming-reference traversal with configurable depth, direction, and filtering.

## Syntax

See [generated MCP syntax](generated-mcp-syntax.md) for live fields. Do not send
CLI flags. Every call requires `project` (absolute repository root).

## Trail direction

MCP `trail` takes `direction`: `incoming`, `outgoing`, or `both` (default).
There is no `mode` field. Depth defaults to 2.

## Output

```
# Trail
resolved: `AppController::open_project` -> [abc123] open_project [FUNCTION]
mode: neighborhood  depth: 2  direction: both  max_nodes: 24
nodes: 8  edges: 12  omitted_edges: 3  truncated: false
- [abc123] open_project [FUNCTION] `src/lib.rs`:150 (depth 0)
- [def456] Storage::open [FUNCTION] `src/storage.rs`:20 (depth 1)
- [ghi789] main [FUNCTION] `src/main.rs`:5 (depth 1)
edges:
- [edge1] open_project -call-> Storage::open certainty=certain
- [edge2] main ~call~> open_project certainty=probable
- [edge3] open_project ?call?> maybe_helper certainty=uncertain
```

## Edge Certainty Notation

Markdown trail output renders edge certainty directly in the arrow shape:

| Certainty | Arrow | Meaning |
|-----------|-------|---------|
| `certain` / `definite` | `-call->` | Verified or high-confidence edge |
| `probable` | `~call~>` | Likely edge inferred from available evidence |
| `uncertain` / `speculative` | `?call?>` | Low-confidence edge |
| missing certainty | `-call-> [unresolved]` | Legacy or unresolved certainty metadata |

MCP `trail` optional `story: true` includes a readable trail story DTO. There
is no `mode`, `include_tests`, `mermaid`, or `output_file` field.

## Interpreting Trail Noise

Focus on whether unrelated resolved targets disappeared after a fix. Local helper calls can still show up as `[unknown]` nodes such as `once`, `from`, or `copied`; that is usually acceptable if they are no longer being resolved to unrelated symbols elsewhere in the repo.
