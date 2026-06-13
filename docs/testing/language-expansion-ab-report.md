# Language Expansion A/B Report

Date: 2026-06-13

## Verdict

The language-expansion evidence is useful, but it is not broad promotion proof.

The strongest current result is a narrow packet-eligible A/B slice: CodeStory
beats the strict no-CodeStory baseline on quality, tokens, commands, and wall
time for nine selected rows. The broader 18-language packet runtime artifact now
passes manifest quality for all 18 rows, but only 6 rows are packet-sufficient
without follow-up commands and two rows still miss the packet retrieval latency
SLA. The older full 18-language paired A/B run is explicitly not a promotion
win because CodeStory quality improved only modestly while total tokens and wall
time regressed.

Do not turn this report into a headline claim that every supported language is
first-class. It proves that the harness and packet path can measure the right
questions, and it identifies the next cleanup targets. It does not prove a
generalized, production-safe, 18-language win.

## Evidence Ledger

| Slice | Raw evidence | Result | Use it for |
| --- | --- | --- | --- |
| Full 18-language paired A/B | `target/agent-benchmark/segment6-full-language-suite-r1-pathfix/reanalyzed-summary.json` and `.md` | CodeStory quality `9/18`; no-CodeStory quality `7/17` scored with one unsuccessful row. CodeStory used `13,060,265` tokens vs `8,191,771`, `4,014,646 ms` runner wall vs `3,094,988 ms`, and `4,796,792 ms` all-in wall vs `3,094,988 ms`. | Historical negative/diagnostic evidence. |
| 18-language packet runtime | `target/agent-benchmark/segment9-generic-18lang-packet-final/packet-runtime-summary.json`, `packet-runtime-summary.md`, `packet-composition.md`, and `quality-debug.json` | Manifest quality passes `18/18`; packet sufficiency is only `6/18`. Java and Redis miss the `18,000 ms` packet retrieval SLA. | Current packet quality and sufficiency baseline. |
| Packet-eligible paired A/B | `target/agent-benchmark/segment8-no-family-steering-current9-ab-java-css-generic-shapes/reanalyzed-summary.json` and `.md` | CodeStory quality `9/9` vs no-CodeStory `6/9`; CodeStory uses `291,788` tokens vs `5,346,265`, `502,289 ms` all-in wall vs `1,881,683 ms`, `9` commands vs `282`, and zero source reads vs `228`. | Narrow positive evidence for the rows that are packet-eligible today. |
| Latest single-row follow-up | `target/agent-benchmark/segment9-current-ab-swr-generic-final/reanalyzed-summary.json` and `.md` | TypeScript/SWR single-row follow-up: CodeStory quality `1/1` vs baseline `0/1`, with lower tokens and commands. | Row-level regression/debug evidence only. |

All rows above are one-repeat local artifacts. They are useful for branch
review, not public savings claims.

## Packet Runtime Baseline

The latest 18-language packet runtime artifact passes manifest quality for every
row, but most rows are still not self-contained enough to call first-class
packet experiences.

Packet-sufficient rows:

- `javascript-express-routing-flow`
- `c-redis-command-loop`
- `go-gin-route-dispatch`
- `bash-nvm-install-dispatch`
- `html-mdn-form-validation`
- `sql-chinook-schema-relations`

Packet-partial rows:

- `python-requests-session-flow`
- `java-commons-lang-string-utils`
- `rust-ripgrep-search-pipeline`
- `typescript-swr-hook-flow`
- `cpp-fmt-formatting-flow`
- `ruby-jekyll-site-build`
- `php-monolog-record-flow`
- `csharp-automapper-map-flow`
- `kotlin-okio-buffer-flow`
- `swift-alamofire-request-flow`
- `dart-http-client-flow`
- `css-animate-base-and-keyframes`

Latency misses:

- `java-commons-lang-string-utils`: `32,279 ms` packet retrieval.
- `c-redis-command-loop`: `25,215 ms` packet retrieval.

The sufficient set is not the same as the packet-eligible A/B set. The A/B slice
was selected because those rows were useful to compare after packet and manifest
work; it is not the full supported-language surface.

## Steering Boundary

`CODESTORY_EVAL_PROBES` remains test-only in non-test builds, and eval rows are
diagnostics rather than promotion evidence. That is good, but it is not the end
of the steering audit.

