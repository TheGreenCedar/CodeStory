# `packet` - Broad Task Packet With Typed Stop/Drill

Builds a bounded answer packet for a broad repository question. Use it before
ordinary source-file reads when the task is explanation, planning, route
tracing, ownership discovery, or change-impact analysis.

## Syntax

See [generated CLI syntax](generated-cli-syntax.md) for the current command usage.
Use `<codestory-cli> <command> --help` for the complete option set.

MCP `packet` arguments are the catalog fields (`question`, `budget`,
`task_class`, `probes`, `extra_probes`, `include_evidence`,
`latency_budget_ms`, and DrillOnce `parent_packet_id` / `option_ids` /
generation pins). Do not send CLI flags such as `--file`, `mode`, or
`max_snippet_bytes` as MCP arguments.

## Agent Paths

| Path | Command | Expected result |
|------|---------|-----------------|
| Normal path | MCP `packet` with `question` and optional `budget` / tagged `probes`. | Packet with compiled `support` units, then `disposition`. |
| Supported / NotEstablished / Unavailable | Stop. For Supported, answer from `support`. For NotEstablished, answer every directly supported claim and name the material gaps without completing the chain by inference. For Unavailable, report the typed preparation reason. | Terminal. Do not search. |
| DrillOnce | Call `packet` once more with the exact original `question`, `parent_packet_id`, the listed `option_ids`, and the pinned `core_generation_id` / `retrieval_generation` when present. | One generation-bound continuation. Then AnswerNow. Merge cannot emit another drill. |
| User-named exact target | `search`, `context`, `trail`, or `snippet` only when the user named that target. | Not packet recovery. |
| Integration edge | Use JSON/MCP structured content. Compact text projects support units first, then disposition. Preserve exact source identifiers from support summaries and citation display names. | Comparable agent loops without a follow-up command list. |

## Notes

- `packet` is for broad questions; `context` is for one concrete target.
- Prefer a compact packet before manually opening source files for a broad explanation or plan.
- `probes` uses tagged objects with `kind` equal to `exact_path`, `symbol_id`,
  `file_symbol`, `free_query`, or `continuation`. For example,
  `{"kind":"exact_path","path":"assets/desk.svg"}` selects that exact
  project-relative file without fuzzy substitution. CLI accepts the same
  object through repeatable `--probe '<json>'`. Typed and legacy probes share
  one combined 16-item limit; every string field is limited to 240 characters.
- Exact path, symbol-ID, file-symbol, and symbol-bound continuation probes add
  exact citations keyed by path or stable node ID. They are not converted back
  into display-name searches.
- A continuation also supplies `contract_version`, `project_id`,
  `core_generation_id`, optional `retrieval_generation`, optional exact
  `symbol_id`, and `query`; reuse fails closed when the selected evidence
  generation changes. Search and definition links emit this bound form.
- `extra_probes` and CLI `--extra-probe` remain legacy compatibility inputs.
  They enter the same runtime resolver. Neither typed nor legacy probes replace
  the compiled disposition.
- Judge the answer from compiled support units (symbol locations, source
  ranges, typed CALL/INHERITANCE/import edges, and complete-query negatives).
  `disposition.kind=supported` means that evidence is present. It does not mean
  an English flow-catalog family was closed. Do not treat a missing named
  family such as `handler_processing` as a reason to search again.
- A parser-partial coverage observation does not invalidate a retained exact
  `source_range` from the same file. That range supports only what its source
  text directly shows; the coverage warning still forbids file-wide absence or
  completeness claims.
- `drill_once` is only for objectively missing, closable evidence: a deadline-
  lost candidate, omitted mandatory support, or one bounded source read of a
  known path. Repeat the exact original question and execute the listed option
  ids once. Do not invent a second search system. CLI `drill` remains the
  maintainer report and is not this agent path.
- `not_established` is terminal. It may be a complete zero-hit, an ambiguous
  probe that needs a user choice, or a packet with useful support whose material
  chain is still incomplete after bounded retrieval. State the supported parts
  and the exact gaps, then stop.
- `unavailable` is stale publication, a dead sidecar, or a hard retrieval
  error. Typed retry or preparation, not search.
- JSON packets include `plan.obligations.version=1`. The obligation ledger
  still records planned flow steps for query planning. It is not an
  agent-facing conclusion.
- CLI JSON, HTTP, and MCP consumers detect the `reported` proof-status value through
  `_meta.codestory_publication.schema_version`, which is `2` for this contract, and should also
  inspect `contract_runtime.pinned_pair_matches`. A configured `CODESTORY_CLI` override is
  surfaced as `contract_runtime.known_override_skew_channel`. The stamp rides on the `initialize`
  result too, so the version is known before the first tool call.
