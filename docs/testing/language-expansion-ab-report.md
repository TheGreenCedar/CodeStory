# Language Expansion A/B Report

Date: 2026-06-13

## Verdict

Production runtime defaults do not enable exact benchmark-family steering. Rows
that used `CODESTORY_EVAL_PROBES=1` are eval-only diagnostics and are not
promotion evidence.

The benchmark harness now measures the right A/B shape: a strict no-CodeStory
local baseline against a CodeStory-first arm, with wall time, token usage, tool
calls, command categories, source reads, web/search leakage, packet quality,
post-packet source reads, and manifest quality scored from recorded artifacts.

The honest result is still mixed. The latest full 18-language paired A/B
artifact is not a promotion win: CodeStory passed more quality rows than the
no-CodeStory baseline (`9/18` versus `7/18`) and used fewer tool
calls/commands (`305` versus `519`), but it used more total tokens
(`13,060,265` versus `8,191,771`), more runner wall time (`4,014,646 ms`
versus `3,094,988 ms`), and more all-in wall time after cache preparation
(`4,796,792 ms` versus `3,094,988 ms`). Packet manifest quality passed only
`7/18` CodeStory rows in that older full paired run.

The current no-hidden-steering packet baseline is better but still partial.
With production-default packet behavior plus explicit manifest-derived probes
and generic source-shape claims, the packet gate quality-passes `9/18` language
rows. That is the current generalized packet baseline. It is not broad
18-language proof.

The current packet-eligible A/B slice is a real win inside that narrower gate:
CodeStory passed `9/9` rows versus `6/9` for the strict no-CodeStory baseline,
with no post-packet source reads and no web searches. It used `291,788` tokens
versus `5,346,265`, `502,289 ms` all-in wall time versus `1,881,683 ms`, and
`9` tool calls/commands versus `282`. This proves the packet-eligible slice is
useful; it does not prove the remaining nine languages.

## Current Baseline

| Evidence slice | Status | Key result |
| --- | --- | --- |
| Full 18-language paired A/B | Historical, not promotion evidence | CodeStory quality `9/18` vs baseline `7/18`, but worse token and wall-time cost |
| Production-default packet gate | Current generalized packet baseline | `9/18` rows pass packet manifest quality; Java and Redis still miss packet latency SLA |
| Packet-eligible paired A/B | Current narrow win | CodeStory `9/9` quality vs baseline `6/9`, much lower tokens and commands |
| Eval-probe rows | Diagnostics only | Useful for debugging exact families, not promotion evidence |

Current packet quality pass set:

- `python-requests-session-flow`
- `java-commons-lang-string-utils`
- `rust-ripgrep-search-pipeline`
- `typescript-swr-hook-flow`
- `c-redis-command-loop`
- `go-gin-route-dispatch`
- `dart-http-client-flow`
- `bash-nvm-install-dispatch`
- `css-animate-base-and-keyframes`

Current packet quality fail set:

- `javascript-express-routing-flow`
- `cpp-fmt-formatting-flow`
- `ruby-jekyll-site-build`
- `php-monolog-record-flow`
- `csharp-automapper-map-flow`
- `kotlin-okio-buffer-flow`
- `swift-alamofire-request-flow`
- `html-mdn-form-validation`
- `sql-chinook-schema-relations`

Important caveats:

- Some passing packet rows are still generically `partial` even though manifest
  quality passes.
- Java broadened the pass set but made the 9-row aggregate A/B gap worse than
  the prior 8-row slice.
- Redis, Rust, Bash, and Dart have remaining citation or expected-claim recall
  caveats inside otherwise passing rows.
- The packet probe retry path recovered transient sidecar failures in earlier
  higher-concurrency runs; keep that reliability path covered before raising
  packet-probe concurrency.

## Durable Surfaces

Scripts and manifests that should remain maintained:

- `scripts/codestory-agent-ab-benchmark.mjs`
- `scripts/codestory-agent-ab-score.mjs`
- `scripts/codestory-agent-ab-analyzer.mjs`
- `scripts/codestory-language-holdout-integrity.mjs`
- `scripts/tests/codestory-agent-ab-analyzer.test.mjs`
- `benchmarks/tasks/language-expansion-holdout/language-support-ab.task.json`
- `benchmarks/tasks/language-expansion-holdout/repos.json`
- `docs/testing/oss-language-corpus.md`

