# Codestory E2E Stats Log

Append one entry before each commit after running:

```powershell
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