Framework and domain semantics are product semantics. React, Next, Remix, LINQ,
ASP.NET, Rails, Django, Gin, Payload CMS, and similar framework-aware routing or
concept extraction should not be removed merely because it is language- or
framework-specific. First-class support requires that kind of domain knowledge.

The audit boundary is whether production crates contain benchmark-specific
knowledge: task ids, known benchmark repo names, `target/agent-benchmark` repo
paths, fixture anchors, expected-answer shapes, or one-off route names that only
exist to satisfy the current holdout. Those belong in benchmark manifests,
scorer inputs, explicit request probes, or `eval_probes.rs` behind test-only
gates.

The current branch largely respects that boundary. The framework route
collectors in `crates/codestory-indexer/src/lib.rs` are legitimate product
semantics and should stay. The request/session/adapter and search-worker/
haystack packet expansions in `crates/codestory-runtime/src/agent/orchestrator.rs`
are broad flow heuristics, so they are **keep or move/rename** candidates, not
delete candidates. If they continue to grow, move them into named domain or
framework profiles instead of hiding them in generic packet planning.

The target boundary is:

- Benchmark-specific probes live in manifests, scorer inputs, request-scoped
  `--extra-probe`/packet inputs, or
  `eval_probes.rs` behind test-only gates.
- Production packet planning can keep product-level framework/domain semantics,
  but it should not name benchmark tasks, repos, fixture paths, or expected
  answer forms.
- Reports say exactly which boundary a run used.

## What This Proves

- The benchmark harness can compare strict no-CodeStory and CodeStory-first
  arms with wall time, token usage, command counts, direct source reads, web
  leakage, packet quality, and post-packet behavior.
- CodeStory is clearly useful on the current 9-row packet-eligible slice.
- Packet runtime can now retrieve and cite expected source evidence across all
  18 supported-language tasks in one-repeat local evidence.
- The remaining problem is no longer just parser coverage; it is packet
  sufficiency, latency, production steering boundaries, and freshness/indexable
  file parity.

## What This Does Not Prove

- It does not prove a broad 18-language A/B win.
- It does not prove every runtime-supported language has equal semantic
  resolution, graph depth, or packet sufficiency.
- It does not prove production packet planning has a clean long-term profile
  architecture for every framework/domain semantic it already knows.
- It does not prove structural/template language freshness parity. That is a
  separate runtime/indexer contract risk to verify with focused tests.
- It does not justify public savings claims or default promotion language.

## Durable Surfaces

Keep these maintained as durable evidence surfaces:

- `scripts/codestory-agent-ab-benchmark.mjs`
- `scripts/codestory-agent-ab-score.mjs`
- `scripts/codestory-language-holdout-integrity.mjs`
- `scripts/tests/codestory-agent-ab-analyzer.test.mjs`
- `benchmarks/tasks/language-expansion-holdout/language-support-ab.task.json`
- `docs/testing/oss-language-corpus.md`

Raw artifacts should stay under `target/agent-benchmark/`. This report should
name the specific raw directories it summarizes, not paste local run catalogs.

## Reproduction

Validate the holdout manifest and corpus shape:

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

Run a packet-gated A/B selection:

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

Run eval-only exact benchmark diagnostics when debugging a row-specific probe:

```powershell
# Only Rust tests and explicit benchmark/eval harnesses can enable this switch;
# release CLI/runtime builds ignore it.
$env:CODESTORY_EVAL_PROBES = "1"
cargo test -p codestory-runtime --test retrieval_generalization_guard -- --nocapture
Remove-Item Env:CODESTORY_EVAL_PROBES
```

Do not use eval-only rows as promotion evidence.

## Promotion Blockers

- Quarantine any task-id, repo-name, fixture-path, expected-answer, or one-off
  benchmark route knowledge found in production crates. Keep real
  framework/domain semantics, and move hidden legitimate semantics into named
  profiles when the generic packet planner becomes too crowded.
- Align runtime freshness, sidecar strictness, and indexer indexability for
  parser-backed, structural, template, text-only, and OpenAPI files.
- Raise packet sufficiency beyond the current `6/18` while keeping manifest
  quality at `18/18`.
- Fix packet retrieval latency misses for Java and Redis.
- Keep no-CodeStory baselines strict: they must inspect the local repository,
  avoid CodeStory tools, avoid web/search leakage, and match the current task
  manifest snapshot.
- Run a fresh full 18-language paired A/B suite only after packet sufficiency and
  steering boundaries improve, then repeat at least three times before claiming
  promotion.
