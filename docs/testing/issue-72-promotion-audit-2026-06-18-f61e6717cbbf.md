# Issue #72 Promotion Audit

Date: 2026-06-18
Base commit: `f61e6717cbbf`
Branch: `codex/issue-72-promotion-audit`
Issue: https://github.com/TheGreenCedar/CodeStory/issues/72
Follow-up blocker: https://github.com/TheGreenCedar/CodeStory/issues/75

## Decision

Promotion is blocked. The focused audit became clean after repairing the live sidecar cache and rerunning the repo-scale e2e with the prepared real-repo drill manifest, but the full publishable packet-runtime benchmark failed its publishable gate.

Do not unblock #73. #74 should not start a final comparison report until #75 is fixed or explicitly accepted as residual risk by the release owner.

This audit does not treat semantic/vector hits, parser count, or `retrieval_mode=full` alone as product proof. `retrieval_mode=full` is recorded only as sidecar readiness; product promotion depends on packet-runtime quality, sufficiency, unresolved-candidate, and SLA gates.

## Scope Analyzed

- Issue, PR, and project state for #38, #62, #71, #72, #74, and saga-labeled issues/PRs.
- Current CodeStory release CLI build from `f61e6717cbbf`.
- Repo-scale CodeStory e2e stats and real-repo drill gate.
- Live sidecar status, doctor readiness, and indexed file coverage.
- Publishable packet-runtime benchmark gate behavior and full language-expansion holdout run.

## Status Matrix

| Gate | Status | Evidence |
| --- | --- | --- |
| Issue/PR/project state | Pass | #62 is closed by merged PR #71 at `f61e6717cbbf`; #72 is in progress; #74 and #73 remain Todo/downstream. |
| Build | Pass | `target\issue-72-promotion-audit\cargo-build-release-codestory-cli.txt`; release build finished in 1m51s with existing dead-code warnings. |
| Repo-scale e2e | Pass after absolute manifest path | `target\issue-72-promotion-audit\codestory-repo-e2e-stats-with-real-drill-absolute.txt`; 2 passed, 0 failed, finished in 184.51s. |
| Sidecar readiness | Pass after repair | `target\issue-72-promotion-audit\retrieval-index-full.json`; `retrieval-status-after-index.json`; `doctor-after-index.json`. |
| Publishable benchmark shape smoke | Pass strictness check | `target\issue-72-promotion-audit\publishable-gate-smoke.txt` failed as expected for `--repeats 1`: `--publishable requires --repeats >= 3`. |
| Full publishable packet-runtime benchmark | Fail | `target\issue-72-promotion-audit\publishable-final-review-console.txt`; artifacts under `target\agent-benchmark\language-expansion-publishable-final-review`. |

## Focused Audit Details

### Repo and Project State

- Local `main` and `origin/main` both resolved to `f61e6717cbbf` before branching.
- PR #71 is merged and closes #62.
- #72 is open and in progress.
- #74 is open/Todo and depends on #72.
- #73 is open/Todo and remains blocked by #74.
- Parent #38 remains open/In Progress.

### Sidecar Readiness

Initial status was not usable for product proof:

- `retrieval_mode`: `unavailable`
- reason: `sidecar_manifest_stale` after changed/indexable files
- evidence: `target\issue-72-promotion-audit\retrieval-status.json` and `doctor.json`

Repair command:

```powershell
target\release\codestory-cli.exe retrieval index --project . --refresh full --format json
```

After repair:

- `retrieval_mode`: `full`
- lexical, semantic dense, symbol-doc, and graph lanes: `ready`
- `symbol_doc_count`: 14,041
- `dense_projection_count`: 760
- `semantic_policy_version`: `graph_first_v1`
- `doctor` readiness: `local_navigation=ready`, `agent_packet_search=ready`

This proves the active sidecar can be made ready. It is not, by itself, a product promotion proof.

### Repo-Scale E2E

Required command initially failed because `CODESTORY_REAL_REPO_DRILL_CASES` was not set:

```powershell
cargo test -p codestory-cli --test codestory_repo_e2e_stats -- --ignored --nocapture
```

The same e2e gate passed when rerun with the existing prepared manifest using an absolute path:

```powershell
$env:CODESTORY_REAL_REPO_DRILL_CASES='C:\Users\alber\source\repos\codestory\target\agent-benchmark\real-repo-drill-cases.json'
cargo test -p codestory-cli --test codestory_repo_e2e_stats -- --ignored --nocapture
```

Headline stats from the passing run:

