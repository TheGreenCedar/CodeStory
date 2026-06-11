# Codestory E2E Stats Log

Append one entry before each commit after running:

```sh
cargo build --release -p codestory-cli
cargo test -p codestory-cli --test codestory_repo_e2e_stats -- --ignored --nocapture
```

Keep the full emitted JSON in the test output when reviewing locally, and add the headline metrics here so search/index reuse trends are visible over time. For performance branches, capture the baseline and no-regression threshold from [performance-review-playbook.md](performance-review-playbook.md) before tuning.

| Date | Commit | Result | Index seconds | Ground seconds | Search seconds | Symbol seconds | Trail seconds | Snippet seconds | Nodes | Edges | Files | Index errors | Semantic docs | Search dir unchanged |
| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| 2026-04-18 | 2d6cc2c | pass | 171.97 | 0.09 | 0.84 | 0.09 | 0.07 | 0.06 | 25,500 | 21,622 | 122 | 0 | 10,205 | true |
| 2026-04-18 | c383227 | pass | 211.02 | 0.04 | 0.78 | 0.07 | 0.03 | 0.03 | 25,937 | 22,011 | 122 | 0 | 10,359 | true |
| 2026-04-18 | c524f1f | pass | 38.43 | 0.03 | 0.47 | 0.07 | 0.04 | 0.03 | 26,105 | 22,178 | 122 | 0 | 3,690 | true |
| 2026-04-19 | 6930933 | pass, semantic aliases schema v3 | 106.19 | 0.04 | 0.77 | 0.09 | 0.04 | 0.03 | 26,846 | 22,813 | 123 | 0 | 3,761 | true |
| 2026-04-19 | 4046f34 | pass, embedding research run 2 harness | 107.77 | 0.06 | 1.85 | 0.13 | 0.06 | 0.10 | 27,460 | 23,326 | 124 | 0 | 3,832 | true |
| 2026-04-19 | 33cb581 | pass, hash semantic check for delight QoL lane | 7.64 | 0.04 | 0.25 | 0.07 | 0.04 | 0.03 | 29,692 | 25,215 | 127 | 0 | 4,039 | true |
| 2026-04-20 | e1dc489 | pass, hash semantic check for embedding research lane | 8.31 | 0.04 | 0.24 | 0.08 | 0.04 | 0.03 | 29,840 | 25,331 | 127 | 0 | 4,055 | true |
| 2026-04-20 | b5c6337 | pass, delight roadmap implementation | 111.52 | 0.05 | 0.94 | 0.09 | 0.04 | 0.03 | 30,414 | 25,829 | 127 | 0 | 4,114 | true |
| 2026-05-07 | 0adcd43 | pass, hash semantic check for stdio MCP envelope fix | 11.01 | 0.20 | 0.45 | 0.19 | 0.14 | 0.14 | 39,087 | 33,167 | 141 | 0 | 5,410 | true |
| 2026-05-07 | a881f80 | pass, managed Vulkan embedding setup cold E2E | 51.45 | 0.18 | 0.60 | 0.20 | 0.15 | 0.14 | 40,064 | 33,971 | 146 | 0 | 5,548 | true |
| 2026-05-07 | faf0fa8 | pass, manual friction autoresearch loop | 121.13 | 0.20 | 0.56 | 0.21 | 0.17 | 0.15 | 40,631 | 34,379 | 147 | 0 | 5,615 | true |
| 2026-05-07 | c9a9552 | pass, intent-level manual friction closure | 148.20 | 0.23 | 0.64 | 0.25 | 0.19 | 0.17 | 41,033 | 34,708 | 147 | 0 | 5,658 | true |
| 2026-05-08 | 345fef5 | pass, branch review fixes | 59.07 | 0.18 | 0.48 | 0.20 | 0.17 | 0.16 | 41,126 | 34,784 | 145 | 0 | 5,703 | true |
| 2026-05-08 | 1457eb8 | pass, cache hotspot telemetry working tree | 89.03 | 0.18 | 0.52 | 0.21 | 0.15 | 0.15 | 41,494 | 35,103 | 145 | 0 | 5,736 | true |
| 2026-05-08 | f24bb2c | pass, managed ONNX review fixes hash e2e | 7.59 | 0.18 | 0.47 | 0.21 | 0.16 | 0.15 | 41,974 | 35,441 | 145 | 0 | 5,822 | true |
| 2026-05-20 | 3964d10 | pass, codestory 0.2.0 release | 8.91 | 0.19 | 0.82 | 0.53 | 0.18 | 0.17 | 43,937 | 37,086 | 145 | 0 | 6,028 | true |
| 2026-05-20 | fea0cc5 | pass, CLI navigation next wave | 15.75 | 0.31 | 1.51 | 0.82 | 0.31 | 0.30 | 46,347 | 39,169 | 145 | 0 | 6,263 | true |
| 2026-05-20 | 71a57a8 | pass, PR review clippy fix | 9.35 | 0.19 | 0.82 | 0.49 | 0.17 | 0.16 | 46,352 | 39,168 | 145 | 0 | 6,270 | true |
| 2026-05-22 | 0fb2a48 | pass, agent grounding review fixes | 10.61 | 0.21 | 0.96 | 0.61 | 0.21 | 0.20 | 50,006 | 42,246 | 146 | 0 | 6,720 | true |
| 2026-05-23 | de0dac9 | pass, agent grounding spec remediation working tree | 13.14 | 0.28 | 1.23 | 0.82 | 0.27 | 0.26 | 53,092 | 45,019 | 147 | 0 | 7,127 | true |
| 2026-05-24 | 7db7fb1+wt | pass, post-rebase benchmark/packet integration | 18.04 | 0.44 | 2.28 | 0.90 | 0.33 | 0.31 | 55,977 | 47,413 | 149 | 0 | 7,466 | true |
| 2026-05-24 | 7c891af+wt | pass, review remediation e2e | 11.10 | 0.29 | 1.29 | 0.86 | 0.25 | 0.23 | 56,272 | 47,628 | 149 | 0 | 7,501 | true |
| 2026-05-24 | 663c257+wt | pass, review findings remediation | 12.30 | 0.24 | 1.04 | 0.65 | 0.24 | 0.21 | 56,362 | 47,659 | 149 | 0 | 7,530 | true |
| 2026-05-24 | 3c62f1e+wt | pass, remove spec docs publish gate | 11.39 | 0.23 | 1.06 | 0.66 | 0.21 | 0.19 | 56,531 | 47,806 | 149 | 0 | 7,566 | true |
| 2026-05-25 | cba6cfe+wt | pass, packet planner local-real A/B checkpoint | 11.61 | 0.21 | 1.06 | 0.63 | 0.19 | 0.17 | 58,659 | 49,707 | 150 | 0 | 7,827 | true |
| 2026-05-25 | 49cd906+wt | pass, vscode packet holdout checkpoint | 11.88 | 0.28 | 1.30 | 0.77 | 0.21 | 0.18 | 58,726 | 49,773 | 150 | 0 | 7,834 | true |
| 2026-05-25 | 73fc42a+wt | pass, vscode cache freshness fix; drill manifest skipped | 11.00 | 0.21 | 1.08 | 0.68 | 0.19 | 0.19 | 58,782 | 49,824 | 150 | 0 | 7,847 | true |
| 2026-05-25 | 5aad799+wt | pass, projection cleanup FK fix; drill manifest skipped | 11.59 | 0.22 | 1.03 | 0.69 | 0.19 | 0.18 | 58,799 | 49,843 | 150 | 0 | 7,851 | true |
| 2026-05-25 | a6416ad+wt | pass, rust receiver chain drill bridge pass | 14.81 | 0.25 | 0.98 | 0.47 | 0.23 | 0.20 | 59,456 | 50,381 | 150 | 0 | 7,915 | true |
| 2026-05-25 | 765fe4b+wt | pass, owner-alias drill evidence and jobs coverage | 14.79 | 0.25 | 0.96 | 0.44 | 0.33 | 0.21 | 59,531 | 50,444 | 150 | 0 | 7,927 | true |
| 2026-05-25 | bce041a+wt | fail, drill search_plan missing before seed-anchor repair; targeted seed-anchor search repro passed after repair | 15.72 | 0.24 | 0.95 | 0.44 | 0.21 | 0.22 | 59,917 | 50,781 | 150 | 0 | 8,008 | true |
| 2026-06-01 | 7c4143f6+wt | pass, mandatory sidecar real-embedding e2e plus real drill manifest | 675.82 | 0.38 | 1.43 | 0.63 | 0.35 | 0.43 | 77,912 | 65,529 | 229 | 0 | 10,668 | true |
| 2026-06-01 | 2deff76e+wt | fail, release e2e stats ok; real drill manifest env missing | 685.56 | 0.34 | 1.29 | 0.54 | 0.32 | 0.31 | 78,795 | 66,280 | 229 | 0 | 10,771 | true |
| 2026-06-02 | 72d4ea4c+wt | pass, review remediation sidecar e2e; retrieval index 16.34s; drill manifest skipped | 746.30 | 0.31 | 1.14 | 0.46 | 0.24 | 0.22 | 78,247 | 66,075 | 217 | 0 | 10,787 | true |
| 2026-06-02 | 8f625b5e+wt | fail, release e2e stats ok; real drill manifest env missing; retrieval_index_seconds 18.15 | 1190.65 | 0.48 | 1.38 | 0.53 | 0.28 | 0.32 | 78,212 | 66,040 | 217 | 0 | 10,814 | true |
| 2026-06-02 | de6436a3+wt | pass, round 3 sidecar contract e2e; optional real drill manifest skipped; retrieval_index_seconds 18.13 | 929.37 | 0.31 | 1.48 | 0.52 | 0.30 | 0.25 | 78,159 | 65,970 | 217 | 0 | 10,806 | true |
| 2026-06-02 | dbba955b+wt | pass, round 4 sidecar contract e2e; optional real drill manifest skipped; retrieval_index_seconds 16.02 | 874.56 | 0.29 | 1.15 | 0.46 | 0.26 | 0.24 | 78,203 | 66,005 | 217 | 0 | 10,814 | true |
| 2026-06-02 | 3c3012af+wt | pass, round 6 sidecar cache/status e2e; optional real drill manifest skipped; retrieval_index_seconds 20.58; retrieval_mode full | 890.52 | 0.34 | 1.91 | 0.63 | 0.31 | 0.29 | 78,376 | 66,156 | 217 | 0 | 10,836 | true |
| 2026-06-02 | 4c616548+wt | blocked, round 7 release e2e index phase did not complete; stopped child after 1075.05s with no stdout/stderr; failed command `index --refresh full --format json`; retrieval_index_seconds n/a; retrieval_mode n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a |
| 2026-06-02 | 25751a39+wt | fail, round 8 release e2e stats ok; real drill manifest env missing fail-closed; retrieval_index_seconds 17.73; retrieval_status_seconds 0.46; retrieval_mode full | 720.80 | 0.31 | 1.54 | 0.52 | 0.26 | 0.26 | 78,478 | 66,235 | 217 | 0 | 10,839 | true |
| 2026-06-02 | a23770f+wt | pass, round 9 stats-only release e2e; real drill intentionally skipped with CODESTORY_ALLOW_SKIP_REAL_REPO_DRILL_CASES=1; not real-drill release evidence; retrieval_index_seconds 18.35; retrieval_status_seconds 0.56; retrieval_mode full | 711.31 | 0.32 | 1.77 | 0.59 | 0.32 | 0.27 | 78,582 | 66,332 | 217 | 0 | 10,847 | true |
| 2026-06-05 | 42089cc5+wt | pass, stats-only retrieval rollout proof guidance plus strict sidecar markdown freshness fix; real drill intentionally skipped with CODESTORY_ALLOW_SKIP_REAL_REPO_DRILL_CASES=1; retrieval_index_seconds 21.57; retrieval_status_seconds 0.96; retrieval_mode full | 981.56 | 0.50 | 2.94 | 0.54 | 0.34 | 0.26 | 79,028 | 66,731 | 217 | 0 | 10,881 | true |
| 2026-06-08 | 9387e9e3 | pass, proof readiness 0.6.2 full-sidecar stats; proof_tier full_sidecar; warnings index_seconds>600 and semantic_phase_seconds>500; real drill not run because CODESTORY_REAL_REPO_DRILL_CASES was missing; retrieval_index_seconds 18.13; retrieval_status_seconds 1.28; retrieval_mode full | 791.43 | 0.39 | 3.46 | 0.49 | 0.27 | 0.35 | 79,779 | 67,446 | 217 | 0 | 11,049 | true |
| 2026-06-10 | a88705f2 | pass, clean main baseline same-machine full-sidecar stats from detached worktree; warnings index_seconds>600 and semantic_phase_seconds>500; retrieval_index_seconds 26.44; retrieval_mode full | 1238.23 | 0.44 | 4.33 | 0.93 | 0.40 | 0.37 | 80,734 | 68,163 | 220 | 0 | 11,178 | true |
| 2026-06-10 | a88705f2+wt | pass, AST-first graph_first_v1 full-sidecar stats; symbol_search_docs 11,315; dense anchors 693; semantic_embedding_ms 43.23s; repeat full refresh 22.75s with 0 embedded; retrieval_index_seconds 7.53; retrieval_mode full | 67.34 | 0.21 | 2.11 | 0.54 | 0.22 | 0.20 | 82,219 | 69,489 | 220 | 0 | 693 | true |
| 2026-06-11 | a88705f2+wt | AST-first graph_first_v1 sampled release e2e; symbol_search_docs 11,336; dense anchors 693; dense skips 10,643; semantic_embedding_ms 48.52s; retrieval_index_seconds 7.31; retrieval_mode full; repeat full refresh 21.39s with 0 embedded; peak descendant 304.93 MB at target/memory-measure/ast-first-release-e2e-v6/summary.json | 67.97 | 0.22 | 2.24 | 0.58 | 0.24 | 0.22 | 82,510 | 69,766 | 220 | 0 | 693 | true |
| 2026-06-11 | a88705f2+wt | final AST-first graph_first_v1 sampled release e2e after drill sidecar finalizer; symbol_search_docs 11,336; dense anchors 693; dense skips 10,643; semantic_embedding_ms 48.83s; retrieval_index_seconds 6.54; retrieval_mode full; repeat full refresh 21.39s with 0 embedded; peak descendant 318.35 MB at target/memory-measure/ast-first-release-e2e-v9/summary.json | 69.18 | 0.26 | 2.38 | 0.56 | 0.24 | 0.23 | 82,528 | 69,784 | 220 | 0 | 693 | true |
| 2026-06-11 | 376df0c8+wt | readiness/handoff and Unix compatibility release e2e; proof_tier full_sidecar; warnings none; real drill intentionally skipped with CODESTORY_ALLOW_SKIP_REAL_REPO_DRILL_CASES=1; symbol_search_docs 11,505; dense anchors 708; dense skips 10,797; semantic_embedding_ms 48.89s; retrieval_index_seconds 10.95; retrieval_mode full; repeat full refresh 20.56s with 0 embedded | 68.23 | 0.22 | 2.27 | 0.54 | 0.22 | 0.20 | 83,735 | 70,803 | 222 | 0 | 708 | true |

