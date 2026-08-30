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
| Normal path | MCP `context` with a user-supplied selected name as `query`, or with an opaque CodeStory-returned `symbol_id` as `id` | Context for that one target. |
| Failure path | If the target is ambiguous, `search` then retry `context` with `id`. If `preparing`, wait `retry_after_ms` and retry. | Keeps context tied to a resolvable target. |

## Notes

- Do not pass broad questions to `context`. Use `packet` with `question` for
  broad tasks, `search` for candidate discovery, then `context` with `id` for
  selected anchors.
- A user-supplied exact name, file name, literal, API path, module, or behavior
  term is a `query`. Use `id` only for an opaque `symbol_id` copied unchanged
  from a CodeStory result. Never guess an ID from a display name.
- When a name is ambiguous and the user supplies an exact path, search the bare
  symbol, ignore rows whose `symbol_id` is null, normalize typed result paths
  under the project root, and choose the unique typed candidate at that exact
  path. Copy its `evidence[].symbol_id` into `context.id`; do not send `query`
  too. Do not combine the name and path into a free-text `query`.
- Good `query` values are symbol names, file names, string literals, API paths,
  module names, and specific behavior terms.
- Use `symbol`, `trail`, or `snippet` for local navigation when retrieval is
  degraded.
- Treat `context` output as incomplete when it reports weak hits, semantic
  stale/partial/failed states, no citations, unresolved graph edges, or a
  material result gap. An evidence row matching the returned target
  `symbol_id` remains focused identity and location evidence when its optional
  `excerpt` is null. That null matters only when the request needs source text
  or a claim the remaining row fields cannot support.
- Evidence line bounds are nullable when the producer supplied only a path.
  `symbol_id` is present only for a resolvable citation; null values preserve
  uncertainty and must not be interpreted as line 1 or a followable symbol.
- `doctor` and manual retrieval indexing are maintainer diagnosis surfaces.
