# Performance and Search Quality Draft

Lane scope: ideas 6, 7, and 8 for the CLI-first navigation next wave.

This draft covers measured performance review, targeted parallelization/async opportunities, and Search Quality 2.0. It intentionally avoids MCP/server expansion, broad async rewrites, and speculative optimization. Prior CodeStory evidence showed broad semantic performance changes can regress repo-scale e2e time, so every optimization proposal must start from measurement and promote only with before/after proof.

## Blueprint Components

| Component | Responsibility | Primary Surfaces |
| --- | --- | --- |
| **PerformanceReviewHarness** | Capture repeatable CLI timing, phase metrics, semantic-doc reuse counts, query/search latency, and candidate optimization evidence. | `codestory_repo_e2e_stats`, `docs/testing/codestory-e2e-stats-log.md`, `codestory-bench` benches |
| **ParallelizationCandidateGate** | Decide whether a parallel/async change is allowed to proceed, based on dominant-cost evidence, bounded scope, and rollback path. | performance review notes, targeted runtime/indexer code paths, bench/e2e results |
| **SearchQualityHarness** | Measure search recall, MRR, latency, route recall, fallback behavior, and ranking explanation quality from CLI tests. | `search_quality_eval`, `retrieval_eval`, `search --why --format json` |
| **ValidationRecord** | Store promotion evidence in existing docs/test output instead of ad hoc notes. | `docs/testing/search-quality-eval.md`, `docs/testing/codestory-e2e-stats-log.md`, PR summary |

## Requirements

### R6: Measured Performance Review

- **R6-AC1 Baseline capture**: Before a performance optimization is proposed, **PerformanceReviewHarness** SHALL capture one current baseline for the affected path, including command, commit, environment knobs, cold/warm cache status, and headline timings.
- **R6-AC2 Dominant-cost evidence**: A performance finding SHALL identify the dominant cost center with measured evidence, such as graph phase, semantic phase, search latency, store write/read, repo-text scan, or CLI JSON rendering.
- **R6-AC3 Regression guard**: Any accepted optimization SHALL define a concrete no-regression threshold for the affected metric and a stop condition when search quality, semantic-doc reuse, or CLI output correctness degrades.
- **R6-AC4 Existing log reuse**: Repo-scale index/search claims SHALL use the existing e2e stats log shape rather than a new metrics document unless a new metric cannot be represented there.

### R7: Parallelization and Async Opportunities

- **R7-AC1 Measurement-first gate**: **ParallelizationCandidateGate** SHALL reject broad parallel semantic scoring, broad async runtime migration, or cargo-wide concurrency changes unless a measured bottleneck shows that path dominates user-visible time.
- **R7-AC2 Targeted candidate shape**: A parallel/async candidate SHALL name the exact code path, expected resource bottleneck, work unit boundary, maximum concurrency, ordering requirements, and fallback to the current serial behavior.
- **R7-AC3 Contention safety**: Candidates SHALL include a failure-path check for build/cache/store locks, memory pressure, search-index writer contention, and nondeterministic result ordering before promotion.
- **R7-AC4 Promotion evidence**: A parallelization change SHALL be promoted only after a targeted micro/bench result and at least one CLI integration run show improved or unchanged p95/maximum latency with unchanged result quality.

### R8: Search Quality 2.0

- **R8-AC1 Eval corpus coverage**: **SearchQualityHarness** SHALL cover exact symbol queries, natural-language queries, route/endpoint queries, repo-text fallback queries, and at least one negative/noisy query.
- **R8-AC2 Metrics**: The eval SHALL report recall, MRR, max latency, fallback source used, and whether expected anchors appeared in `indexed_symbol_hits`, `repo_text_hits`, or both.
- **R8-AC3 Explainability**: `search --why --format json` SHALL expose enough score/fallback detail to explain why a top result won without requiring a debugger.
- **R8-AC4 Quality gates**: Ranking or route-search changes SHALL fail validation when expected anchors disappear, MRR drops below the agreed threshold, or latency crosses the fixture cap without an explicit documented reason.
- **R8-AC5 CLI-first boundary**: Search-quality work SHALL improve CLI commands/tests/docs only; no new server, MCP, or watch behavior is part of this lane.

## Design Notes

