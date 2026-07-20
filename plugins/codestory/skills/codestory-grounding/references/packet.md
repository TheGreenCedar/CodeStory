# `packet` - Broad Task Packet With Sufficiency Contract

Builds a bounded answer packet for a broad repository question. Use it before
ordinary source-file reads when the task is explanation, planning, route
tracing, ownership discovery, or change-impact analysis.

## Syntax

See [generated CLI syntax](generated-cli-syntax.md) for the current command usage.
Use `<codestory-cli> <command> --help` for the complete option set.

## Agent Paths

| Path | Command | Expected result |
|------|---------|-----------------|
| Normal path | `<codestory-cli> packet --project <target-workspace> --question "How does indexing flow from CLI to storage?" --budget compact` | Markdown packet with cited claims, budget usage, gaps, and follow-up commands. |
| Failure path | If the packet reports `partial` or `insufficient`, follow its `follow_up_commands`, usually deeper packet budget or concrete `search`, `context`, `trail`, or `snippet` calls. | Broad exploration is bounded by reported gaps instead of drifting into repeated file reads. |
| Integration edge | Use JSON output for harnesses and stdio clients. If `sufficiency.status` is `sufficient` and `follow_up_commands` is empty, answer from packet supported claims and include a compact support-file list from `answer.citations` and `sufficiency.avoid_opening_paths`; budget truncation alone is not a gap. Preserve exact source identifiers and covered-claim phrases from `sufficiency.covered_claims` and citation display names. Do not merge repeated exact anchors into shorthand that drops required prefixes; write each exact anchor independently when naming declarations, tables, symbols, selectors, or other source-defined terms. Treat `sufficiency.avoid_opening` as compatibility prose only. | Makes benchmark traces and agent loops comparable across runs. |

## Notes

- `packet` is for broad questions; `context` is for one concrete target.
- Prefer `packet --budget compact` before manually opening source files for a broad explanation or plan.
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
  They enter the same runtime resolver. Neither typed nor legacy probes promote
  packet sufficiency or choose route order.
- Treat `sufficiency.status=partial` as useful but incomplete evidence. The packet should say which next command would deepen or verify the answer.
- Architecture, data-flow, and route-tracing sufficiency requires causal flow-role coverage, not just citation or claim counts. Generic "inspect this anchor" claims may guide follow-up, but they do not make a packet safe to answer from.
