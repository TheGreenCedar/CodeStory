# Natural-Language Search Validation

Validation must prove two things:

1. exact and anchored searches did not get worse
2. broad natural-language questions now produce enough evidence for a
   CodeStory-only draft before source reads

## Traceability Matrix

| Requirement | Design Coverage | Task Coverage | Validation |
| --- | --- | --- | --- |
| NLS-REQ-1 | Planner, reranking | 1, 4, 7, 10 | exact-anchor tests, negative/noisy tests, real-repo drill anchors |
| NLS-REQ-2 | query assessment and extraction | 2, 4 | unit tests for intent and extracted/dropped terms |
| NLS-REQ-3 | planner subqueries | 2, 4 | JSON/Markdown contract tests for 3 to 8 visible subqueries |
| NLS-REQ-4 | candidate windows | 3, 5 | CLI JSON tests for window labels, limits, counts, and truncation |
| NLS-REQ-5 | repo-text promotion | 6 | promotion unit tests and repo-text fallback fixtures |
| NLS-REQ-6 | multi-term/co-location ranking | 7 | MRR and top-hit tests for multi-anchor architecture queries |
| NLS-REQ-7 | semantic as one signal | 5, 7 | hybrid score breakdown tests and semantic-off fallback tests |
| NLS-REQ-8 | bridge evidence | 8, 10 | bridge completeness tests and drill bridge report assertions |
| NLS-REQ-9 | command boundaries | 3, 8, 9, 11 | CLI output tests and drill partial-discovery assertions |
| NLS-REQ-10 | agent-usable output | 2, 3, 6, 7, 9, 11 | `--why` Markdown snapshots and JSON field contract tests |
| NLS-REQ-11 | deterministic gates | 1, 10, 11 | expanded `search_quality_eval` and real-repo drill harness |
| NLS-REQ-12 | bounded cost | 1, 5, 8, 10 | latency metrics, bridge expansion counters, truncation assertions |

## Fixture Queries

Exact anchors:

- `WorkspaceIndexer`
- `SearchService`
- `TrailResult`
- `SourceGroupCxxCdb`
- `IndexerJava`
- `StorageAccess`
- `Posts`
- `getElsewhereFeed`
- `getCommentAuth`

Broad drill questions:

- `Explain how CodeStory's full-index path flows through CLI/runtime/workspace/indexer/store and how that supports later search, trail, and snippet commands.`
- `Explain how Sourcetrail turns project/source-group configuration into indexing work, then how indexed data is accessed by the application.`
- `Explain how public writing/social surfaces connect to Payload collections, comment auth, and the elsewhere feed.`

Negative/noisy guards:

- `nonexistent noisy payment webhook route qxz`
- `direct UI render from getElsewhereFeed`
- `live mutable storage writes without staged publish`
- `graphql billing webhook oauth tenant resolver`

CodeGraph-inspired query classes:

- subsystem overview
- exact symbol location
- callers and usage
- impact analysis
- interaction between two components
- flow through several layers
- polymorphism or implementation discovery
- route/page/framework entry point
- bug investigation from symptoms

## Metrics

- Recall: expected anchors appear in typed-symbol or promoted-anchor buckets.
- MRR: expected anchors are ranked high enough for an agent to choose them.
- Bridge completeness: required anchor pairs have forward, reverse, shared-file,
  or explicit unsupported bridge status.
- Promotion precision: repo-text leads are not marked anchored unless binding is
  supported.
- Overclaim risk: no high-confidence claim is based only on broad search or
  unpromoted repo text.
- Latency: fixture queries stay below the agreed cap.
- Output usability: `--why` explains terms, windows, promotions, rejected hits,
  and next commands without source reads.

## Commands

Run Cargo commands serially.

```powershell
cargo test -p codestory-runtime --test retrieval_eval
cargo test -p codestory-cli --test search_json_output -- --ignored --nocapture search_quality_eval
cargo test -p codestory-cli --test codestory_repo_e2e_stats -- --ignored --nocapture
```

For a focused manual check after the release binary is built:

```powershell
$Cli = "C:\Users\alber\source\repos\codestory\target\release\codestory-cli.exe"
& $Cli search --project "C:\Users\alber\source\repos\codestory" --query "how full indexing supports search trail and snippet commands" --refresh none --repo-text on --why --format json
```

## Promotion Gates

A change can be treated as complete only when:

- exact anchors still rank first or are explicitly tied for first
- broad drill questions produce usable anchor groups before source reads
- repo-text-only evidence remains partial
- bridge evidence is visible and directional uncertainty is preserved
- the real-repo drill shows fewer source-verification corrections than the
  current baseline
- search-quality docs and grounding skill guidance are updated for any changed
  operator workflow