Artifact policy:

- Keep durable conclusions in this report.
- Keep raw benchmark artifacts under `target/agent-benchmark/` for local
  forensics, but do not paste long local run catalogs into this document.
- Keep `summary.json`, `reanalyzed-summary.json`, packet quality summaries, and
  transcript-derived metrics as the authoritative raw evidence for a run.
- Treat exact family steering, static family citations, and eval probes as
  diagnostics unless a report explicitly marks them as excluded from promotion
  evidence.

## Reproduction Commands

Validate the recorded holdout/corpus shape without rerunning indexing:

```powershell
node scripts\codestory-language-holdout-integrity.mjs
```

Run harness self-checks:

```powershell
node --test scripts\tests\codestory-agent-ab-analyzer.test.mjs
node scripts\codestory-agent-ab-benchmark.mjs --self-test
node --check scripts\codestory-agent-ab-score.mjs
node --check scripts\codestory-agent-ab-benchmark.mjs
```

Run a fresh one-repeat full paired A/B suite:

```powershell
node scripts\codestory-agent-ab-benchmark.mjs `
  --task-suite language-expansion-holdout `
  --repeats 1 `
  --repo-cache-dir target\oss-language-corpus\repos `
  --materialize-repos `
  --prepare-codestory-cache `
  --jobs 4 `
  --prepare-codestory-jobs 2 `
  --out-dir target\agent-benchmark\language-expansion-current `
  --timeout-ms 600000 `
  --prepare-codestory-timeout-ms 1800000 `
  --allow-failures
```

Reanalyze an existing run:

```powershell
node scripts\codestory-agent-ab-benchmark.mjs `
  --reanalyze-dir target\agent-benchmark\language-expansion-current `
  --task-suite language-expansion-holdout `
  --repo-cache-dir target\oss-language-corpus\repos `
  --materialize-repos
```

Run a packet-gated A/B selection from a prepared run:

```powershell
node scripts\codestory-agent-ab-score.mjs `
  --packet-gate `
  --packet-probe-jobs 1 `
  --task-ids python-requests-session-flow,rust-ripgrep-search-pipeline,typescript-swr-hook-flow,c-redis-command-loop,go-gin-route-dispatch,dart-http-client-flow,bash-nvm-install-dispatch,java-commons-lang-string-utils,css-animate-base-and-keyframes `
  --repeats 1 `
  --reuse-baseline-from target\agent-benchmark\language-expansion-current `
  --out-dir target\agent-benchmark\language-expansion-packet-eligible `
  --jobs 1 `
  --prepare-codestory-jobs 1 `
  --prepare-codestory-timeout-ms 1800000 `
  --timeout-ms 600000
```

Run eval-only exact-family diagnostics when debugging a row-specific probe:

```powershell
$env:CODESTORY_EVAL_PROBES = "1"
# Run the narrow diagnostic command.
Remove-Item Env:CODESTORY_EVAL_PROBES
```

Do not use eval-only rows as promotion evidence.

## Promotion Blockers

- Raise production-default packet manifest quality beyond the current `9/18`
  pass rate without restoring hidden exact-family steering.
- Fix the remaining packet quality failures for JavaScript, C++, Ruby, PHP, C#,
  Kotlin, Swift, HTML, and SQL.
- Fix packet latency; the latest clean serial gate still misses the `18,000 ms`
  retrieval target on Java and Redis.
- Replace row-specific detectors with generic structural claim layers selected
  from code evidence, not repository names.
- Keep no-CodeStory baselines strict: they must inspect the local repository,
  avoid CodeStory tools, avoid web/search leakage, and match the current task
  manifest snapshot.
- Run a fresh full 18-language paired A/B suite only after packet quality is
  materially better, then repeat at least 3 times before claiming promotion.
- Promote only after packet-first and no-CodeStory-baseline gates pass with
  clean pinned checkout provenance, local-only CodeStory cache provenance, no
  hidden eval steering, and no web/remote context blockers.
