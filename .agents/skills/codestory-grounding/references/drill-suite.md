# `drill-suite` - Run A Manifest-Defined Real-Repo Agent Drill Matrix

Runs a repeatable agent-grounding drill suite from the CodeStory owner checkout. Use it when evaluating whether CodeStory helps an agent answer realistic architecture questions across one or more real repositories, not just whether indexing succeeds.

## Usage

```
target/release/codestory-cli(.exe) drill-suite [OPTIONS] --case-file <FILE> --output-dir <DIR>
```

## Case Manifest

The suite is intentionally manifest-driven so the CLI is not coupled to one workstation or a fixed set of repos. Relative `project` paths resolve from the manifest directory.

```json
{
  "suite": "agent-grounding-regression",
  "cases": [
    {
      "slug": "example-repo",
      "project": "../example-repo",
      "question": "Explain how the public API reaches the backing store.",
      "anchors": ["ApiController", "Repository", "StorageClient"]
    }
  ]
}
```

## Arguments

| Argument | Type | Default | Description |
|----------|------|---------|-------------|
| `--project` | path | `.` | CodeStory owner checkout used to run the suite |
| `--case-file` | path | **required** | JSON manifest describing suite cases |
| `--cache-dir` | path | *auto* | Optional suite cache root; explicit roots are split into per-case sub-caches |
| `--output-dir` | path | **required** | Directory for aggregate suite reports and per-case drill artifacts |
| `--refresh` | enum | `full` | Refresh strategy passed to each per-case drill: `auto`, `full`, `incremental`, `none` |
| `--format` | enum | `json` | Primary aggregate output format: `json` or `markdown` |

## Output

The command writes:

- `suite-report.json` and `suite-report.md`
- `drill-suite-report.json` or `drill-suite-report.md`, matching `--format`
- one per-case drill directory named `<slug>-drill`
- each successful per-case `drill-report.json`, `drill-report.md`, `drill-summary.json`, and anchor/bridge artifacts

The suite report summarizes per-case verdicts, freshness, retrieval mode, anchor resolution, bridge status, source-truth check counts, and next actions. A case can be mechanically healthy but still `degraded` when source-truth verification is required, bridge evidence is partial, retrieval is symbolic-only, or freshness is stale. A failed case is recorded as `blocked` instead of aborting the whole suite, so other manifest cases still produce evidence.

## Interpretation

Use `suite-report.json` for machine comparison across runs. Use `suite-report.md` for the short human readout. Then inspect each per-case `drill-summary.json` and `drill-report.json` before drafting CodeStory-only answers.

Do not treat `ready_count`, `degraded_count`, or green index stats as answer-quality proof by themselves. The real question is whether the Evidence Packet, bridge rows, endpoint files, consumer summaries, and source-truth checks let an agent draft an answer that survives focused source verification.

For cached or iterative runs, prefer `--refresh full` when proving historical failures are gone. Use `--refresh none` only when the index was just rebuilt in the same session and freshness is known to be current.
