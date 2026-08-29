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
| `available` | Answer only what the returned evidence rows establish and name material gaps. | Terminal. Do not search to strengthen the answer. |
| `continuation_available` | Repeat the question with `parent_packet_id=continuation.continuation_id`, `option_ids=continuation.gap_ids.map((item) => item.gap_id)`, and the core/retrieval generation IDs from `publication.core.generation_id` and `publication.retrieval.retrieval_generation`. | One bounded continuation, then answer from the combined evidence and the continuation result's remaining gaps. |
| `no_useful_evidence` / `unavailable` | State the evidence gap. Inspect source only for an exact user-named file or a material gap that itself names one exact path. | Terminal. |
| User-named exact target | `search`, `context`, `trail`, or `snippet` only when the user named that target. | Not packet recovery. |
| Integration edge | Use JSON/MCP structured content. Preserve exact paths, symbol IDs, ranges, evidence IDs, and gap IDs. | The public result carries no proof disposition. |

## Notes

- `packet` is for broad questions; `context` is for one concrete target.
- Prefer the default standard packet before manually opening source files for a
  broad explanation or plan. Select `compact` explicitly when minimizing
  context is more important than retaining the fuller evidence set.
- `probes` uses tagged objects with `kind` equal to `exact_path`, `symbol_id`,
  `file_symbol`, `free_query`, or `continuation`. For example,
  `{"kind":"exact_path","path":"assets/desk.svg"}` selects that exact
  project-relative file without fuzzy substitution. Typed and legacy probes share
  one combined 16-item limit; every string field is limited to 240 characters.
- A path named only for a conditional continuation or source fallback is
  fallback-only. The initial broad request uses only `project` and `question`;
  do not send that path as an initial probe or invent continuation pins unless
  the user explicitly requested a probe. A generic gap does not combine with
  the conditional path to authorize a source read; the returned material gap
  must itself name that exact path.
- Exact path, symbol-ID, file-symbol, and symbol-bound continuation probes add
  exact citations keyed by path or stable node ID. They are not converted back
  into display-name searches.
- A continuation also supplies `contract_version`, `project_id`,
  `core_generation_id`, optional `retrieval_generation`, optional exact
  `symbol_id`, and `query`; reuse fails closed when the selected evidence
  generation changes. Search and definition links emit this bound form.
- `extra_probes` remains a legacy compatibility input. It enters the same
  runtime resolver. Neither typed nor legacy probes replace the returned
  availability, evidence, or gap fields.
- Judge each claim from the concrete evidence rows: exact source, structural
  source, graph relations, and retrieval excerpts. A bounded negative query is
  a gap, never proof that something is absent.
- A parser-partial coverage observation does not invalidate a retained exact
  `source_range` from the same file. That range supports only what its source
  text directly shows; the coverage warning still forbids file-wide absence or
  completeness claims.
- A continuation is only for objectively missing, closable evidence and has a
  positive `remaining_rounds` bound. Execute it once. Do not invent a second
  search system. A first-pass continuation-required gap is resolved when it is
  absent from the continuation result; retain other first-pass gaps only when
  that result still reports them. CLI `drill` remains a maintainer report and
  is not this agent path.
- `no_useful_evidence` is terminal even when retrieval itself was healthy.
  State the exact gaps, then stop. `unavailable` means the requested evidence
  surface could not serve the request. Preserve it unless an exact user-named
  file or exact path identified by a material gap authorizes a focused read.
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