| Metric | Value |
| --- | ---: |
| `index_seconds` | 83.31 |
| `graph_phase_seconds` | 17.16 |
| `semantic_phase_seconds` | 52.21 |
| `semantic_embedding_ms` | 50,947 |
| `symbol_search_docs_written` | 14,041 |
| `semantic_docs_embedded` | 760 |
| `repeat_full_refresh_seconds` | 28.42 |
| `repeat_semantic_docs_embedded` | 0 |
| `retrieval_index_seconds` | 8.70 |
| `retrieval_status_seconds` | 0.63 |
| `search.sidecar_shadow_retrieval_mode` | `full` |
| `index.error_count` | 0 |

The stats row was appended to `docs/testing/codestory-e2e-stats-log.md`.

## Publishable Benchmark

Command:

```powershell
node scripts\codestory-agent-ab-benchmark.mjs --packet-runtime --packet-runtime-mode both --task-suite language-expansion-holdout --repeats 3 --materialize-repos --jobs 4 --prepare-codestory-jobs 2 --codestory-cli .\target\release\codestory-cli.exe --out-dir target\agent-benchmark\language-expansion-publishable-final-review --timeout-ms 180000 --publishable
```

Artifacts:

- `target\issue-72-promotion-audit\publishable-final-review-console.txt`
- `target\agent-benchmark\language-expansion-publishable-final-review\packet-runtime-summary.md`
- `target\agent-benchmark\language-expansion-publishable-final-review\packet-runtime-summary.json`
- `target\agent-benchmark\language-expansion-publishable-final-review\packet-runtime-runs.jsonl`
- `target\agent-benchmark\language-expansion-publishable-final-review\quality-debug.json`
- `target\agent-benchmark\language-expansion-publishable-final-review\packet-quality-deltas.json`
- `target\agent-benchmark\language-expansion-publishable-final-review\packet-composition.md`
- `target\agent-benchmark\language-expansion-publishable-final-review\packet-composition.json`

Summary:

| Measure | Result |
| --- | ---: |
| packet-runtime rows | 108 |
| command row success | 108 |
| manifest quality pass | 106 |
| manifest quality fail | 2 |
| sufficient packets | 107 |
| partial packets | 1 |
| rows with unresolved retrieval candidates | 108 |
| rows with packet retrieval SLA miss | 21 |

Blocker classification:

| Category | Finding | Evidence |
| --- | --- | --- |
| Product | Every row had unresolved retrieval candidates; publishable expects 0. | `packet-runtime-runs.jsonl`, `quality-debug.json`, console blocker list. |
| Product/performance | 21 rows missed packet retrieval SLA. | `packet-runtime-summary.md`, console blocker list. |
| Product quality | `square-okio` Kotlin cold CLI repeat 2 and `Alamofire` Swift cold CLI repeat 2 failed manifest quality. | `quality-debug.json`. |
| Product sufficiency | `Alamofire` Swift cold CLI repeat 2 was partial, with compact-budget gaps and 3 follow-up commands. | `quality-debug.json`. |
| Harness | No blocker found. | Harness materialized repos, prepared caches, emitted artifacts, and classified blockers. |
| Environment | No blocker found after sidecar repair and absolute real-drill manifest path. | `doctor-after-index.json`, passing e2e output, benchmark artifacts. |

Largest unresolved-candidate counts by repo:

| Repo | Max unresolved candidates |
| --- | ---: |
| AutoMapper-AutoMapper | 51 |
| mdn-learning-area | 48 |
| BurntSushi-ripgrep | 46 |
| fmtlib-fmt | 46 |
| apache-commons-lang | 43 |
| expressjs-express | 31 |
| vercel-swr | 31 |
| Alamofire-Alamofire | 31 |
| redis-redis | 30 |
| gin-gonic-gin | 30 |

SLA misses by repo:

| Repo | Missed rows |
| --- | ---: |
| apache-commons-lang | 6 |
| redis-redis | 6 |
| AutoMapper-AutoMapper | 2 |
| square-okio | 2 |
| Alamofire-Alamofire | 2 |
| dart-lang-http | 2 |
| mdn-learning-area | 1 |

## Follow-Up

Filed #75: https://github.com/TheGreenCedar/CodeStory/issues/75

Smallest recommended mitigation is not to relax the publishable gate. Fix the product path that leaves full-mode sidecar candidates unresolved, then rerun the same publishable command until unresolved candidates are 0, SLA misses are 0, quality is all-pass, and sufficiency is all-pass.

Residual risk: #75 may require deeper runtime retrieval admission or packet composition work. Until that lands, the saga has CodeStory self-e2e and sidecar readiness evidence, but not promotion-grade packet-runtime evidence.
