# `packet` - Broad Evidence Packet

Builds a bounded answer packet for a broad repository question. Use it before
ordinary source-file reads when the task is explanation, planning, route
tracing, ownership discovery, or change-impact analysis.

## Syntax

See [generated MCP syntax](generated-mcp-syntax.md) for live fields. Do not send
CLI flags. Every call requires `project` (absolute repository root).

## Agent Paths

| Path | Command | Expected result |
|------|---------|-----------------|
| Normal path | MCP `packet` with `question` and optional `budget` / tagged `probes`. | Schema-3 evidence rows, gaps, retrieval state, diagnostics capability, and optional continuation. |
| `available` | Use the returned evidence rows first. Follow an exact identity with `snippet`, `context`, or an explicit graph operation when the task still needs it. | The packet never asserts answer sufficiency. |
| `continuation_available` | Repeat the question with `parent_packet_id=continuation.continuation_id`, `option_ids=continuation.gap_ids.map((item) => item.gap_id)`, and the core/retrieval generation IDs from `publication.core.generation_id` and `publication.retrieval.retrieval_generation`. | One bounded packet continuation; ordinary exact navigation remains available afterward. |
| `no_useful_evidence` / `unavailable` | Preserve the reported gap and use exact search, source, or relations if the task can still be grounded. | Do not turn absence of packet evidence into an absence claim. |
| Explicit target | `search`, `context`, `trail`, or `snippet` may be used directly when the user or prior evidence identifies the target. | These are the packet substrate and fallback. |
| Integration edge | Use JSON/MCP structured content. Preserve exact paths, symbol IDs, ranges, evidence IDs, and gap IDs. | The public result carries no proof disposition. |

## Notes

- `packet` is for broad questions; `context` is for one concrete target.
- When the user supplies an exact packet question, copy it verbatim into
  `question`, including its punctuation. Do not paraphrase or trim it, and use
  the same bytes for an offered continuation.
- Prefer the default standard packet before manually opening source files for a
  broad explanation or plan. Select `compact` explicitly when minimizing
  context is more important than retaining the fuller evidence set.
- `probes` uses tagged objects with `kind` equal to `exact_path`, `symbol_id`,
  `qualified_symbol`, `file_symbol`, `free_query`, or `continuation`. For example,
  `{"kind":"exact_path","path":"assets/desk.svg"}` selects that exact
  project-relative file without fuzzy substitution. The request accepts at most
  sixteen typed probes; every string field is limited to 240 characters.
- Use an exact probe only for an identity supplied by the user or already
  established by repository evidence. Do not translate prose into guessed
  paths, symbols, answer stages, or relation policy.
- Exact path, symbol-ID, file-symbol, and symbol-bound continuation probes add
  exact citations keyed by path or stable node ID. They are not converted back
  into display-name searches.
- A continuation supplies `contract_version`, `project_id`,
  `core_generation_id`, optional `retrieval_generation`, and one typed stable
  selector carrying the exact uncovered structural reason. Reuse fails closed
  when the selected publication changes. Diagnostic text is never reissued as
  a retrieval query.
- Judge each claim from the concrete evidence rows: exact source, structural
  source, graph relations, and retrieval excerpts. A bounded negative query is
  a gap, never proof that something is absent. A path or stable identity in an
  evidence row may be followed with an exact source or relation operation when
  the agent still needs more evidence.
- A parser-partial coverage observation does not invalidate a retained exact
  `source_range` from the same file. That range supports only what its source
  text directly shows; the coverage warning still forbids file-wide absence or
  completeness claims.
- A packet continuation is bounded to one round. It names stable selectors
  and a structural gap, never a claim that the current packet is insufficient
  for the answer. After that round, let the task determine whether exact
  navigation is useful rather than manufacturing another packet policy.
- `no_useful_evidence` and `unavailable` describe the packet result, not the
  repository. Preserve the gap when falling back to exact navigation.
- Packet JSON is a closed root object. It contains no internal plan,
  obligations, score, eligibility, or proof-disposition fields.
- The complete MCP ToolResult is limited to 16 KiB. If the mandatory envelope
  cannot fit, packet returns the explicit `budget_exceeded` variant with no
  partial evidence. Diagnostics remain immutable and separately capability-
  addressed for ten minutes in the serving session.
- CLI, HTTP, and MCP consumers use CodeStory publication
  `schema_version=minimum_compatible_schema_version=3`. MCP protocol revision
  negotiation is separate: CodeStory supports `2024-11-05`, `2025-03-26`,
  `2025-06-18`, and `2025-11-25`, preferring the newest.
