# CodeStory Stdio Warm Loop Stats

**Audience:** Evidence record — not an install guide.

This log tracks the persistent `serve --stdio` path that agents should prefer once an index already exists. It complements `codestory-e2e-stats-log.md`, which tracks cold one-shot CLI timings.

Run after building the release CLI:

```sh
export CODESTORY_EMBED_MODEL_SOURCE="$(node scripts/prepare-embedded-model.mjs)"
cargo build --release -p codestory-cli
cargo test -p codestory-cli --test stdio_warm_loop_stats -- --ignored --nocapture
```

The harness prints metrics from the test process after the stdio server exits. The server stdout remains protocol-only: one JSON-RPC response per line, with no benchmark text mixed into the protocol stream.

| Date | Commit | Scenario | Result | Reps | Startup ms | Tools/list ms | First search ms | Cold one-loop ms | Warm total ms | Warm per-loop ms | Warm/cold per-loop ratio | Search p50/p95/p99 ms | Symbol p50/p95/p99 ms | Trail p50/p95/p99 ms | Snippet p50/p95/p99 ms | Retrieval fingerprint/status p50/p95/p99 ms | Index semantic reload ms | Warm stdio semantic reload ms | Fallback reason | Warm search dir unchanged | Protocol stdout only |
| --- | --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- | --- | --- | --- | --- | ---: | --- | --- | --- | --- |
| 2026-05-06 | pending | small fixture, release binary, hash embeddings | pass | 20 | 25.09 | 1.56 | 25.96 | 169.29 | 1070.03 | 53.50 | 0.32 | 20.84/25.96/25.96 | 15.01/17.67/17.67 | 10.25/13.92/13.92 | 6.50/8.36/8.36 | 6.79/13.17/13.17 | 0 | null | null | true | true |

## Current Promotion Budget

The active budget has two tiers.

### Smoke Budget

The small-fixture release-binary warm loop must stay comfortably below these
p95 limits:

| Operation | p95 Budget |
| --- | ---: |
| search | 75 ms |
| symbol | 50 ms |
| trail | 75 ms |
| snippet | 50 ms |
| resources/read:status | 50 ms |
| full `search -> symbol -> trail -> snippet` loop | 250 ms |

The 2026-05-06 baseline passes this smoke budget, but it remains a
small-fixture smoke, not separate web UI promotion evidence.

### Web UI Promotion Budget

Before starting or promoting a separate web UI, record a current warm run
against CodeStory itself or another representative real repository on the target
machine class. The run must meet:

| Scope | p95 Budget |
| --- | ---: |
| each default read operation | 500 ms |
| full `search -> symbol -> trail -> snippet` loop | 1.5 s |

The browser surface gate stays closed until a current real-repo run in this log
meets the Web UI Promotion Budget, and the stress-lane gate for the target
scale also passes.

## Browser Surface Gate

`explore` and `serve --stdio` are the current browser-capable read surfaces. Do
not promote a new `browse` command, web UI route, or browser-specific UI until
the repo has current evidence for each gate:

- Tool, resource, and prompt manifests are stable under stdio catalog tests.
- HTTP and stdio browser contracts stay aligned with the read-only browser
  service.
- Warm stdio/browser-loop p50, p95, and p99 timings are recorded here: small
  fixture smoke p95 stays under the Smoke Budget, and a current real-repo run
  meets the Web UI Promotion Budget.
- Browser stress lanes pass at the intended scale, and synthetic evidence is not
  treated as real-repository promotion proof.
- `explore` demonstrates the browser workflow in JSON/Markdown and
  keyboard-first TUI paths.
- Screenshot-visible review is planned before implementation, with one reviewer
  for the full viewport and one reviewer for the changed surface or acceptance
  path.

Evidence sources: `crates/codestory-cli/tests/stdio_protocol_contracts.rs`,
`crates/codestory-cli/tests/http_transport_contracts.rs`,
`crates/codestory-cli/tests/stdio_warm_loop_stats.rs`,
`docs/testing/codestory-stdio-warm-loop-stats.md`,
`docs/testing/codestory-stress-lanes.md`, and
`crates/codestory-cli/tests/cli_golden_path.rs`.

If the gates are satisfied, start with a written implementation plan that names
why the new surface is not a duplicate of `explore`, the exact routes or
commands to add, the screenshot-visible review loop, and the promotion guard
path.

## Baseline Payload Sizes

From the 2026-05-06 baseline:

| Operation | Samples | p50 bytes | max bytes |
| --- | ---: | ---: | ---: |
| search | 20 | 10,700 | 10,700 |
| symbol | 20 | 1,812 | 1,812 |
| trail | 20 | 6,523 | 6,523 |
| snippet | 20 | 744 | 744 |
| resources/read:status | 20 | 1,003 | 1,003 |

## Packet Cache Probe

`serve --stdio` keeps a small in-process LRU for identical successful `packet`
requests. The key includes request arguments plus the SQLite DB/WAL fingerprint,
so a changed index bypasses the cached packet.

| Date | Commit | Scenario | First packet ms | Repeated packet ms | Speedup | Same packet id | Trace steps | Protocol stderr |
| --- | --- | --- | ---: | ---: | ---: | --- | ---: | ---: |
| 2026-05-25 | pending | CodeStory repo, release binary, `--refresh none`, repeated identical tiny packet | 3495.60 | 0.93 | 3754.27x | true | 13 | 0 bytes |

## Notes

- The baseline is a small-fixture release-binary smoke, not a repo-scale promotion gate.
- Response bytes are run-local smoke metrics because temp paths appear in JSON payloads.
- `warm per-loop ms` covers `search -> symbol -> trail -> snippet`; the
  project-bound `resources/read codestory://status{?project}` call is measured
  separately as `retrieval_status` because it includes the mandatory retrieval
  fingerprint/status check, not the cold one-shot comparison.
- `warm stdio semantic reload ms` is `null` because `serve --stdio` does not currently expose a dedicated semantic reload phase; any warm-server load cost is included in `startup ms`.
- Add hard latency budgets only after several local runs establish variance.