- Use `codestory_repo_e2e_stats` for repo-scale timing because it already captures index, graph phase, semantic phase, semantic-doc reuse/embed/stale counts, and search/symbol/trail/snippet timings.
- Use `search_quality_eval` as the first Search Quality 2.0 home. Extend it before creating a second harness so route, symbol, natural-language, and fallback checks share one command.
- Use `retrieval_eval` for runtime-level search/grounding quality when ranking logic changes below the CLI adapter.
- Treat semantic phase and search scoring separately. A faster semantic-doc build does not prove better query latency, and a faster query does not prove semantic cache reuse stayed healthy.
- Do not reintroduce broad semantic score parallelization as a default optimization. If semantic scoring becomes the measured bottleneck again, first test bounded candidate reduction, vector encoding, prefilter/rescore limits, or cache reuse before parallelizing all docs.
- Keep CLI JSON/Markdown contracts stable. Performance evidence may add test output or doc rows, but should not force user-facing format churn unless another lane approves it.
- Serialize heavy cargo/e2e validation in this repo to avoid measuring lock contention instead of the intended change.

## Implementation Tasks

- [ ] 1. Expand the measured performance review checklist.
  - Add a concise checklist to the nearest testing/debugging doc that maps affected path -> baseline command -> metric -> promotion threshold.
  - Include cold cache, warm cache, semantic-doc reuse, and search latency as distinct evidence types.
  - _Requirements: R6-AC1, R6-AC2, R6-AC3, R6-AC4_

- [ ] 2. Extend `codestory_repo_e2e_stats` only if a missing metric blocks performance triage.
  - Prefer existing fields first: `index_seconds`, `graph_phase_seconds`, `semantic_phase_seconds`, semantic-doc counts, and per-command seconds.
  - If needed, add one narrowly named field and update the stats log instructions.
  - _Requirements: R6-AC1, R6-AC2, R6-AC4_

- [ ] 3. Add a parallelization candidate template.
  - Capture path, bottleneck evidence, concurrency limit, ordering requirements, resource risks, rollback behavior, and required validation commands.
  - Place the template in docs, not in final spec prose, unless implementation is approved.
  - _Requirements: R7-AC1, R7-AC2, R7-AC3_

- [ ] 4. Triage existing hot paths before changing concurrency.
  - Compare semantic phase, graph phase, search latency, repo-text scan stats, and store/search-doc writes.
  - Rank candidates by measured user-visible impact and implementation risk.
  - _Requirements: R6-AC2, R7-AC1, R7-AC2_

- [ ] 5. Extend `search_quality_eval` expectations.
  - Add route/endpoint, exact symbol, natural-language, repo-text fallback, and negative/noisy query cases.
  - Emit recall, MRR, max latency, fallback source, and per-query anchor status.
  - _Requirements: R8-AC1, R8-AC2, R8-AC4_

- [ ] 6. Verify `search --why --format json` remains useful for ranking diagnosis.
  - Ensure top hits expose lexical/semantic/graph contributions, fallback status, match quality, and deterministic next-action guidance where available.
  - Add or adjust focused assertions only for missing diagnostic fields.
  - _Requirements: R8-AC3, R8-AC4, R8-AC5_

- [ ] 7. Add promotion gates for ranking changes.
  - Require `search_quality_eval` for CLI ranking/route-search changes.
  - Require `retrieval_eval` for runtime retrieval/ranking changes.
  - Require repo-scale e2e stats only when default indexing, semantic-doc persistence, embedding reuse, or cold-start behavior changes.
  - _Requirements: R6-AC3, R7-AC4, R8-AC4, R8-AC5_

## Validation Points

| Validation | Command or Evidence | Covers |
| --- | --- | --- |
| Normal path | `cargo test -p codestory-cli --test search_json_output -- --ignored --nocapture search_quality_eval` | R8-AC1, R8-AC2, R8-AC4 |
| Runtime search path | `cargo test -p codestory-runtime --test retrieval_eval` | R8-AC3, R8-AC4 |
| Performance integration path | `cargo build --release -p codestory-cli`; `cargo test -p codestory-cli --test codestory_repo_e2e_stats -- --ignored --nocapture` | R6-AC1, R6-AC2, R6-AC3, R7-AC4 |
| Bench path | `cargo check -p codestory-bench --benches`; targeted Criterion bench when practical | R6-AC2, R7-AC2, R7-AC4 |
| Failure path | Force missing semantic assets or lexical-only mode and verify fallback messaging/search output remains explicit | R8-AC2, R8-AC3, R8-AC5 |
| Integration edge | Run one route query and one natural-language query with `--repo-text off`, then one fallback query with repo-text enabled | R8-AC1, R8-AC2, R8-AC4 |
| Parallelization edge | Compare serial fallback versus bounded parallel candidate on identical cache/project, including result ordering and memory/lock notes | R7-AC1, R7-AC3, R7-AC4 |

## Open Decisions

- Decide whether Search Quality 2.0 should keep a single ignored CLI test or split into a fast fixture test plus a heavier repo-scale search suite.
- Decide the initial MRR and latency thresholds after the first expanded eval run; do not invent thresholds before observing current behavior.
- Decide whether performance review output needs a machine-readable JSON artifact, or whether existing test output plus docs rows is sufficient for the next wave.
