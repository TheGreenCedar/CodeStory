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
      "anchors": ["ApiController", "Repository", "StorageClient"],
      "expect": {
        "source_truth_files": ["src/api/controller.ts", "src/store/repository.ts"],
        "false_claims": ["public API writes directly to the database"],
        "min_anchor_resolution": 3,
        "allow_partial_bridges": true
      }
    }
  ]
}
```

The optional `expect` block lets the suite report target-ranking and
answer-quality gaps without hard-coding those expectations into the CLI.
Missing expected files are reported as source-truth target misses. False claims
are checked when a source-truth ledger is supplied.

After writing the CodeStory-only draft and completing focused source reads,
pass an optional source-truth ledger:

```json
{
  "schema_version": 1,
  "suite": "agent-grounding-regression",
  "cases": [
    {
      "slug": "example-repo",
      "draft_written": true,
      "claims": [
        {
          "id": "claim-1",
          "text": "The controller delegates writes through Repository.",
          "classification": "correct",
          "changed_after_source_read": false,
          "source_files": ["src/api/controller.ts", "src/store/repository.ts"]
        }
      ],
      "layer_findings": [
        {
          "layer": "graph_trail",
          "status": "partial",
          "detail": "Trail showed the controller and repository but not the framework route entry."
        }
      ]
    }
  ]
}
```

Allowed claim classifications are `correct`, `partial`, `misleading`, and
`unsupported`.

## Arguments

| Argument | Type | Default | Description |
|----------|------|---------|-------------|
| `--project` | path | `.` | CodeStory owner checkout used to run the suite |
| `--case-file` | path | **required** | JSON manifest describing suite cases |
| `--ledger` | path | *none* | Optional source-truth ledger JSON to merge into answer-quality scoring |
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

The suite report summarizes per-case verdicts, answer-quality status, freshness,
retrieval mode, anchor resolution, bridge status, source-truth check counts,
expected-file recall, source-truth target roles/ranking reasons, bridge
`evidence_kind`, claim classification counts, and next actions. A case can
be mechanically healthy but still `degraded` when source-truth verification is
required, bridge evidence is partial, retrieval is symbolic-only, freshness is
stale, expected files were missed, or the ledger records partial/materially
revised claims. A failed case is recorded as `blocked` instead of aborting the
whole suite, so other manifest cases still produce evidence.

Per-case `drill` runs include the broad question search plus bounded
supplemental searches for terms such as public pages, home components, Payload
collections, social feeds, comments, and store crates. Those hits are added as
provisional source-truth targets so expected-file misses are visible without
treating broad search results as proof.

## Interpretation

Use `suite-report.json` for machine comparison across runs. Use `suite-report.md` for the short human readout. Then inspect each per-case `drill-summary.json` and `drill-report.json` before drafting CodeStory-only answers.

Do not treat `ready_count`, `degraded_count`, or green index stats as
answer-quality proof by themselves. The real question is whether the Evidence
Packet, bridge rows, endpoint files, consumer summaries, and source-truth checks
let an agent draft an answer that survives focused source verification. When a
ledger is supplied, use `answer_quality.final_answer_status` and the claim
counts to decide whether the final answer is `ready`, `degraded`, `failed`,
`blocked`, or still `pending_source_verification`.

Bridge `evidence_kind` distinguishes `graph_path`, `framework_route`,
`component_usage`, `data_collection_usage`, `shared_file`, `repo_text_hint`,
`source_truth_only`, and `isolated_anchors`. `source_truth_only` is an explicit
degraded bridge: CodeStory found concrete files to verify, but no graph,
framework, or data bridge should be treated as proven. Source-truth target
details distinguish public/runtime surfaces from data-store, auth, admin, test,
generated, and auxiliary files so ranking defects are visible in JSON instead of
buried in raw file lists.
Native class anchors may also include bounded related method targets in
consumer summaries, for example Sourcetrail source-group methods or
`IndexerJava::doIndex`; use those as concrete snippet/trail follow-ups rather
than treating class containment as runtime-flow proof.

For cached or iterative runs, prefer `--refresh full` when proving historical failures are gone. Use `--refresh none` only when the index was just rebuilt in the same session and freshness is known to be current.
