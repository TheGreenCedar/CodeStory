# Codestory E2E Stats Log

Append one entry before each commit after running:

```powershell
cargo build --release -p codestory-cli
cargo test -p codestory-cli --test codestory_repo_e2e_stats -- --ignored --nocapture
```

Keep the full emitted JSON in the test output when reviewing locally, and add the headline metrics here so search/index reuse trends are visible over time.

| Date | Commit | Result | Index seconds | Ground seconds | Search seconds | Symbol seconds | Trail seconds | Snippet seconds | Nodes | Edges | Files | Index errors | Semantic docs | Search dir unchanged |
| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| 2026-04-18 | 2d6cc2c | pass | 171.97 | 0.09 | 0.84 | 0.09 | 0.07 | 0.06 | 25,500 | 21,622 | 122 | 0 | 10,205 | true |
| 2026-04-18 | c383227 | pass | 211.02 | 0.04 | 0.78 | 0.07 | 0.03 | 0.03 | 25,937 | 22,011 | 122 | 0 | 10,359 | true |
| 2026-04-18 | c524f1f | pass | 38.43 | 0.03 | 0.47 | 0.07 | 0.04 | 0.03 | 26,105 | 22,178 | 122 | 0 | 3,690 | true |
| 2026-04-19 | 6930933 | pass, semantic aliases schema v3 | 106.19 | 0.04 | 0.77 | 0.09 | 0.04 | 0.03 | 26,846 | 22,813 | 123 | 0 | 3,761 | true |

## Phase Metrics

| Date | Commit | Scenario | Index seconds | Graph phase seconds | Semantic phase seconds | Semantic docs reused | Semantic docs embedded | Semantic docs stale |
| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 2026-04-18 | c383227 | fresh temp cache E2E | 211.02 | 3.21 | 201.66 | 0 | 10,359 | 0 |
| 2026-04-18 | c383227 | repeat full refresh on default cache | 9.16 | 2.82 | 0.07 | 10,360 | 0 | 0 |
| 2026-04-18 | c524f1f | durable semantic cold E2E | 38.43 | 2.92 | 32.07 | 0 | 3,690 | 0 |
| 2026-04-18 | c524f1f | durable repeat full refresh | 7.56 | 3.25 | 0.12 | 3,690 | 0 | 0 |
| 2026-04-19 | 6930933 | semantic aliases schema v3 cold E2E | 106.19 | 2.88 | 99.44 | 0 | 3,761 | 0 |
