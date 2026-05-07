# CodeStory Stdio Warm Loop Stats

This log tracks the persistent `serve --stdio` path that agents should prefer once an index already exists. It complements `codestory-e2e-stats-log.md`, which tracks cold one-shot CLI timings.

Run after building the release CLI:

```powershell
cargo build --release -p codestory-cli
cargo test -p codestory-cli --test stdio_warm_loop_stats -- --ignored --nocapture
```

The harness prints metrics from the test process after the stdio server exits. The server stdout remains protocol-only: one JSON-RPC response per line, with no benchmark text mixed into the protocol stream.

| Date | Commit | Scenario | Result | Reps | Startup ms | Tools/list ms | First search ms | Cold one-loop ms | Warm total ms | Warm per-loop ms | Warm/cold per-loop ratio | Search p50/p95/p99 ms | Symbol p50/p95/p99 ms | Trail p50/p95/p99 ms | Snippet p50/p95/p99 ms | Status p50/p95/p99 ms | Index semantic reload ms | Warm stdio semantic reload ms | Fallback reason | Warm search dir unchanged | Protocol stdout only |
| --- | --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- | --- | --- | --- | --- | ---: | --- | --- | --- | --- |
| 2026-05-06 | pending | small fixture, release binary, hash embeddings | pass | 20 | 25.09 | 1.56 | 25.96 | 169.29 | 1070.03 | 53.50 | 0.32 | 20.84/25.96/25.96 | 15.01/17.67/17.67 | 10.25/13.92/13.92 | 6.50/8.36/8.36 | 6.79/13.17/13.17 | 0 | null | null | true | true |

## Current Promotion Budget

No hard warm p95 promotion budget is approved yet. The 2026-05-06 baseline is a
small-fixture release-binary smoke, not web-cockpit promotion evidence. The
browser surface gate stays closed until this section names a per-tool or
per-loop p95 budget, target machine class, and passing current run.

## Baseline Payload Sizes

From the 2026-05-06 baseline:

| Operation | Samples | p50 bytes | max bytes |
| --- | ---: | ---: | ---: |
| search | 20 | 10,700 | 10,700 |
| symbol | 20 | 1,812 | 1,812 |
| trail | 20 | 6,523 | 6,523 |
| snippet | 20 | 744 | 744 |
| resources/read:status | 20 | 1,003 | 1,003 |

## Notes

- The baseline is a small-fixture release-binary smoke, not a repo-scale promotion gate.
- Response bytes are run-local smoke metrics because temp paths appear in JSON payloads.
- `warm per-loop ms` covers `search -> symbol -> trail -> snippet`; `resources/read codestory://status` is measured separately because it is a health check, not part of the cold one-shot comparison.
- `warm stdio semantic reload ms` is `null` because `serve --stdio` does not currently expose a dedicated semantic reload phase; any warm-server load cost is included in `startup ms`.
- Add hard latency budgets only after several local runs establish variance.
