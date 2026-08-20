# `context` - Target Context For One Concrete Target

Builds target context around one concrete retrieval target (`query` must name a
symbol, file, literal, API path, module, or behavior term, not a broad
question). Fails closed unless full retrieval is ready. For broad questions use
`packet`; for discovery use `search`.

## Syntax

See [generated MCP syntax](generated-mcp-syntax.md) for live fields. Do not send
CLI flags. Every call requires `project` (absolute repository root).

## Agent Paths

| Path | Command | Expected result |
|------|---------|-----------------|
| Normal path | MCP `context` with `query` or `id` | Context for that one target. |
| Failure path | If the target is ambiguous, `search` then retry `context` with `id`. If `preparing`, wait `retry_after_ms` and retry. | Keeps context tied to a resolvable target. |

## Notes

- Do not pass broad questions to `context`. Use `packet` with `question` for
  broad tasks, `search` for candidate discovery, then `context` with `id` for
  selected anchors.
- Good `query` values are symbol names, file names, string literals, API paths,
  module names, and specific behavior terms.
- Use `symbol`, `trail`, or `snippet` for local navigation when retrieval is
  degraded.
- Treat `context` output as incomplete when it reports weak hits, semantic
  stale/partial/failed states, missing snippets, no citations, or unresolved
  graph edges.
- `doctor` and manual retrieval indexing are maintainer diagnosis surfaces.
