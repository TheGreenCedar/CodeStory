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
- Treat `sufficiency.status=partial` as useful but incomplete evidence. The packet should say which next command would deepen or verify the answer.
- Architecture, data-flow, and route-tracing sufficiency requires causal flow-role coverage, not just citation or claim counts. Generic "inspect this anchor" claims may guide follow-up, but they do not make a packet safe to answer from.
