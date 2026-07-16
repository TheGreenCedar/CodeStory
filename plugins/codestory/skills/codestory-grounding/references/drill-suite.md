# `drill-suite` - Run A Manifest-Defined Real-Repo Agent Drill Matrix

Runs a repeatable agent-grounding drill suite from a manifest file. Use it to evaluate whether CodeStory helps an agent answer realistic architecture questions across real repositories.

## Syntax

See [generated CLI syntax](generated-cli-syntax.md) for the current command usage.
Use `<codestory-cli> <command> --help` for the complete option set.

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

The optional `expect` block records evaluation inputs without hard-coding them
into runtime behavior. The production suite reports candidate source-truth
targets; the versioned evaluator compares those targets and claims afterward.

After writing the CodeStory-only draft and completing focused source reads,
record a source-truth ledger:

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

## Output

The command writes:

- `suite-report.json` and `suite-report.md`
- `drill-suite-report.json` or `drill-suite-report.md`, matching `--format`
- one per-case drill directory named `<slug>-drill`
- each successful per-case `drill-report.json`, `drill-report.md`, `drill-summary.json`, and anchor/bridge artifacts

The suite report summarizes per-case mechanical verdicts, freshness,
retrieval mode, anchor resolution, bridge status, source-truth check counts,
source-truth target roles/ranking reasons, bridge `evidence_kind`, and next actions. A case can
be mechanically healthy but still `degraded` when source-truth verification is
required, bridge evidence is partial, retrieval is unavailable, or freshness is
stale. A failed case is recorded as `blocked` instead of aborting the
whole suite, so other manifest cases still produce evidence.

`--jobs` is default-off and only applies to read-only `--refresh none` loops.
It leaves refreshing or indexing runs serialized, caps worker count
automatically, preserves final manifest order in aggregate reports, and writes
each single-case drill's anchor and bridge artifacts in deterministic report
order.
Measure it on the target suite before treating it as a speed-up: multi-case
manifests can benefit from parallel isolated cases, while single-case anchor
and bridge checks may be limited by storage and graph traversal contention.

Per-case `drill` runs include the broad question search plus bounded
supplemental searches for terms such as public pages, home components, Payload
collections, social feeds, comments, and store crates. Those hits are added as
provisional source-truth targets so expected-file misses are visible without
treating broad search results as proof.

## Interpretation

Use `suite-report.json` for machine comparison across runs. Use `suite-report.md` for the short human readout. Then inspect each per-case `drill-summary.json` and `drill-report.json` before drafting CodeStory-only answers.

Do not treat `ready_count`, `degraded_count`, or green index stats as
answer-quality proof by themselves. After source verification, run
`node scripts/score-drill-ledger.mjs <suite-report.json> <ledger.json> [scored-report.json]`.
The scored artifact restores `answer_quality` per repo
and aggregate ready/degraded/failed/pending counts without adding benchmark
formulas to the production CLI.

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
