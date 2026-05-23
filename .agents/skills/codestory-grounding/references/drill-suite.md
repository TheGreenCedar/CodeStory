# `drill-suite` - Run The Fixed Real-Repo Agent Drill Matrix

Runs the repeatable Sourcetrail, CodeStory, and rootandruntime agent-grounding drill from the CodeStory owner checkout. Use it when evaluating whether CodeStory helps an agent answer realistic cross-repo architecture questions, not just whether indexing succeeds.

## Usage

```
target/release/codestory-cli(.exe) drill-suite [OPTIONS] --output-dir <DIR>
```

## Arguments

| Argument | Type | Default | Description |
|----------|------|---------|-------------|
| `--project` | path | `.` | CodeStory owner checkout used to derive sibling target repos |
| `--cache-dir` | path | *auto* | Optional suite cache root; explicit roots are split into per-repo sub-caches |
| `--output-dir` | path | **required** | Directory for aggregate suite reports and per-repo drill artifacts |
| `--refresh` | enum | `full` | Refresh strategy passed to each per-repo drill: `auto`, `full`, `incremental`, `none` |
| `--format` | enum | `json` | Primary aggregate output format: `json` or `markdown` |

## Output

The command writes:

- `suite-report.json` and `suite-report.md`
- `drill-suite-report.json` or `drill-suite-report.md`, matching `--format`
- per-repo drill directories for `sourcetrail`, `codestory`, and `rootandruntime`
- each per-repo `drill-report.json`, `drill-report.md`, `drill-summary.json`, and anchor/bridge artifacts

The suite report summarizes per-repo verdicts, freshness, retrieval mode, anchor resolution, bridge status, source-truth check counts, and next actions. A repo can be mechanically healthy but still `degraded` when source-truth verification is required, bridge evidence is partial, retrieval is symbolic-only, or freshness is stale.

## Interpretation

Use `suite-report.json` for machine comparison across runs. Use `suite-report.md` for the short human readout. Then inspect the per-repo `drill-summary.json` and `drill-report.json` before drafting CodeStory-only answers.

Do not treat `ready_count`, `degraded_count`, or green index stats as answer-quality proof by themselves. The real question is whether the Evidence Packet, bridge rows, endpoint files, consumer summaries, and source-truth checks let an agent draft an answer that survives focused source verification.

For cached or iterative runs, prefer `--refresh full` when proving historical failures are gone. Use `--refresh none` only when the index was just rebuilt in the same session and freshness is known to be current.