## Repeat And Report Timing

New `codestory_repo_e2e_stats` runs emit `repeat_full_refresh_seconds`,
`report_seconds`, and nested `report.markdown_seconds` / `report.json_seconds`.
Append the measurement row here when running the release harness.

| Date | Commit | Scenario | Repeat full refresh seconds | Report seconds | Report markdown seconds | Report JSON seconds |
| --- | --- | --- | ---: | ---: | ---: | ---: |
| 2026-06-11 | 376df0c8+wt | readiness/handoff and Unix compatibility release e2e; proof_tier full_sidecar; real drill skipped with CODESTORY_ALLOW_SKIP_REAL_REPO_DRILL_CASES=1 | 20.56 | 2.59 | 1.09 | 1.50 |

## Phase Metrics

| Date | Commit | Scenario | Index seconds | Graph phase seconds | Semantic phase seconds | Semantic docs reused | Semantic docs embedded | Semantic docs stale |
| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 2026-04-18 | c383227 | fresh temp cache E2E | 211.02 | 3.21 | 201.66 | 0 | 10,359 | 0 |
| 2026-04-18 | c383227 | repeat full refresh on default cache | 9.16 | 2.82 | 0.07 | 10,360 | 0 | 0 |
| 2026-04-18 | c524f1f | durable semantic cold E2E | 38.43 | 2.92 | 32.07 | 0 | 3,690 | 0 |
| 2026-04-18 | c524f1f | durable repeat full refresh | 7.56 | 3.25 | 0.12 | 3,690 | 0 | 0 |
| 2026-04-19 | 6930933 | semantic aliases schema v3 cold E2E | 106.19 | 2.88 | 99.44 | 0 | 3,761 | 0 |
| 2026-04-19 | 4046f34 | embedding research run 2 harness cold E2E | 107.77 | 2.90 | 100.80 | 0 | 3,832 | 0 |
| 2026-04-19 | 33cb581 | hash semantic check for delight QoL lane cold E2E | 7.64 | 3.21 | 0.89 | 0 | 4,039 | 0 |
| 2026-04-20 | e1dc489 | hash semantic check for embedding research lane cold E2E | 8.31 | 3.27 | 0.92 | 0 | 4,055 | 0 |
| 2026-04-20 | b5c6337 | delight roadmap implementation cold E2E | 111.52 | 3.07 | 103.66 | 0 | 4,114 | 0 |
| 2026-05-07 | 0adcd43 | hash semantic check for stdio MCP envelope fix cold E2E | 11.01 | 4.47 | 1.60 | 0 | 5,410 | 0 |
| 2026-05-07 | a881f80 | managed Vulkan embedding setup cold E2E | 51.45 | 4.48 | 40.19 | 0 | 5,548 | 0 |
| 2026-05-07 | faf0fa8 | manual friction autoresearch loop cold E2E | 121.13 | 4.12 | 111.89 | 0 | 5,615 | 0 |
| 2026-05-07 | c9a9552 | intent-level manual friction closure cold E2E | 148.20 | 5.28 | 137.35 | 0 | 5,658 | 0 |
| 2026-05-08 | 345fef5 | branch review fixes cold E2E | 59.07 | 4.95 | 47.39 | 0 | 5,703 | 0 |
| 2026-05-08 | 1457eb8 | cache hotspot telemetry cold E2E | 89.03 | 3.12 | 82.21 | 0 | 5,736 | 0 |
| 2026-05-08 | f24bb2c | managed ONNX review fixes hash E2E | 7.59 | 2.93 | 0.46 | 0 | 5,822 | 0 |
| 2026-05-20 | 3964d10 | codestory 0.2.0 release E2E | 8.91 | 3.42 | 0.65 | 0 | 6,028 | 0 |
| 2026-05-20 | fea0cc5 | CLI navigation next wave cold E2E | 15.75 | 6.23 | 1.74 | 0 | 6,263 | 0 |
| 2026-05-20 | 71a57a8 | PR review clippy fix cold E2E | 9.35 | 3.80 | 0.95 | 0 | 6,270 | 0 |
| 2026-05-22 | 0fb2a48 | agent grounding review fixes cold E2E | 10.61 | 4.25 | 0.72 | 0 | 6,720 | 0 |
| 2026-05-23 | de0dac9 | agent grounding spec remediation cold E2E | 13.14 | 5.73 | 0.81 | 0 | 7,127 | 0 |
| 2026-05-24 | 7db7fb1+wt | post-rebase benchmark/packet integration E2E | 18.04 | 5.38 | 1.69 | 0 | 7,466 | 0 |
| 2026-05-24 | 7c891af+wt | review remediation E2E | 11.10 | 5.12 | 0.79 | 0 | 7,501 | 0 |
| 2026-05-24 | 3c62f1e+wt | remove spec docs publish gate E2E | 11.39 | 5.14 | 0.70 | 0 | 7,566 | 0 |
| 2026-05-25 | cba6cfe+wt | packet planner local-real A/B checkpoint E2E | 11.61 | 4.99 | 0.80 | 0 | 7,827 | 0 |
| 2026-05-25 | 49cd906+wt | vscode packet holdout checkpoint E2E | 11.88 | 5.27 | 0.81 | 0 | 7,834 | 0 |
| 2026-05-25 | 73fc42a+wt | vscode cache freshness fix E2E | 11.00 | 5.20 | 0.94 | 0 | 7,847 | 0 |
| 2026-05-25 | 5aad799+wt | projection cleanup FK fix E2E | 11.59 | 5.26 | 1.18 | 0 | 7,851 | 0 |
| 2026-05-25 | a6416ad+wt | rust receiver chain drill bridge E2E | 14.81 | 7.47 | 1.61 | 0 | 7,915 | 0 |
| 2026-05-25 | 765fe4b+wt | owner-alias drill evidence and jobs coverage E2E | 14.79 | 7.79 | 0.93 | 0 | 7,927 | 0 |
| 2026-05-25 | bce041a+wt | semantic role-awareness E2E release half; drill half failed before seed-anchor repair | 15.72 | 7.94 | 0.98 | 0 | 8,008 | 0 |
| 2026-06-01 | 7c4143f6+wt | mandatory sidecar real-embedding e2e with real drill manifest | 675.82 | 9.19 | 643.68 | 0 | 10,668 | 0 |
| 2026-06-01 | 2deff76e+wt | segment 70 stage probes release e2e; drill manifest env missing | 685.56 | 8.61 | 654.37 | 0 | 10,771 | 0 |
| 2026-06-02 | 72d4ea4c+wt | review remediation sidecar e2e; retrieval index 16.34s; drill manifest skipped | 746.30 | 10.40 | 727.80 | 0 | 10,787 | 0 |
| 2026-06-02 | 8f625b5e+wt | review remediation stats pass; drill failed CODESTORY_REAL_REPO_DRILL_CASES missing; retrieval_index_seconds 18.15 | 1190.65 | 9.77 | 1172.38 | 0 | 10,814 | 0 |
| 2026-06-02 | de6436a3+wt | round 3 sidecar contract e2e; optional real drill manifest skipped; retrieval_index_seconds 18.13 | 929.37 | 11.74 | 906.29 | 0 | 10,806 | 0 |
| 2026-06-02 | dbba955b+wt | round 4 sidecar contract e2e; optional real drill manifest skipped; retrieval_index_seconds 16.02 | 874.56 | 11.09 | 854.21 | 0 | 10,814 | 0 |
| 2026-06-02 | b582a9bb+wt | round 5 sidecar contract e2e; optional real drill manifest skipped; retrieval_index_seconds 19.75; retrieval_mode full | 917.01 | 10.30 | 897.84 | 0 | 10,826 | 0 |
| 2026-06-02 | 3c3012af+wt | round 6 sidecar cache/status e2e; optional real drill manifest skipped; retrieval_index_seconds 20.58; retrieval_mode full | 890.52 | 12.95 | 866.79 | 0 | 10,836 | 0 |
| 2026-06-02 | 4c616548+wt | round 7 blocked before phase metrics; release index child stopped after 1075.05s with no stdout/stderr; retrieval_index_seconds n/a; retrieval_mode n/a | n/a | n/a | n/a | n/a | n/a | n/a |
| 2026-06-02 | 25751a39+wt | round 8 release e2e stats ok; real drill manifest env missing fail-closed; retrieval_index_seconds 17.73; retrieval_status_seconds 0.46; retrieval_mode full | 720.80 | 10.27 | 702.18 | 0 | 10,839 | 0 |
| 2026-06-02 | a23770f+wt | round 9 stats-only release e2e; real drill intentionally skipped with CODESTORY_ALLOW_SKIP_REAL_REPO_DRILL_CASES=1; not real-drill release evidence; retrieval_index_seconds 18.35; retrieval_mode full | 711.31 | 11.08 | 691.07 | 0 | 10,847 | 0 |
| 2026-06-05 | 42089cc5+wt | stats-only retrieval rollout proof guidance plus strict sidecar markdown freshness fix; real drill intentionally skipped with CODESTORY_ALLOW_SKIP_REAL_REPO_DRILL_CASES=1; retrieval_index_seconds 21.57; retrieval_mode full | 981.56 | 9.67 | 963.51 | 0 | 10,881 | 0 |
| 2026-06-08 | 9387e9e3 | proof readiness 0.6.2 full-sidecar stats; proof_tier full_sidecar; warnings index_seconds>600 and semantic_phase_seconds>500; real drill not run because CODESTORY_REAL_REPO_DRILL_CASES was missing; retrieval_index_seconds 18.13; retrieval_mode full | 791.43 | 9.73 | 772.72 | 0 | 11,049 | 0 |
| 2026-06-10 | a88705f2 | clean main baseline same-machine full-sidecar stats from detached worktree; warnings index_seconds>600 and semantic_phase_seconds>500; retrieval_index_seconds 26.44; retrieval_mode full | 1238.23 | 13.61 | 1211.82 | 0 | 11,178 | 0 |
| 2026-06-10 | a88705f2+wt | AST-first graph_first_v1 full-sidecar stats; symbol_search_docs 11,315; dense anchors 693; dense skips 10,622; reasons public_api 643, entrypoint 5, central_graph_node 36, component_report 9; repeat full refresh 22.75s with 0 embedded | 67.34 | 13.16 | 43.98 | 0 | 693 | 0 |
| 2026-06-11 | 376df0c8+wt | readiness/handoff and Unix compatibility release e2e; proof_tier full_sidecar; real drill skipped with CODESTORY_ALLOW_SKIP_REAL_REPO_DRILL_CASES=1; symbol_search_docs 11,505; dense anchors 708; dense skips 10,797; reasons public_api 656, entrypoint 5, central_graph_node 38, component_report 9 | 68.23 | 10.11 | 49.85 | 0 | 708 | 0 |
