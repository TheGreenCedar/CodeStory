# `packet` - Broad Task Packet With Sufficiency Contract

Builds a bounded answer packet for a broad repository question. Use it before
ordinary source-file reads when the task is explanation, planning, route
tracing, ownership discovery, or change-impact analysis.

## Usage

```
<codestory-cli> packet [OPTIONS] --question <QUESTION>
```

## Key Options

| Option | Default | Use |
|--------|---------|-----|
| `--project <path>` | `.` | Repository root to query. Always pass it explicitly. |
| `--cache-dir <path>` | auto | Reuse or isolate a specific cache. |
| `--question <text>` | required | Broad repository question or task. |
| `--budget <tiny|compact|standard|deep>` | `compact` | Output and retrieval budget. Start compact; deepen only when the packet reports gaps. |
| `--task-class <class>` | auto | Optional retrieval hint: architecture explanation, bug localization, change impact, route tracing, symbol ownership, data flow, or edit planning. |
| `--refresh <auto|full|incremental|none>` | `none` | Read an existing cache unless you intentionally refresh. |
| `--format <markdown|json>` | `markdown` | Human or structured output. |
| `--output-file <path>` | none | Write output to a file. |
| `--no-evidence` | off | Omit citation edge ids and score breakdowns. Avoid this for grounded claims. |
| `--latency-budget-ms <n>` | none | Optional runtime latency target for integrations. |

## Agent Paths

| Path | Command | Expected result |
|------|---------|-----------------|
| Normal path | `<codestory-cli> packet --project <target-workspace> --question "How does indexing flow from CLI to storage?" --budget compact` | Markdown packet with cited claims, budget usage, gaps, and follow-up commands. |
| Failure path | If the packet reports `partial` or `insufficient`, follow its `follow_up_commands`, usually deeper packet budget or concrete `search`, `context`, `trail`, or `snippet` calls. | Broad exploration is bounded by reported gaps instead of drifting into repeated file reads. |
| Integration edge | Use JSON output for harnesses and stdio clients. If `sufficiency.status` is `sufficient` and `follow_up_commands` is empty, answer from packet supported claims and include a compact support-file list from `answer.citations` and `sufficiency.avoid_opening`; budget truncation alone is not a gap. | Makes benchmark traces and agent loops comparable across runs. |

## Notes

- `packet` is for broad questions; `context` is for one concrete target.
- Prefer `packet --budget compact` before manually opening source files for a broad explanation or plan.
- Treat `sufficiency.status=partial` as useful but incomplete evidence. The packet should say which next command would deepen or verify the answer.
