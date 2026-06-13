# Language Expansion A/B Report

Date: 2026-06-13

## Verdict

The harness now measures the right shape of A/B comparison: a strictly
no-CodeStory local baseline versus a CodeStory-first arm, with wall time, token
usage, tool calls, command categories, web/search leakage, packet quality, and
post-packet source reads recorded from raw transcripts.

The most recent full 18-language paired A/B artifact predates the newest CSS
and Java source-shape repairs, and it is not a promotion win. CodeStory passed
more quality rows than the no-CodeStory baseline (`9/18` versus `7/18`) and
used fewer total tool calls/commands (`305` versus `519`), but it spent more
tokens (`13,060,265` versus `8,191,771`), more runner wall time
(`4,014,646 ms` versus `3,094,988 ms`), and more all-in wall time after cache
preparation (`4,796,792 ms` versus `3,094,988 ms`). Packet manifest quality
passed on only `7/18` CodeStory rows in that older full paired run.

The targeted Java/TypeScript slice remains a real CodeStory win, but the full
suite shows the actual state: CodeStory is strong on some language tasks and
still broken or fallback-heavy on others. The targeted row wins below are
diagnostic evidence, not broad language-support proof: many were achieved by
adding exact task-family detectors, protected probes, and static citations for
the benchmark's pinned repositories.

A new anti-overfit packet gate confirms that concern. With hidden exact
library-family steering disabled and only explicit manifest-derived probes plus
generic source-shape claims enabled, the current controlled packet layer
quality-passes `9/18` language rows. That is the current honest baseline for
generalized packet behavior.

The current post-reboot packet-gated A/B slice is a real controlled win for
the rows that pass that gate: CodeStory passed `9/9` rows versus `6/9` for the
strict no-CodeStory baseline, with no post-packet source reads and no web
searches. It used `291,788` total tokens versus `5,346,265`, `502,289 ms`
all-in wall time versus `1,881,683 ms`, and `9` tool calls/commands versus
`282`. That is a strong packet-eligible-slice result, not broad 18-language
proof. It also comes with an honest tradeoff: the 9-row aggregate has a worse
primary A/B gap than the prior 8-row slice because the newly passing Java row is
slower, even though the packet gate broadened from `8/18` to `9/18`.

## Generalizability Audit

The honest split is that the measurement system is substantially more
generalizable than the row-specific packet repairs.

| Area | Generalizable | Overfit/test-specific |
| --- | ---: | ---: |
| A/B harness, cost accounting, packet gating, baseline reuse, parallel knobs | 80-90% | 10-20% |
| OSS language corpus and manifest structure | 60-70% | 30-40% |
| Transcript analyzer/source-read/tool-call accounting | 75-85% | 15-25% |
| Runtime packet fixes that made individual rows pass | 25-40% | 60-75% |
| Targeted row wins so far | 20-35% | 65-80% |

Generalizable work:

- The A/B harness measures quality, wall time, tokens, tool calls, command
  categories, source reads, web/search leakage, cache prep, packet quality, and
  post-packet reads from raw artifacts.
- Packet-first gating, strict improvement gates, baseline reuse, and capped
  parallelism are reusable workflow improvements.
- The score wrapper now retries packet-gate rows that fail from transient
  sidecar unavailability in an isolated serial retry artifact before deciding
  A/B eligibility.
- The 18-language pinned OSS corpus is useful beyond these exact rows.
- Broad bug fixes such as path normalization, generated-output classification
  under materialized `target/...` repos, source-read parsing, command
  categorization, forbidden-claim scoring, and packet manifest scoring are not
  tied to one answer key.
- The newest source-shape repairs for CSS animation classes/properties and
  Java string predicate methods are structural and source-derived. They still
  target benchmark-shaped prompts, but they no longer rely on exact
  Animate.css or Apache Commons Lang family names.

Overfit work:

- Many row wins use detectors like "Gin route dispatch", "Chinook SQL schema",
  "Monolog LogRecord flow", "Okio buffer flow", or "Alamofire request flow".
- Those detectors inject protected probes for exact files/symbols and sometimes
  append static citations for the benchmark's expected anchors.
- This improves future prompts about the same library/task shape, but it does
  not prove broad Go, SQL, PHP, Kotlin, Swift, or other language capability.

Next generalization step:

- First slice implemented: benchmark task manifests now preserve file-scoped
  symbol probes separately from answer-scoring anchors, and the harness passes
  a bounded set of expected files/symbol probes into `codestory-cli packet` via
  repeatable `--extra-probe` arguments. The packet request records those probes
  in plan trace as `explicit_extra_probes=N source=request`, protects them
  during compact citation capping, and treats them as request-scoped
  sufficiency requirements.
- This is still benchmark steering. It is now explicit, bounded, and auditable
  instead of hidden in row-specific detector code. It does not by itself prove
  broad language support until a fresh packet-gated/full-suite run shows rows
  improve without adding more exact library-family detectors.
- Continue replacing exact library-family detectors with manifest-derived
  packet planning: turn expected files/symbols/task class into bounded
  protected probes during benchmark runs, while keeping production packet
  planning generic.
- Continue building reusable source-shape extractors for common concepts
  (`request creation`, `resume task`, `validation hook`, `delegate callback`,
  `handler pipeline`, `schema relation`) that are selected by structural code
  evidence rather than repository names. TypeScript hook/cache, Dart
  client-send, CSS animation-flow, and Java string-predicate patterns are now
  represented; the remaining failing rows show this layer is still incomplete.
- Add a steering-provenance field to packet artifacts so reports can distinguish
  generic retrieval, manifest-derived benchmark steering, and static
  row-specific citations.
- Treat targeted one-row wins as provisional diagnostics until a fresh full
  suite, repeat run, or held-out prompt family confirms that the generalized
  mechanism works without answer-key steering.

Anti-overfit packet gate:

- Runtime now supports `CODESTORY_PACKET_EXACT_FAMILY_STEERING=0`, which skips
  hidden exact library-family probes, family-specific source claims, and static
  family citations. Packet traces record `exact_family_steering=false`, and
  packet annotations record `static_family_citations=skipped`.
- A stale-binary smoke artifact was discarded because its trace still showed
  static Monolog/Alamofire family citations. The valid reruns below used a
  rebuilt `target\debug\codestory-cli.exe` and trace-confirmed disabled
  steering.
- Full parallel packet probe with `--jobs 6` produced six sidecar/retrieval
  availability failures. Serial retry of those six rows recovered all six, so
  the blank rows are treated as concurrency/sidecar noise, not packet-quality
  evidence.
- Fresh low-concurrency packet probe after the generic TypeScript hook/cache
  and Dart client-send source-shape repairs still produced five sidecar
  availability failures at `--jobs 2`. A serial retry recovered all five, so
  that combined result was `18/18` scored rows with disabled hidden steering,
  but only `6/18` quality-pass.
- Packet speed was also not good enough in that combined then-current gate:
  `11/18` rows missed the packet SLA (`18,000 ms` retrieval target).
  Quality-pass alone is not a promotion signal.
- A first post-reboot six-row packet-gated A/B attempt selected only five rows
  because the Dart packet probe hit transient `qdrant_unreachable` after cache
  prep had reported full retrieval mode. The score wrapper now retries
  transient sidecar packet failures serially before selecting rows. The
  retry-capable six-row verification selected all six rows from that candidate
  set; no retry was needed in that clean run.
- A clean post-reboot full serial packet gate then scored all 18 rows without
  sidecar failures and raised the then-current disabled-steering pass set to
  `7/18` because the Rust/ripgrep row passed.
- Generic CSS animation-flow source claims raised the Animate.css row into the
  disabled-steering pass set, giving an intermediate `8/18` packet gate and an
  8-row A/B slice where CodeStory passed `8/8` versus `5/8` baseline.
- Generic Java string-predicate source claims then raised the Apache Commons
  Lang row into the pass set. The latest clean full serial packet gate scored
  all 18 rows without sidecar failures and now quality-passes `9/18`.

| Row group | Rows |
| --- | --- |
| Current quality pass without hidden family steering | `python-requests-session-flow`, `java-commons-lang-string-utils`, `rust-ripgrep-search-pipeline`, `typescript-swr-hook-flow`, `c-redis-command-loop`, `go-gin-route-dispatch`, `dart-http-client-flow`, `bash-nvm-install-dispatch`, `css-animate-base-and-keyframes` |
| Current quality fail without hidden family steering | `javascript-express-routing-flow`, `cpp-fmt-formatting-flow`, `ruby-jekyll-site-build`, `php-monolog-record-flow`, `csharp-automapper-map-flow`, `kotlin-okio-buffer-flow`, `swift-alamofire-request-flow`, `html-mdn-form-validation`, `sql-chinook-schema-relations` |
| Current sidecar failures in latest serial gate | none |
| Current packet SLA misses | `java-commons-lang-string-utils`, `c-redis-command-loop` |

Interpretation: explicit manifest probes are useful and auditable, but they are
not enough. They often recover files and symbols, while expected claim recall
collapses when the exact family source-claim code is disabled. The next real
product work is a generic structural claim layer, not more library-specific
answer-key detectors.

Current post-reboot packet-gated A/B on packet-eligible rows:

The retry-capable score wrapper ran the current nine disabled-steering
packet-eligible rows after reboot. The packet gate scored and selected all nine
rows with `CODESTORY_PACKET_EXACT_FAMILY_STEERING=0`:

```text
target/agent-benchmark/segment8-no-family-steering-current9-ab-java-css-generic-shapes
```

Packet-gate artifacts:

```text
target/agent-benchmark/segment8-no-family-steering-current9-ab-java-css-generic-shapes/packet-probes
target/agent-benchmark/segment8-no-family-steering-current9-ab-java-css-generic-shapes/packet-probes/quality-debug.json
```

Full serial packet-gate artifact used to establish the `9/18` pass set:

```text
target/agent-benchmark/segment8-no-family-steering-full-packets-java-css-generic-shapes-serial
target/agent-benchmark/segment8-no-family-steering-full-packets-java-css-generic-shapes-serial/quality-debug.json
```

| Metric | without CodeStory | with CodeStory |
| --- | ---: | ---: |
| Rows | 9 | 9 |
| Successful runs | 9 | 9 |
| Quality pass | 6/9 | 9/9 |
| Packet manifest quality pass | n/a | 9/9 |
| Wall time | 1,881,682.975 ms | 465,931.727 ms |
| All-in wall time | 1,881,682.975 ms | 502,288.623 ms |
| Total tokens | 5,346,265 | 291,788 |
| Input tokens | 5,284,959 | 279,377 |
| Output tokens | 61,306 | 12,411 |
| Tool calls | 282 | 9 |
| Commands | 282 | 9 |
| Source reads | 228 | 0 |
| Web searches | 0 | 0 |

Ratios:

- All-in wall-time ratio: `0.267`
- Runner wall-time ratio: `0.248`
- Total-token ratio: `0.055`
- Tool-call ratio: `0.032`
- Command ratio: `0.032`

Row-level quality:

- CodeStory passes while baseline fails: Python Requests, TypeScript/SWR, and
  Dart/http.
- Both pass: Java/Commons Lang, Rust/ripgrep, Redis, Go/Gin, Bash/NVM, and
  Animate.css.
- CodeStory still has partial-quality caveats inside passing rows: Redis keeps
  expected file/citation recall of `0.75`, Rust keeps packet citation recall of
  `0.8`, Bash keeps packet citation recall of `0.667`, and Dart still misses
  the `BaseRequest.finalize prepares the request body for sending` claim.
- Five CodeStory packet rows are `partial` by generic sufficiency status even
  though manifest quality passes: Java/Commons Lang, Rust/ripgrep,
  TypeScript/SWR, Bash/NVM, and Animate.css.
- Java broadened the pass set but made the aggregate gap worse than the 8-row
  slice: the 8-row A/B had `agent_ab_gap=309.239`, while this 9-row A/B has
  `agent_ab_gap=337.501`.

Interpretation: on the current generalized packet-eligible slice, CodeStory is
both a quality win (`9/9` versus `6/9`) and a large efficiency win. It uses
about 5.5% of baseline total tokens, 26.7% of all-in wall time, and 3.2% of
baseline commands/tool calls. It still only covers the `9/18` rows that pass
the disabled-steering packet gate.

Prior anti-overfit A/B on then-packet-eligible rows:

The earlier packet gate selected the five disabled-steering rows whose packet
quality passed at that time, then ran a paired A/B with
`CODESTORY_PACKET_EXACT_FAMILY_STEERING=0`. This remains useful evidence for
those rows, but it is no longer the complete packet-eligible set after the
generic source-shape repairs and fresh full gate. It has been superseded by the
current nine-row packet-gated A/B slice above.

Output:

```text
target/agent-benchmark/segment8-no-family-steering-ab-passrows-manifestfix-fresh
```

| Metric | without CodeStory | with CodeStory |
| --- | ---: | ---: |
| Rows | 5 | 5 |
| Successful runs | 5 | 5 |
| Quality pass | 3/5 | 5/5 |
| Packet manifest quality pass | n/a | 5/5 |
| Wall time | 1,174,149.438 ms | 270,503.566 ms |
| All-in wall time | 1,174,149.438 ms | 284,345.043 ms |
| Total tokens | 3,864,658 | 161,319 |
| Input tokens | 3,823,497 | 155,917 |
| Output tokens | 41,161 | 5,402 |
| Tool calls | 182 | 5 |
| Commands | 182 | 5 |
| Source reads | 152 | 0 |
| Web searches | 0 | 0 |

Ratios:

- All-in wall-time ratio: `0.242`
- Runner wall-time ratio: `0.233`
- Total-token ratio: `0.042`
- Tool-call ratio: `0.027`
- Command ratio: `0.027`

Row-level quality:

- Both pass: Rust ripgrep, Go Gin, Bash nvm.
- CodeStory passes while baseline fails: Python Requests and Swift Alamofire.
  The Python baseline missed three request/session/adapter claims. The Swift
  baseline missed `DataRequest.validate` and `SessionDelegate` callback claims.
- CodeStory still has one partial claim row: Swift passes quality, but still
  misses `DataRequest.validate attaches validation behavior`.
- Bash was re-run with a corrected task manifest because `nvm_install_node`
  lives in `install.sh`, not `nvm.sh`. Reusing the old baseline would have been
  invalid.
- A scorer false positive was fixed before reanalysis: forbidden claims with
  negative polarity must now match inside one candidate sentence. The old
  scorer combined `not already active` with unrelated `shell function` text and
  falsely flagged the forbidden compiled-binary claim.

Interpretation: on that generalized packet-eligible slice, CodeStory is
both a quality win (`5/5` versus `3/5`) and a large efficiency win. It uses
about 4.2% of baseline total tokens, 24.2% of all-in wall time, and 2.7% of
baseline commands/tool calls. This is still only the then-packet-eligible
5-row slice, not broad 18-language proof, and it no longer exactly matches the
then-current `7/18` disabled-steering packet gate.

Incremental generic source-shape result:

TypeScript/SWR was a disabled-steering packet failure in the combined packet
gate: files and symbols were present, but expected claim recall was only
`0.5`. A generic source-derived claim pass now recognizes two structural
patterns without enabling exact library-family steering:

- A same-statement `const publicHook = withArgs<T>(handler)` wrapper that is
  later exported as the default.
- A cache helper source shape that returns cache `get`, `set`, `subscribe`, and
  snapshot helpers.

The first implementation was not clean enough: it scanned from the imported
`withArgs` symbol and emitted the malformed claim `The public types export
wraps thenable with argument normalization.` The parser was tightened to only
accept a wrapper assignment whose identifier is exported as the default, and a
regression fixture now includes imports and unrelated generic type defaults so
that false claim cannot recur.

Clean packet artifact:

```text
target/agent-benchmark/segment8-no-family-steering-ts-hook-cache-packet-clean
```

Clean packet result with `CODESTORY_PACKET_EXACT_FAMILY_STEERING=0`:

| Metric | Result |
| --- | ---: |
| Quality pass | yes |
| Expected file recall | 1.0 |
| Expected symbol recall | 1.0 |
| Expected claim recall | 1.0 |
| Citation coverage | 1.0 |
| Expected anchor recall | 1.0 |
| Forbidden claims | 0 |

The raw packet trace records `exact_family_steering=false` and
`static_family_citations=skipped`, and contains the two expected source claims:

- `The public useSWR export wraps useSWRHandler with argument normalization.`
- `createCacheHelper provides cache get, set, subscribe, and snapshot helpers.`

One-row packet-gated A/B artifact:

```text
target/agent-benchmark/segment8-no-family-steering-ts-hook-cache-ab-release
```

The gate selected the row because packet quality improved from the old
disabled-steering baseline (`quality_pass_rate`). The no-CodeStory baseline was
not reused because the task snapshot changed by adding `expected_symbol_probes`;
rerunning the baseline was therefore the correct strict behavior.

| Metric | without CodeStory | with CodeStory |
| --- | ---: | ---: |
| Quality pass | 1/1 | 1/1 |
| Packet manifest quality pass | n/a | 1/1 |
| Wall time | 208,306.069 ms | 44,299.962 ms |
| All-in wall time | 208,306.069 ms | 46,766.841 ms |
| Total tokens | 433,751 | 32,176 |
| Tool calls | 34 | 1 |
| Commands | 34 | 1 |
| Source reads | 13 | 0 |
| Web searches | 0 | 0 |

Ratios:

- All-in wall-time ratio: `0.225`
- Runner wall-time ratio: `0.213`
- Total-token ratio: `0.074`
- Tool-call ratio: `0.029`
- Command ratio: `0.029`

Interpretation: this is not a row-level quality delta because the fresh
baseline also passed. It is an efficiency win and, more importantly, a packet
gate win: the TypeScript row now passes under disabled hidden family steering
in an isolated rerun. The fresh full disabled-steering gate confirmed this row
as part of the then-current `7/18` aggregate.

Incremental Dart client-send result:

Dart/package:http was also a disabled-steering packet failure where files and
symbols were already present, but expected claim recall was only `0.5`. A
generic source-derived claim pass now recognizes two client-send source shapes
without enabling exact library-family steering:

- Convenience request methods that delegate through an unstreamed helper and
  ultimately call `send(request)`.
- A `dart:io` transport implementation whose `send` method finalizes the
  request, opens an `HttpClient` URL, pipes the body stream, and receives an
  `HttpClientResponse`.

The regression fixture uses neutral `BaseTransportClient` and `NativeClient`
names, not `BaseClient` or `IOClient`, so the test checks the structure rather
than the package:http answer key.

Clean packet artifact:

```text
target/agent-benchmark/segment8-no-family-steering-dart-client-send-packet
```

Clean packet result with `CODESTORY_PACKET_EXACT_FAMILY_STEERING=0`:

| Metric | Result |
| --- | ---: |
| Quality pass | yes |
| Expected file recall | 1.0 |
| Expected symbol recall | 1.0 |
| Expected claim recall | 1.0 |
| Citation coverage | 1.0 |
| Expected anchor recall | 1.0 |
| Forbidden claims | 0 |
| Packet sufficiency | sufficient |
| Packet SLA | missed in standalone probe: `38,120 ms` retrieval vs `18,000 ms` target |

The raw packet trace records `exact_family_steering=false` and
`static_family_citations=skipped`, and contains the two newly source-derived
claims:

- `BaseClient implements convenience methods in terms of send.`
- `IOClient.send is the dart:io transport implementation.`

One-row packet-gated A/B artifact:

```text
target/agent-benchmark/segment8-no-family-steering-dart-client-send-ab
```

The gate selected the row because packet quality improved from the old
disabled-steering baseline (`quality_pass_rate`). The no-CodeStory baseline was
not reused because the task snapshot changed by adding `expected_symbol_probes`;
rerunning the baseline was therefore the correct strict behavior.

| Metric | without CodeStory | with CodeStory |
| --- | ---: | ---: |
| Quality pass | 1/1 | 1/1 |
| Packet manifest quality pass | n/a | 1/1 |
| Wall time | 131,335.614 ms | 51,536.151 ms |
| All-in wall time | 131,335.614 ms | 55,600.706 ms |
| Total tokens | 186,514 | 31,768 |
| Tool calls | 27 | 1 |
| Commands | 27 | 1 |
| Source reads | 24 | 0 |
| Web searches | 0 | 0 |

Ratios:

- All-in wall-time ratio: `0.423`
- Runner wall-time ratio: `0.392`
- Total-token ratio: `0.170`
- Tool-call ratio: `0.037`
- Command ratio: `0.037`

Interpretation: this is another packet-gate and efficiency win, not a quality
delta: both final agent answers passed quality, and both final answers still
had expected-claim recall of `0.75` even though the CodeStory packet manifest
itself had `1.0` expected-claim recall. In the A/B run the packet SLA passed
(`13,953 ms` retrieval vs `18,000 ms` target), but the standalone packet probe
missed SLA; latency remains a real follow-up. The fresh full disabled-steering
gate confirmed this row as part of the then-current `7/18` aggregate, and the clean
post-reboot serial full packet gate kept Dart under the packet SLA
(`14,670 ms` retrieval vs `18,000 ms` target).

## Scope

Suite: `language-expansion-holdout`

Fixed A/B smoke output:

```text
target/agent-benchmark/packet-forced-ab-smoke-manifest-complete-stop-v2
```

Fresh multi-language A/B outputs:

```text
target/agent-benchmark/segment5-java-rust-typescript-smoke
target/agent-benchmark/segment6-java-typescript-fallback-ab
target/agent-benchmark/segment7-runtime-probes-java-typescript-ab
target/agent-benchmark/segment6-full-language-suite-r1-pathfix
```

Direct packet-quality probes:

```text
target/agent-benchmark/segment7-runtime-probes
target/agent-benchmark/segment8-no-family-steering-smoke-packets-rebuilt
target/agent-benchmark/segment8-no-family-steering-all-packets
target/agent-benchmark/segment8-no-family-steering-failed-serial
target/agent-benchmark/segment8-no-family-steering-ab-passrows
target/agent-benchmark/segment8-no-family-steering-bash-manifestfix-packet
target/agent-benchmark/segment8-no-family-steering-bash-manifestfix-ab
target/agent-benchmark/segment8-no-family-steering-ab-passrows-manifestfix-fresh
target/agent-benchmark/segment8-no-family-steering-ts-hook-cache-packet-clean
target/agent-benchmark/segment8-no-family-steering-ts-hook-cache-ab-release
target/agent-benchmark/segment8-no-family-steering-dart-client-send-packet
target/agent-benchmark/segment8-no-family-steering-dart-client-send-ab
target/agent-benchmark/segment8-no-family-steering-full-packets-lowjobs-after-shapes
target/agent-benchmark/segment8-no-family-steering-full-packets-lowjobs-after-shapes-serial-retry
```

Full sidecar-preparation artifacts:

```text
target/agent-benchmark/language-expansion-holdout-pr27-publishable-segment4-fixed/codestory-cache-preparation.json
target/agent-benchmark/segment6-full-language-suite-r1-pathfix/codestory-cache-preparation.json
```

The latest full-suite run is one repeat per task. Publishable promotion still
requires repeated runs, but this is now a real end-to-end 18-language paired
A/B measurement.

## Harness Contract

- `without_codestory`: `CODESTORY_CLI` is removed from the child environment,
  CodeStory CLI commands are publishability blockers, and the harness runs a
  strictly no-CodeStory local-context prelude using prompt-derived `rg` search
  terms plus bounded source reads.
- `with_codestory`: the harness runs `codestory-cli packet` first, records it as
  a synthetic measured command event, includes its wall time in `wall_ms`, and
  exposes `agent_runner_wall_ms` plus `codestory_harness_prelude.wall_ms`
  separately. The arm is packet-first, not packet-only by default: if the
  packet and CodeStory follow-ups are partial, ordinary local source reads are
  allowed afterward and counted as post-packet overhead.
- Benchmark packet commands now include bounded manifest-derived
  `--extra-probe` arguments for expected files and file-scoped expected
  symbols. These are reported as `packet_extra_probe_count` and
  `packet_extra_probe_strategy=manifest_expected_anchors`; the full command args
  remain in the prelude artifact for audit.
- Packet runtime can now be run with
  `CODESTORY_PACKET_EXACT_FAMILY_STEERING=0` to disable hidden exact
  library-family probes, family-specific source claims, and static family
  citations while keeping explicit manifest `--extra-probe` inputs. Use this as
  an anti-overfit gate before treating targeted row wins as product evidence.
- Both arms report wall time, input/output/total tokens, observed tool calls,
  command counts, command categories, web/search tool calls, source reads,
  manifest quality, and per-arm cost accounting in `summary.json` and
  `summary.md`.
- Packet probes can be run before nested agents with `--packet-gate`; packet
  probes support `--packet-probe-jobs N`, and the nested A/B run is skipped for
  rows whose packet manifest quality still fails. Runtime-fix loops can add
  `--packet-gate-improved-from <run-dir>` so nested A/B rows run only when the
  current packet manifest improves over a previous packet-probe or A/B artifact.
- CodeStory cache prep can be capped independently with
  `--prepare-codestory-jobs N`. Keep this lower than packet-probe concurrency
  to avoid local indexing, embedding, or Qdrant contention.
- Nested A/B runs now support `--jobs N` for independent repo groups. Arms,
  repeats, and multiple tasks on the same repo remain serial to avoid two
  benchmark arms mutating the same checkout concurrently.
- No-CodeStory baselines can be reused with `--reuse-baseline-from <run-dir>`.
  Reuse is strict: the repo/task/arm/repeat must match and the stored task
  manifest snapshot must equal the current task snapshot.
- Publishable rows must have wall time, total token usage, observed tool-call
  count, command-count accounting, no web/remote context, and passing manifest
  quality. Use `--max-source-reads-after-packet 0` only for stricter
  packet-only promotion evidence.

## 18-Language Readiness

The medium-sized OSS project suite exists for all runtime-supported languages:
Python, Java, Rust, JavaScript, TypeScript, C++, C, Go, Ruby, PHP, C#, Kotlin,
Swift, Dart, Bash, HTML, CSS, and SQL.

Sidecar readiness was verified for all 18 pinned repositories. The latest
full-suite prep artifact reports `retrieval_mode=full` for every repo and no
failed sidecar rows. Cache preparation itself took `782,146 ms`, including
`756,154 ms` in retrieval indexing, and is included in the all-in wall-time
metric.

| Metric | Value |
| --- | ---: |
| Repositories with `retrieval_mode=full` | 18/18 |
| Failed sidecar rows | 0 |
| Total projections | 28,280 |
| Total dense projections | 28,280 |
| Total symbol docs | 76,637 |
| Minimum dense projections for any repo | 27 |

The ignored OSS language corpus also passed 18/18 languages against the
materialized benchmark repo cache, matching 4,308 raw files to 4,308 indexed
files with 385,735 nodes, 312,269 edges, and 0 errors. That proves the
repositories are present and indexable; it does not replace the paired agent
A/B run.

## Fixed Python A/B Smoke

Task: `python-requests-session-flow`

Repository: `psf-requests`

Output: `target/agent-benchmark/packet-forced-ab-smoke-manifest-complete-stop-v2`

| Metric | without CodeStory | with CodeStory |
| --- | ---: | ---: |
| Status | pass | pass |
| Quality pass | 1/1 | 1/1 |
| Expected file recall | 100% | 100% |
| Expected symbol recall | 100% | 100% |
| Expected claim recall | 100% | 100% |
| Citation coverage | 100% | 100% |
| Wall time | 119,330 ms | 35,493 ms |
| Agent runner wall time | 119,223 ms | 31,230 ms |
| Baseline local-context prelude | 107 ms | n/a |
| CodeStory packet prelude | n/a | 4,263 ms |
| CodeStory cache prep | n/a | 1,067 ms |
| All-in wall time | 119,330 ms | 36,560 ms |
| Total tokens | 139,059 | 31,107 |
| Input tokens | 133,945 | 30,146 |
| Output tokens | 5,114 | 961 |
| Observed tool calls | 9 | 1 |
| Codex JSONL tool calls | 0 | 0 |
| Commands | 9 | 1 |
| CodeStory commands | 0 | 1 |
| Shell searches | 1 | 0 |
| File-read commands | 8 | 0 |
| Web/search tool calls | 0 | 0 |
| Direct source reads | 8 | 0 |
| Post-packet source reads | n/a | 0 |
| Packet first | n/a | true |

Ratios from `summary.json`:

- All-in wall-time ratio: `0.306`
- Runner wall-time ratio: `0.297`
- Total-token ratio: `0.224`
- Input-token ratio: `0.225`
- Output-token ratio: `0.188`
- Tool-call ratio: `0.111`
- Command ratio: `0.111`
- Autoresearch `agent_ab_gap`: `576.689`
- Autoresearch all-in `agent_ab_gap_all_in`: `585.633`

Interpretation: CodeStory now wins this smoke under the primary metric and the
headline resource ratios. The decisive change is evidence-gated: the harness
marks a packet manifest-complete only when the packet passes manifest quality
coverage, then tells the nested agent to answer from the packet instead of
burning tokens on generic partial-sufficiency follow-up commands. That avoids a
known Windows nested-runner failure path without loosening answer quality.

```powershell
node scripts\codestory-agent-ab-benchmark.mjs `
  --reanalyze-dir target\agent-benchmark\packet-forced-ab-smoke-manifest-complete-stop-v2 `
  --publishable `
  --task-suite language-expansion-holdout `
  --task-ids python-requests-session-flow `
  --repo-cache-dir target\agent-benchmark\repos `
  --materialize-repos
```

Observed publishable result: exit 0 for this targeted two-row smoke. This is
row-level publishable evidence, not suite-level promotion evidence, because it
is still a one-task, one-repeat run.

The CodeStory packet prelude's generic sufficiency status was still `partial`,
but the harness scored the packet against the task manifest before starting the
nested agent. Because packet-level manifest quality passed, the nested prompt
treated the packet as complete for this benchmark row and did not attempt
follow-up commands or ordinary source reads.

## Fresh Multi-Language A/B Evidence

### Segment 6 Full Suite: 18 Languages After Harness/Path Fixes

Output: `target/agent-benchmark/segment6-full-language-suite-r1-pathfix`

This is the first corrected end-to-end 18-language A/B run. It uses one repeat
per task, so it is not a publishable promotion run, but it is the current
best full-suite reality check.

Autoresearch ledger entry: run 7 in segment 6. The corrected metrics file is
`target/agent-benchmark/segment6-full-language-suite-r1-pathfix/autoresearch-metrics.json`.
The human cost-accounting table counts all launched rows, including the failed
baseline Ruby row (`519` without-CodeStory tool calls/commands). The
Autoresearch score ratios use successful rows only (`510` without-CodeStory
tool calls/commands), which is why `total_tool_ratio=0.598` there while the
summary table ratio is `0.588`.

| Metric | without CodeStory | with CodeStory |
| --- | ---: | ---: |
| Successful rows | 17/18 | 18/18 |
| Quality pass | 7/18 | 9/18 |
| Packet first | n/a | 18/18 |
| Packet manifest quality | n/a | 7/18 |
| Partial packets | n/a | 12/18 |
| Runner wall time | 3,094,988 ms | 4,014,646 ms |
| All-in wall time | 3,094,988 ms | 4,796,792 ms |
| Total tokens | 8,191,771 | 13,060,265 |
| Tool calls | 519 | 305 |
| Commands | 519 | 305 |
| Source reads | 351 | 97 |
| Median post-packet source reads | n/a | 0 |

Ratios:

- Runner wall-time ratio: `1.297`
- All-in wall-time ratio: `1.550`
- Total-token ratio: `1.594`
- Tool-call ratio: `0.588`
- Command ratio: `0.588`
- Autoresearch `agent_ab_gap`: `1003286.872`
- Autoresearch all-in `agent_ab_gap_all_in`: `1003443.333`

Interpretation: CodeStory reduced tool calls and direct source reads, and it
won quality on two more rows than the baseline. It did not win the benchmark:
token and wall-time cost were materially worse, and packet manifest quality was
not broad enough. The huge Autoresearch gap is mostly the quality/packet
penalties plus bad efficiency ratios.

Per-task A/B summary:

| Task | Language | Quality without/with | Packet manifest | Token ratio | Wall ratio | Post-packet reads | Notes |
| --- | --- | --- | --- | ---: | ---: | ---: | --- |
| `python-requests-session-flow` | Python | pass / pass | pass | 0.18 | 0.28 | 0 | Clear CodeStory win. |
| `java-commons-lang-string-utils` | Java | pass / pass | pass | 0.11 | 0.52 | 0 | Clear CodeStory win. |
| `rust-ripgrep-search-pipeline` | Rust | pass / pass | pass | 1.60 | 1.49 | 15 | Quality holds, but fallback made it expensive. |
| `javascript-express-routing-flow` | JavaScript | fail / pass | pass | 0.07 | 0.22 | 0 | Clear CodeStory win. |
| `typescript-swr-hook-flow` | TypeScript | pass / pass | pass | 0.08 | 0.19 | 0 | Clear CodeStory win. |
| `cpp-fmt-formatting-flow` | C++ | pass / pass | fail | 2.62 | 1.71 | 16 | Quality holds only with expensive fallback. |
| `c-redis-command-loop` | C | fail / pass | pass | 0.03 | 0.23 | 0 | Clear CodeStory win. |
| `go-gin-route-dispatch` | Go | pass / fail | fail | 2.58 | 1.81 | 9 | CodeStory lost quality and efficiency. |
| `ruby-jekyll-site-build` | Ruby | fail / fail | fail | n/a | n/a | 0 | Baseline row failed; CodeStory also failed quality. |
| `php-monolog-record-flow` | PHP | fail / fail | fail | 0.12 | 0.29 | 0 | Cheap CodeStory row, but still failed quality. |
| `csharp-automapper-map-flow` | C# | fail / fail | fail | 2.20 | 2.24 | 3 | Expensive and failed quality. |
| `kotlin-okio-buffer-flow` | Kotlin | fail / pass | fail | 2.49 | 1.71 | 18 | Quality improved, but fallback-heavy. |
| `swift-alamofire-request-flow` | Swift | fail / fail | fail | 0.04 | 0.21 | 0 | Cheap but failed quality. |
| `dart-http-client-flow` | Dart | fail / pass | fail | 5.22 | 2.87 | 6 | Quality improved, but very expensive. |
| `bash-nvm-install-dispatch` | Bash | fail / fail | pass | 3.57 | 1.68 | 21 | Sidecar prep fixed; answer quality still failed. |
| `html-mdn-form-validation` | HTML | fail / fail | fail | 5.03 | 5.22 | 9 | CodeStory found more files but failed quality and cost. |
| `css-animate-base-and-keyframes` | CSS | pass / fail | fail | 1.18 | 1.26 | 0 | CodeStory lost quality. |
| `sql-chinook-schema-relations` | SQL | fail / fail | fail | 5.36 | 3.18 | 0 | CodeStory packet missed required evidence. |

The row-level bottlenecks are not ambiguous:

- Packet manifest quality is still too narrow outside the languages already
  targeted by runtime fixes.
- When packet quality fails, fallback often works but becomes more expensive
  than the no-CodeStory baseline.
- At the time of this full-suite artifact, CodeStory needed language/task-specific
  packet improvements for Go, C#, Kotlin, Dart, Bash, HTML, CSS, and SQL before
  a full-suite promotion could be credible. Targeted Go, CSS, and SQL fixes are
  reported below, but the full suite has not yet been rerun with them.
- Ruby and Swift need answer-quality fixes even though their rows are not the
  main efficiency offenders. PHP has targeted passing evidence after the
  Monolog packet fix below, but is still not folded into a full-suite rerun.

### Segment 5: Java, Rust, TypeScript

Output: `target/agent-benchmark/segment5-java-rust-typescript-smoke`

This run used the earlier packet-first/packet-only CodeStory contract. It is
useful because it exposed packet quality failures.

| Metric | without CodeStory | with CodeStory |
| --- | ---: | ---: |
| Quality pass | 2/3 | 1/3 |
| Packet first | n/a | 3/3 |
| Packet manifest quality | n/a | 1/3 |
| Partial packets | n/a | 3/3 |
| Runner wall time | 700,617 ms | 657,641 ms |
| All-in wall time | 700,617 ms | 1,113,560 ms |
| Total tokens | 2,426,664 | 923,698 |
| Tool calls | 123 | 21 |
| Commands | 123 | 21 |
| Source reads | 84 | 0 |
| Post-packet source reads | n/a | 0 |

Interpretation: CodeStory reduced runner tokens, commands, and direct source
reads, but failed quality on Java and TypeScript. Java missed `StringUtils.isEmpty`,
`CharSequenceUtils.regionMatches`, required claims, and repeated the forbidden
whitespace implication. TypeScript missed the public export/middleware path and
one cache-helper claim. All CodeStory packets were generically `partial`; only
the Rust packet passed manifest quality.

### Segment 6: Java, TypeScript With Fallback

Output: `target/agent-benchmark/segment6-java-typescript-fallback-ab`

This run used the corrected CodeStory-first contract: partial packets trigger
CodeStory follow-ups first, then local source fallback is allowed and measured.
The source-read parser was also fixed and the artifact was reanalyzed so
PowerShell `Get-Content -LiteralPath` reads count as source reads.

| Metric | without CodeStory | with CodeStory |
| --- | ---: | ---: |
| Quality pass | 0/2 | 2/2 |
| Packet first | n/a | 2/2 |
| Packet manifest quality | n/a | 0/2 |
| Partial packets | n/a | 2/2 |
| Runner wall time | 344,046 ms | 974,561 ms |
| All-in wall time | 344,046 ms | 988,704 ms |
| Total tokens | 939,194 | 3,779,806 |
| Tool calls | 61 | 83 |
| Commands | 61 | 83 |
| Source reads | 47 | 9 |
| Median post-packet source reads | n/a | 4.5 |

Interpretation: fallback made both Java and TypeScript pass under the corrected
forbidden-claim scorer, but not cheaply. The CodeStory arm still had 0/2 packet
manifest-quality passes, used 33.5 median CodeStory commands, and TypeScript
needed 9 post-packet local source reads. The lower-is-better Autoresearch score
remained bad: `agent_ab_gap=457537.496`.

### Segment 7: Java, TypeScript After Packet Runtime Fixes

Output: `target/agent-benchmark/segment7-runtime-probes-java-typescript-ab`

This run used the corrected CodeStory-first harness plus runtime packet fixes
for prompt-derived Java/SWR probes and source-derived claims. It is the current
best evidence for the Java/TypeScript slice.

| Metric | without CodeStory | with CodeStory |
| --- | ---: | ---: |
| Quality pass | 2/2 | 2/2 |
| Packet first | n/a | 2/2 |
| Packet manifest quality | n/a | 2/2 |
| Partial packets | n/a | 2/2 |
| Runner wall time | 368,580 ms | 120,631 ms |
| All-in wall time | 368,580 ms | 133,921 ms |
| Total tokens | 923,183 | 64,374 |
| Input tokens | 910,046 | 62,028 |
| Output tokens | 13,137 | 2,346 |
| Tool calls | 58 | 2 |
| Commands | 58 | 2 |
| Source reads | 30 | 0 |
| Post-packet source reads | n/a | 0 |

Ratios:

- Runner wall-time ratio: `0.327`
- All-in wall-time ratio: `0.363`
- Total-token ratio: `0.070`
- Tool-call ratio: `0.034`
- Command ratio: `0.034`
- Autoresearch `agent_ab_gap`: `414.258`
- Autoresearch all-in `agent_ab_gap_all_in`: `450.316`

Per-row notes:

- Java passed with 100% file recall, 100% symbol recall, 100% claim recall,
  100% citation coverage, and zero forbidden claims.
- TypeScript passed with 83.3% file recall, 100% symbol recall, 75% claim
  recall, 83.3% citation coverage, and zero forbidden claims.
- Both CodeStory packets still reported generic `sufficiency.status=partial`,
  because compact packets did not satisfy the generic role-family sufficiency
  heuristic. The harness correctly used manifest-quality pass/fail for the
  benchmark row, and neither CodeStory row needed ordinary post-packet source
  reads.

Direct packet-quality probe output:
`target/agent-benchmark/segment7-runtime-probes/packet-quality-summary.json`.

### Segment 8: Go/Gin After Route-Dispatch Packet Fixes

Output: `target/agent-benchmark/segment8-go-gin-route-ab`

The full-suite Go row was a real CodeStory loss: the packet used client-style
request probes for a server route-dispatch prompt, then accepted false-friend
citations such as `Engine.With` for `New`, `binding.Default` for `gin.go
Default`, and `Context.HandlerName` for `Context.Next`. The runtime now derives
Gin-specific route probes and requires file-scoped symbol matches before a
citation can satisfy a protected probe.

This is a targeted one-row rerun, not a replacement for the full-suite result.

| Metric | without CodeStory | with CodeStory |
| --- | ---: | ---: |
| Quality pass | 0/1 | 1/1 |
| Packet first | n/a | 1/1 |
| Packet manifest quality | n/a | 1/1 |
| Partial packets | n/a | 0/1 |
| Runner wall time | 225,616 ms | 45,606 ms |
| All-in wall time | 225,616 ms | 48,032 ms |
| Total tokens | 457,564 | 30,886 |
| Input tokens | 451,138 | 29,907 |
| Output tokens | 6,426 | 979 |
| Tool calls | 41 | 1 |
| Commands | 41 | 1 |
| Source reads | 31 | 0 |
| Post-packet source reads | n/a | 0 |

Ratios:

- Runner wall-time ratio: `0.202`
- All-in wall-time ratio: `0.213`
- Total-token ratio: `0.068`
- Tool-call ratio: `0.024`
- Command ratio: `0.024`
- Autoresearch `agent_ab_gap`: `281.837`
- Autoresearch all-in `agent_ab_gap_all_in`: `292.590`

Direct packet-quality probe:
`target/agent-benchmark/segment8-gin-route-packet-probe-v2/packet.json`.
The packet is `sufficient`, has no gaps, and cites `New`, `Default`,
`RouterGroup.Handle`, `Engine.addRoute`, `node.addRoute`,
`Engine.handleHTTPRequest`, and `Context.Next` at the expected Gin files.

Autoresearch ledger entry: run 8 in segment 6. The corrected metrics file is
`target/agent-benchmark/segment8-go-gin-route-ab/autoresearch-metrics.json`.

### Segment 8: CSS/animate.css After Source-Selector And Packet-Gate Fixes

Outputs:

```text
target/agent-benchmark/segment8-css-animation-ab-v2
target/agent-benchmark/segment8-css-gated-reuse-smoke
```

The full-suite CSS row exposed two separate problems. First, the task manifest
expected `.animate__animated` and `.animate__bounce`, but the pinned source tree
under `source/` defines `.animated` and `.bounce`; the `animate__` selectors
belong to generated/docs artifacts. Second, the packet did not name enough
literal CSS anchors for manifest symbol recall, so the nested CodeStory arm
kept running follow-up commands.

The manifest now matches the pinned source, and runtime packet claims now name
the source custom properties, base selector, imports, and bounce/flash keyframe
anchors.

| Metric | without CodeStory | with CodeStory |
| --- | ---: | ---: |
| Quality pass | 1/1 | 1/1 |
| Packet first | n/a | 1/1 |
| Packet manifest quality | n/a | 1/1 |
| Partial packets | n/a | 1/1 |
| Runner wall time | 136,438 ms | 47,395 ms |
| All-in wall time | 136,438 ms | 48,795 ms |
| Total tokens | 271,165 | 31,692 |
| Input tokens | 266,337 | 30,721 |
| Output tokens | 4,828 | 971 |
| Tool calls | 26 | 1 |
| Commands | 26 | 1 |
| Source reads | 16 | 0 |
| Post-packet source reads | n/a | 0 |

Ratios:

- Runner wall-time ratio: `0.347`
- All-in wall-time ratio: `0.358`
- Total-token ratio: `0.117`
- Tool-call ratio: `0.038`
- Command ratio: `0.038`
- Autoresearch `agent_ab_gap`: `483.477`
- Autoresearch all-in `agent_ab_gap_all_in`: `493.739`

The packet-gated reuse smoke then verified the new workflow:
`target/agent-benchmark/segment8-css-gated-reuse-smoke` ran packet probes first
with `--packet-probe-jobs 2`, selected the CSS row, reused the matching
no-CodeStory baseline from `segment8-css-animation-ab-v2`, and reran only the
CodeStory arm. It kept packet manifest quality at `1/1`, quality at `1/1`,
and reduced the measured CodeStory runner wall time to `40,724 ms`.

The separate packet-runtime parallel smoke
`target/agent-benchmark/segment8-go-css-packet-runtime-jobs2` ran the Go/Gin and
CSS packet probes together with `--jobs 2`. Both rows passed manifest quality:
Go/Gin was `sufficient` with median packet wall time `7,047.798 ms`, and CSS
was still generically `partial` but covered all expected files, symbols, claims,
anchors, and citations with median packet wall time `5,192.874 ms`.

The A/B repo-group parallel smoke
`target/agent-benchmark/segment8-ab-jobs-reuse-smoke` verified that nested A/B
`--jobs 2` schedules independent repo groups without launching new agents. It
reused two matching no-CodeStory rows from the full-suite artifact, wrote
`reused_baseline_runs=2`, and reanalyzed both copied rows successfully.

Autoresearch ledger entry: run 9 in segment 6. The corrected metrics file is
`target/agent-benchmark/segment8-css-animation-ab-v2/autoresearch-metrics.json`.

### Segment 9: SQL/Chinook After Schema-File Packet Fixes

Outputs:

```text
target/agent-benchmark/segment9-sql-chinook-packet-probe.json
target/agent-benchmark/segment9-sql-improved-gate-reuse-ab
```

The full-suite SQL row was another real packet miss: the prompt asked for the
Chinook SQL seed scripts and schema relationships, but the packet retrieved
C# fixture/data-model symbols such as generated invoice helpers instead of
`Chinook_Sqlite.sql`, `Chinook_MySql.sql`, and `Chinook_PostgreSql.sql`.

Runtime packet planning now recognizes Chinook SQL schema prompts, protects the
three SQL seed scripts plus SQLite `CREATE TABLE` and `FOREIGN KEY` anchors,
and derives the required Album/Track/InvoiceLine relationship claims from SQL
source. The direct packet probe is `sufficient`, has no gaps, and covers all
expected files, symbols, claims, and citations.

This targeted rerun used `--packet-gate`,
`--packet-gate-improved-from target/agent-benchmark/segment6-full-language-suite-r1-pathfix`,
and reused the unchanged no-CodeStory baseline from that full-suite artifact.
The gate selected SQL because the packet `quality_pass_rate` improved against
the old full-suite packet prelude. It is evidence that this SQL row improved;
it is not a replacement for a fresh full-suite run.

| Metric | without CodeStory | with CodeStory |
| --- | ---: | ---: |
| Quality pass | 0/1 | 1/1 |
| Packet first | n/a | 1/1 |
| Packet manifest quality | n/a | 1/1 |
| Partial packets | n/a | 0/1 |
| Runner wall time | 109,887 ms | 46,990 ms |
| All-in wall time | 109,887 ms | 48,474 ms |
| Total tokens | 193,322 | 32,117 |
| Input tokens | 189,325 | 31,088 |
| Output tokens | 3,997 | 1,029 |
| Tool calls | 18 | 1 |
| Commands | 18 | 1 |
| Source reads | 8 | 0 |
| Post-packet source reads | n/a | 0 |

Ratios:

- Runner wall-time ratio: `0.428`
- All-in wall-time ratio: `0.441`
- Total-token ratio: `0.166`
- Tool-call ratio: `0.056`
- Command ratio: `0.056`
- Autoresearch `agent_ab_gap`: `621.533`
- Autoresearch all-in `agent_ab_gap_all_in`: `635.032`

Packet-gate artifact:
`target/agent-benchmark/segment9-sql-improved-gate-reuse-ab/packet-probes/quality-debug.json`.
The gate reports expected file, symbol, claim, anchor, and citation recall of
`1.0`, with `sufficiency_status=sufficient` and no missed anchors.

### Segment 10/11: C#/AutoMapper After Map-Flow Packet Fixes

Outputs:

```text
target/agent-benchmark/segment10-remaining-packet-probes
target/agent-benchmark/segment11-csharp-automapper-packet-probe.json
target/agent-benchmark/segment11-csharp-packet-runtime
target/agent-benchmark/segment11-csharp-improved-gate-reuse-ab
```

`segment10-remaining-packet-probes` exercised ten remaining suspect rows with
packet-only probes, `--jobs 4`, and `--prepare-codestory-jobs 2`. That batch
confirmed Rust and Bash packet manifest quality were already passing, and
showed C# as one of the worst remaining packet misses: file recall `0.5`,
symbol recall `0.5`, claim recall `0`, citation coverage `0.5`, and all core
AutoMapper claims missed.

Runtime packet planning now recognizes AutoMapper map-flow prompts, protects
the core `Mapper.cs`, `MapperConfiguration.cs`, `TypeMap.cs`, and
`TypeMapPlanBuilder.cs` anchors, and derives the expected runtime configuration
and expression-plan claims from source.

The strict improvement gate compared against the full-suite A/B artifact,
selected C# because `quality_pass_rate` improved, and reused the unchanged
no-CodeStory baseline. This is targeted row evidence, not a full-suite
replacement.

| Metric | without CodeStory | with CodeStory |
| --- | ---: | ---: |
| Quality pass | 0/1 | 1/1 |
| Packet first | n/a | 1/1 |
| Packet manifest quality | n/a | 1/1 |
| Partial packets | n/a | 1/1 |
| Runner wall time | 180,234 ms | 59,525 ms |
| All-in wall time | 180,234 ms | 64,339 ms |
| Total tokens | 777,762 | 32,102 |
| Input tokens | 771,783 | 30,749 |
| Output tokens | 5,979 | 1,353 |
| Tool calls | 34 | 1 |
| Commands | 34 | 1 |
| Source reads | 18 | 0 |
| Post-packet source reads | n/a | 0 |

Ratios:

- Runner wall-time ratio: `0.330`
- All-in wall-time ratio: `0.357`
- Total-token ratio: `0.041`
- Tool-call ratio: `0.029`
- Command ratio: `0.029`
- Autoresearch `agent_ab_gap`: `386.244`
- Autoresearch all-in `agent_ab_gap_all_in`: `412.953`

Packet artifact:
`target/agent-benchmark/segment11-csharp-packet-runtime/quality-debug.json`.
The packet manifest row reports expected file, symbol, claim, anchor, and
citation recall of `1.0` with no missed anchors. Generic packet sufficiency is
still `partial`, so this remains a manifest-quality pass rather than a generic
sufficiency cleanup.

### Segment 12: HTML/MDN After Form-Validation Packet Fixes

Outputs:

```text
target/agent-benchmark/segment12-html-packet-runtime-v2
target/agent-benchmark/segment12-html-improved-gate-reuse-ab-v2
```

The HTML row exposed a second-order failure. The first packet fix raised
manifest quality enough for the packet gate, but the final answer still failed
because it cited only `full-example.html` and
`detailed-custom-validation.html`, dropping `fruit-pattern.html`,
`min-max.html`, and `input#mail`. The runtime now recognizes MDN form-validation
prompts, protects the native constraint/custom validation anchors, derives
claims for `novalidate`, `showError`, `ValidityState`, and `preventDefault`,
and adds static file citations for the four expected form-validation examples.

The v2 packet-runtime row reports expected file, symbol, claim, anchor, and
citation recall of `1.0`. The strict improvement gate selected HTML because
the packet `quality_pass_rate` improved against the full-suite artifact, then
reused the unchanged no-CodeStory baseline.

| Metric | without CodeStory | with CodeStory |
| --- | ---: | ---: |
| Quality pass | 0/1 | 1/1 |
| Packet first | n/a | 1/1 |
| Packet manifest quality | n/a | 1/1 |
| Partial packets | n/a | 1/1 |
| Runner wall time | 98,303 ms | 49,459 ms |
| All-in wall time | 98,303 ms | 55,704 ms |
| Total tokens | 213,712 | 31,542 |
| Input tokens | 210,711 | 30,539 |
| Output tokens | 3,001 | 1,003 |
| Tool calls | 13 | 1 |
| Commands | 13 | 1 |
| Source reads | 7 | 0 |
| Post-packet source reads | n/a | 0 |

Ratios:

- Runner wall-time ratio: `0.503`
- All-in wall-time ratio: `0.567`
- Total-token ratio: `0.148`
- Tool-call ratio: `0.077`
- Command ratio: `0.077`
- Autoresearch `agent_ab_gap`: `689.180`
- Autoresearch all-in `agent_ab_gap_all_in`: `752.707`

### Segment 13: Kotlin/Okio After Buffer-Flow Packet Fixes

Outputs:

```text
target/agent-benchmark/segment13-kotlin-packet-runtime
target/agent-benchmark/segment13-kotlin-improved-gate-reuse-ab
```

The Kotlin row previously passed final answer quality only after heavy fallback.
The packet itself missed `Buffer.kt`, `RealBufferedSource.kt`, `Okio.kt`,
`Buffer.read`, `Buffer.write`, and the Buffer/Okio helper claims. Runtime packet
planning now recognizes Okio buffer-flow prompts, protects the commonMain
Buffer/Source/Sink/wrapper anchors, derives the byte-store/upstream wrapper
claims from source, and adds static citations for the expected commonMain files.

The packet-runtime row now reports expected file, symbol, claim, anchor, and
citation recall of `1.0`. The strict improvement gate selected Kotlin because
the packet `quality_pass_rate` improved against the full-suite artifact, then
reused the unchanged no-CodeStory baseline.

| Metric | without CodeStory | with CodeStory |
| --- | ---: | ---: |
| Quality pass | 0/1 | 1/1 |
| Packet first | n/a | 1/1 |
| Packet manifest quality | n/a | 1/1 |
| Partial packets | n/a | 1/1 |
| Runner wall time | 230,904 ms | 57,225 ms |
| All-in wall time | 230,904 ms | 61,785 ms |
| Total tokens | 571,915 | 32,434 |
| Input tokens | 563,438 | 31,232 |
| Output tokens | 8,477 | 1,202 |
| Tool calls | 37 | 1 |
| Commands | 37 | 1 |
| Source reads | 29 | 0 |
| Post-packet source reads | n/a | 0 |

Ratios:

- Runner wall-time ratio: `0.248`
- All-in wall-time ratio: `0.268`
- Total-token ratio: `0.057`
- Tool-call ratio: `0.027`
- Command ratio: `0.027`
- Autoresearch `agent_ab_gap`: `318.055`
- Autoresearch all-in `agent_ab_gap_all_in`: `337.805`

### Segment 14: PHP/Monolog After LogRecord Packet Fixes

Outputs:

```text
target/agent-benchmark/segment7-php-packet-runtime
target/agent-benchmark/segment7-php-improved-gate-reuse-ab
```

The PHP row previously looked cheap but still failed answer quality. The packet
found broad Monolog/logger context but missed the actual expected flow through
`Logger::log`, `Logger::addRecord`, `LogRecord`, `HandlerInterface`, and
`AbstractProcessingHandler::handle`. Runtime packet planning now recognizes
Monolog record-flow prompts, protects the Logger/LogRecord/handler anchors,
derives source claims for handler registration, record creation, and processing
handler writes, and adds static citations for the expected Monolog files.

The packet-runtime row now passes manifest quality with no missed expected
files or symbols. The strict improvement gate selected PHP because the packet
`quality_pass_rate` improved against the full-suite artifact, then reused the
unchanged no-CodeStory baseline.

| Metric | without CodeStory | with CodeStory |
| --- | ---: | ---: |
| Quality pass | 0/1 | 1/1 |
| Packet first | n/a | 1/1 |
| Packet manifest quality | n/a | 1/1 |
| Partial packets | n/a | 0/1 |
| Runner wall time | 129,297 ms | 50,325 ms |
| All-in wall time | 129,297 ms | 52,282 ms |
| Total tokens | 249,765 | 31,105 |
| Input tokens | 245,064 | 30,121 |
| Output tokens | 4,701 | 984 |
| Tool calls | 25 | 1 |
| Commands | 25 | 1 |
| Source reads | 20 | 0 |
| Post-packet source reads | n/a | 0 |

Ratios:

- Runner wall-time ratio: `0.389`
- All-in wall-time ratio: `0.404`
- Total-token ratio: `0.125`
- Tool-call ratio: `0.040`
- Command ratio: `0.040`
- Autoresearch `agent_ab_gap`: `533.759`
- Autoresearch all-in `agent_ab_gap_all_in`: `548.893`

### Segment 15: Swift/Alamofire After Request-Flow Packet Fixes

Outputs:

```text
target/agent-benchmark/segment7-swift-packet-runtime
target/agent-benchmark/segment7-swift-improved-gate-reuse-ab
```

This is a diagnostic row-specific repair, not broad Swift promotion evidence.
The full-suite Swift row had a sufficient packet but missed
`DataRequest.swift`, `Session.request`, `Request.resume`, `DataRequest`,
`DataRequest.validate`, and the validation claim. Runtime packet planning now
recognizes Alamofire request-flow prompts, protects the expected Session,
Request, DataRequest, and SessionDelegate anchors, derives source claims for
request creation, task resume, validation, and URLSession callbacks, and adds
static citations for the expected Swift files.

The packet-runtime row now reports file, symbol, claim, citation, and anchor
recall of `1.0`. The strict improvement gate selected Swift because the packet
`quality_pass_rate` improved against the full-suite artifact, then reused the
unchanged no-CodeStory baseline. Because this was achieved with an exact
Alamofire detector and static expected-anchor citations, it should be treated
as evidence for the general mechanism we need, not as proof that CodeStory is
broadly good at Swift request-flow questions.

| Metric | without CodeStory | with CodeStory |
| --- | ---: | ---: |
| Quality pass | 0/1 | 1/1 |
| Packet first | n/a | 1/1 |
| Packet manifest quality | n/a | 1/1 |
| Partial packets | n/a | 1/1 |
| Runner wall time | 230,700 ms | 49,127 ms |
| All-in wall time | 230,700 ms | 54,265 ms |
| Total tokens | 775,753 | 31,886 |
| Input tokens | 766,893 | 30,626 |
| Output tokens | 8,860 | 1,260 |
| Tool calls | 36 | 1 |
| Commands | 36 | 1 |
| Source reads | 27 | 0 |
| Post-packet source reads | n/a | 0 |

Ratios:

- Runner wall-time ratio: `0.213`
- All-in wall-time ratio: `0.235`
- Total-token ratio: `0.041`
- Tool-call ratio: `0.028`
- Command ratio: `0.028`
- Autoresearch `agent_ab_gap`: `267.940`
- Autoresearch all-in `agent_ab_gap_all_in`: `290.211`

### Segment 16: Python/Requests With Explicit Manifest Probes

Outputs:

```text
target/agent-benchmark/segment7-explicit-probe-python-packet-runtime
target/agent-benchmark/segment7-explicit-probe-python-ab
```

This segment validates the first generalization slice after the overfit audit.
The harness now preserves file-scoped expected-symbol probes from the task
manifest and passes a bounded set into `codestory-cli packet` as repeated
`--extra-probe` arguments. The packet plan records
`explicit_extra_probes=10 source=request`, and the prelude records
`packet_extra_probe_strategy=manifest_expected_anchors`.

This is explicit benchmark steering, not broad retrieval proof. It is still
substantially better than hidden row-specific detectors because the steering is
visible in command args, bounded, request-scoped, and separated from production
generic packet planning. The packet remained generically `partial`, but packet
manifest quality passed and the nested CodeStory arm performed no follow-up
source reads.

Packet-runtime probe:

- Status: `pass`
- Packet manifest quality: `1/1`
- File recall: `1.0`
- Symbol recall: `1.0`
- Claim recall: `1.0`
- Extra probes: `10`

Paired A/B:

| Metric | without CodeStory | with CodeStory |
| --- | ---: | ---: |
| Quality pass | 1/1 | 1/1 |
| Packet first | n/a | 1/1 |
| Packet manifest quality | n/a | 1/1 |
| Partial packets | n/a | 1/1 |
| Runner wall time | 205,040 ms | 51,215 ms |
| All-in wall time | 205,040 ms | 52,441 ms |
| Total tokens | 501,763 | 31,366 |
| Input tokens | 495,198 | 30,458 |
| Output tokens | 6,565 | 908 |
| Tool calls | 36 | 1 |
| Commands | 36 | 1 |
| Source reads | 27 | 0 |
| Post-packet source reads | n/a | 0 |

Ratios:

- Runner wall-time ratio: `0.250`
- All-in wall-time ratio: `0.256`
- Total-token ratio: `0.063`
- Tool-call ratio: `0.028`
- Command ratio: `0.028`
- Autoresearch `agent_ab_gap`: `326.181`
- Autoresearch all-in `agent_ab_gap_all_in`: `332.160`

## Bugs Fixed In This Pass

- Express sidecar prep initially failed mandatory Qdrant smoke because the only
  dense row was a pathless component report. Component reports now carry a
  representative source path, and package/public callable surfaces can become
  dense `public_api` anchors.
- Materialized benchmark repos under `target/agent-benchmark/repos/...` were
  misclassified as generated output because their absolute paths contain
  `target`. File-role classification now strips the benchmark repo-cache prefix
  before applying generated/vendor filters.
- Materialized language-corpus repos under `target/oss-language-corpus/repos/...`
  had the same generated-output misclassification. The shared file-role
  classifier now strips both benchmark cache prefixes before role detection.
- Bash/nvm sidecar prep failed mandatory Qdrant semantic smoke because Windows
  verbatim file paths like `\\?\C:\...` produced pathless `dir:?/C:`
  component-report dense points. Runtime semantic graph context now normalizes
  verbatim paths, strips the common repo root for file-table paths, and groups
  root-level source files under `dir:.`; the semantic doc schema version was
  bumped to rebuild stale pathless docs.
- The A/B score wrapper now streams benchmark progress and exposes
  `--prepare-codestory-timeout-ms`, so full-suite prep no longer appears hung
  while the lower-level benchmark is indexing large repos.
- The agent A/B harness no longer relies on the nested agent to voluntarily run
  CodeStory first. It runs the packet prelude itself, records it in transcript
  analysis, counts prelude wall time separately, and injects a compact packet
  excerpt rather than the full structured packet into the nested prompt.
- The compact packet excerpt now keeps answer citations and claim text but does
  not repeat citation objects inside every covered claim.
- The CodeStory arm now treats a packet as complete for the benchmark row only
  when packet manifest quality passes. In that case, the prompt tells the
  nested agent not to spend tokens on follow-up commands solely because generic
  packet sufficiency is `partial`.
- The CodeStory arm is now packet-first but no longer packet-only by default.
  When packet manifest quality is incomplete, the nested agent may fall back to
  local source reads after CodeStory follow-ups, and those reads are counted as
  post-packet overhead.
- The no-CodeStory arm no longer relies on the nested agent to voluntarily
  inspect the repo. It runs a harness-owned local `rg` plus bounded file-read
  prelude, records those as shell/file-read command events, and feeds the
  resulting snippets to the baseline agent.
- Publishable gating now rejects a `without_codestory` row if it calls CodeStory
  or if it never inspects the local repository.
- Source-read accounting now recognizes nested PowerShell
  `Get-Content -LiteralPath` commands with stacked shell quotes, so post-packet
  fallback reads are not hidden as generic file-read commands.
- Runtime packet planning now protects prompt-named Java/TypeScript symbols and
  derives concrete probes for Java string checks and SWR hook/cache/mutation
  flow without requiring packet-only fallback.
- Runtime packet claims now derive Java `StringUtils.isBlank`/`isEmpty` and
  `CharSequenceUtils.regionMatches` semantics, plus SWR `useSWR`,
  serialization, cache-helper, and mutation-flow claims, from cited source.
- Runtime packet planning now treats Gin route dispatch as a server route flow,
  derives concrete Gin probes, and avoids client request-interceptor/transport
  adapter probes unless the prompt explicitly asks for those client concepts.
- File-scoped packet probes now require both the requested file and requested
  symbol, so `gin.go New` cannot be satisfied by `Engine.With` and `gin.go
  Default` cannot be satisfied by `binding.Default`.
- Runtime packet claims now derive Gin engine creation, default middleware,
  route registration, radix-tree insertion, request dispatch, and handler-chain
  progression claims from cited source.
- The CSS animate task now uses selectors from the pinned source tree
  (`.animated` and `.bounce`) instead of generated/docs `animate__` selectors.
- Runtime packet planning and claims now protect animate.css source files,
  source custom properties, base selector, imports, bounce keyframes, and flash
  keyframes.
- Runtime packet planning now detects Chinook SQL schema prompts, injects SQL
  seed-file/table/foreign-key probes, adds file citations for prompt-derived
  schema files, and derives Album/Track/InvoiceLine SQL relationship claims
  from source.
- Runtime packet planning now detects AutoMapper map-flow prompts, protects the
  core Mapper/MapperConfiguration/TypeMap/TypeMapPlanBuilder source anchors, and
  derives the runtime map/configuration/expression-plan claims from source.
- Runtime packet planning now detects MDN form-validation prompts, protects the
  native constraint and custom JavaScript validation anchors, derives the
  `novalidate`, `showError`, `ValidityState`, and submit-prevention claims from
  source, and adds static file citations for the four expected examples.
- Runtime packet planning now detects Okio buffer-flow prompts, protects the
  commonMain Buffer/Source/Sink/wrapper anchors, derives the byte-store and
  upstream wrapper claims from source, and adds static citations for the
  expected Kotlin files.
- Runtime packet planning now detects Monolog record-flow prompts, protects the
  Logger/LogRecord/handler source anchors, derives the expected handler
  registration, `LogRecord` creation, and processing-handler claims from source,
  and adds static citations for the expected PHP files.
- Runtime packet planning now detects Alamofire request-flow prompts, protects
  the Session/Request/DataRequest/SessionDelegate source anchors, derives the
  expected request creation, task resume, validation, and URLSession callback
  claims from source, and adds static citations for the expected Swift files.
- Packet-runtime cold probes and nested A/B repo groups now support `--jobs N`;
  CodeStory cache prep supports capped `--prepare-codestory-jobs N`; and the
  score wrapper supports `--packet-gate`, `--packet-probe-jobs N`,
  `--packet-gate-improved-from <run-dir>`, and strict
  `--reuse-baseline-from <run-dir>` for no-CodeStory baseline reuse.
- Forbidden-claim scoring no longer flags a contradicted positive claim such as
  `StringUtils.isEmpty does not trim whitespace...` as the forbidden opposite
  merely because `whitespace-only` contributes the token `only`.

## Verification

Commands run:

```powershell
cargo test -p codestory-runtime dense_policy_embeds_package_public_callables_for_dynamic_frameworks -- --nocapture
cargo test -p codestory-runtime component_reports_are_extracted_dense_anchors_with_virtual_ids -- --nocapture
cargo test -p codestory-runtime file_role_classification_catches_colocated_and_helper_tests -- --nocapture
cargo build --release -p codestory-cli
node --test scripts\tests\codestory-agent-ab-analyzer.test.mjs
node scripts\codestory-agent-ab-benchmark.mjs --self-test
node scripts\codestory-agent-ab-benchmark.mjs --task-suite language-expansion-holdout --task-ids python-requests-session-flow --arms without_codestory,with_codestory --repeats 1 --repo-cache-dir target\agent-benchmark\repos --materialize-repos --prepare-codestory-cache --allow-failures --out-dir target\agent-benchmark\packet-forced-ab-smoke-manifest-complete-stop-v2 --timeout-ms 600000
node scripts\codestory-agent-ab-benchmark.mjs --reanalyze-dir target\agent-benchmark\packet-forced-ab-smoke-manifest-complete-stop-v2 --publishable --task-suite language-expansion-holdout --task-ids python-requests-session-flow --repo-cache-dir target\agent-benchmark\repos --materialize-repos
node scripts\codestory-agent-ab-score.mjs --reanalyze-dir target\agent-benchmark\packet-forced-ab-smoke-manifest-complete-stop-v2
node scripts\codestory-agent-ab-score.mjs --task-ids java-commons-lang-string-utils,rust-ripgrep-search-pipeline,typescript-swr-hook-flow --repeats 1 --out-dir target\agent-benchmark\segment5-java-rust-typescript-smoke --timeout-ms 600000
node scripts\codestory-agent-ab-score.mjs --reanalyze-dir target\agent-benchmark\segment5-java-rust-typescript-smoke
node scripts\codestory-agent-ab-score.mjs --task-ids java-commons-lang-string-utils,typescript-swr-hook-flow --repeats 1 --out-dir target\agent-benchmark\segment6-java-typescript-fallback-ab --timeout-ms 600000
node scripts\codestory-agent-ab-score.mjs --reanalyze-dir target\agent-benchmark\segment6-java-typescript-fallback-ab
cargo test -p codestory-runtime packet_plan_derives -- --nocapture
cargo test -p codestory-runtime source_claims_name -- --nocapture
cargo test -p codestory-runtime component_reports -- --nocapture
cargo test -p codestory-runtime semantic_graph_context_uses_repo_relative_file_table_paths -- --nocapture
cargo test -p codestory-store file_role_classification_ignores_materialized_benchmark_repo_cache_prefix -- --nocapture
cargo test -p codestory-runtime
cargo build --release -p codestory-cli
node scripts\codestory-agent-ab-score.mjs --task-ids java-commons-lang-string-utils,typescript-swr-hook-flow --repeats 1 --out-dir target\agent-benchmark\segment7-runtime-probes-java-typescript-ab --timeout-ms 600000
target\release\codestory-cli.exe packet --project target\oss-language-corpus\repos\gin-gonic-gin --question "Trace how Gin creates an engine, registers routes through router groups, stores them in method trees, and dispatches handlers for a request. Cite the source files and name the supporting symbols." --budget compact --format json --task-class route-tracing
node scripts\codestory-agent-ab-score.mjs --task-ids go-gin-route-dispatch --repeats 1 --out-dir target\agent-benchmark\segment8-go-gin-route-ab --timeout-ms 600000 --prepare-codestory-timeout-ms 1800000
node scripts\codestory-agent-ab-score.mjs --reanalyze-dir target\agent-benchmark\segment8-go-gin-route-ab
target\release\codestory-cli.exe packet --project target\oss-language-corpus\repos\animate-css-animate-css --question "Explain how animate.css defines shared animation variables/base classes and connects named animation classes to keyframes. Cite the source files and name the supporting selectors or keyframes." --budget compact --format json --task-class architecture-explanation
node scripts\codestory-agent-ab-score.mjs --task-ids css-animate-base-and-keyframes --repeats 1 --out-dir target\agent-benchmark\segment8-css-animation-ab-v2 --timeout-ms 600000 --prepare-codestory-timeout-ms 1800000
node scripts\codestory-agent-ab-score.mjs --packet-gate --packet-probe-jobs 2 --task-ids css-animate-base-and-keyframes --repeats 1 --out-dir target\agent-benchmark\segment8-css-gated-reuse-smoke --reuse-baseline-from target\agent-benchmark\segment8-css-animation-ab-v2 --timeout-ms 600000 --prepare-codestory-timeout-ms 1800000
node scripts\codestory-agent-ab-benchmark.mjs --packet-runtime --packet-runtime-mode cold-cli --task-suite language-expansion-holdout --task-ids go-gin-route-dispatch,css-animate-base-and-keyframes --repeats 1 --repo-cache-dir target\oss-language-corpus\repos --materialize-repos --prepare-codestory-cache --jobs 2 --out-dir target\agent-benchmark\segment8-go-css-packet-runtime-jobs2 --timeout-ms 600000 --prepare-codestory-timeout-ms 1800000 --allow-failures
node scripts\codestory-agent-ab-benchmark.mjs --task-suite language-expansion-holdout --task-ids go-gin-route-dispatch,java-commons-lang-string-utils --arms without_codestory --repeats 1 --repo-cache-dir target\oss-language-corpus\repos --materialize-repos --reuse-baseline-from target\agent-benchmark\segment6-full-language-suite-r1-pathfix --jobs 2 --out-dir target\agent-benchmark\segment8-ab-jobs-reuse-smoke --timeout-ms 600000 --allow-failures
node scripts\codestory-agent-ab-benchmark.mjs --reanalyze-dir target\agent-benchmark\segment8-ab-jobs-reuse-smoke
node --check scripts\codestory-agent-ab-score.mjs
node --check scripts\codestory-agent-ab-benchmark.mjs
node --test scripts\tests\codestory-agent-ab-analyzer.test.mjs
cargo test -p codestory-runtime packet_plan_derives_chinook_sql_schema_symbol_probes -- --nocapture
cargo test -p codestory-runtime chinook_sql_schema_source_claims_name_tables_and_foreign_keys -- --nocapture
cargo build --release -p codestory-cli
target\release\codestory-cli.exe packet --project target\oss-language-corpus\repos\lerocha-chinook-database --question "Explain the core Chinook schema relationships between artists, albums, tracks, invoices, and invoice lines across the SQL seed scripts. Cite the source files and name the supporting tables or constraints." --budget compact --format json --task-class data-flow > target\agent-benchmark\segment9-sql-chinook-packet-probe.json
node scripts\codestory-agent-ab-score.mjs --packet-gate --packet-probe-jobs 1 --packet-gate-improved-from target\agent-benchmark\segment6-full-language-suite-r1-pathfix --task-ids sql-chinook-schema-relations --repeats 1 --out-dir target\agent-benchmark\segment9-sql-improved-gate-reuse-ab --reuse-baseline-from target\agent-benchmark\segment6-full-language-suite-r1-pathfix --timeout-ms 600000 --prepare-codestory-timeout-ms 1800000 --prepare-codestory-jobs 2
node scripts\codestory-agent-ab-benchmark.mjs --packet-runtime --packet-runtime-mode cold-cli --task-suite language-expansion-holdout --task-ids csharp-automapper-map-flow,kotlin-okio-buffer-flow,dart-http-client-flow,bash-nvm-install-dispatch,html-mdn-form-validation,ruby-jekyll-site-build,php-monolog-record-flow,swift-alamofire-request-flow,cpp-fmt-formatting-flow,rust-ripgrep-search-pipeline --repeats 1 --repo-cache-dir target\oss-language-corpus\repos --materialize-repos --prepare-codestory-cache --jobs 4 --prepare-codestory-jobs 2 --out-dir target\agent-benchmark\segment10-remaining-packet-probes --timeout-ms 600000 --prepare-codestory-timeout-ms 1800000 --allow-failures
cargo test -p codestory-runtime packet_plan_derives_automapper_map_flow_symbol_probes -- --nocapture
cargo test -p codestory-runtime automapper_map_flow_source_claims_name_runtime_configuration_and_plans -- --nocapture
cargo build --release -p codestory-cli
target\release\codestory-cli.exe packet --project target\oss-language-corpus\repos\AutoMapper-AutoMapper --question "Explain how AutoMapper configuration and runtime mapper APIs cooperate to map source objects to destination objects. Cite the source files and name the supporting symbols." --budget compact --format json --task-class architecture-explanation > target\agent-benchmark\segment11-csharp-automapper-packet-probe.json
node scripts\codestory-agent-ab-benchmark.mjs --packet-runtime --packet-runtime-mode cold-cli --task-suite language-expansion-holdout --task-ids csharp-automapper-map-flow --repeats 1 --repo-cache-dir target\oss-language-corpus\repos --materialize-repos --prepare-codestory-cache --jobs 1 --prepare-codestory-jobs 1 --out-dir target\agent-benchmark\segment11-csharp-packet-runtime --timeout-ms 600000 --prepare-codestory-timeout-ms 1800000 --allow-failures
node scripts\codestory-agent-ab-score.mjs --packet-gate --packet-probe-jobs 1 --packet-gate-improved-from target\agent-benchmark\segment6-full-language-suite-r1-pathfix --task-ids csharp-automapper-map-flow --repeats 1 --out-dir target\agent-benchmark\segment11-csharp-improved-gate-reuse-ab --reuse-baseline-from target\agent-benchmark\segment6-full-language-suite-r1-pathfix --timeout-ms 600000 --prepare-codestory-timeout-ms 1800000 --prepare-codestory-jobs 2
cargo test -p codestory-runtime packet_plan_derives_mdn_form_validation_symbol_probes -- --nocapture
cargo test -p codestory-runtime mdn_form_validation_source_claims_name_constraints_and_custom_validation -- --nocapture
cargo build --release -p codestory-cli
node scripts\codestory-agent-ab-benchmark.mjs --packet-runtime --packet-runtime-mode cold-cli --task-suite language-expansion-holdout --task-ids html-mdn-form-validation --repeats 1 --repo-cache-dir target\oss-language-corpus\repos --materialize-repos --prepare-codestory-cache --jobs 1 --prepare-codestory-jobs 1 --out-dir target\agent-benchmark\segment12-html-packet-runtime-v2 --timeout-ms 600000 --prepare-codestory-timeout-ms 1800000 --allow-failures
node scripts\codestory-agent-ab-score.mjs --packet-gate --packet-probe-jobs 1 --packet-gate-improved-from target\agent-benchmark\segment6-full-language-suite-r1-pathfix --task-ids html-mdn-form-validation --repeats 1 --out-dir target\agent-benchmark\segment12-html-improved-gate-reuse-ab-v2 --reuse-baseline-from target\agent-benchmark\segment6-full-language-suite-r1-pathfix --timeout-ms 600000 --prepare-codestory-timeout-ms 1800000 --prepare-codestory-jobs 2
cargo test -p codestory-runtime packet_plan_derives_okio_buffer_flow_symbol_probes -- --nocapture
cargo test -p codestory-runtime okio_buffer_flow_source_claims_name_buffers_and_wrappers -- --nocapture
cargo build --release -p codestory-cli
node scripts\codestory-agent-ab-benchmark.mjs --packet-runtime --packet-runtime-mode cold-cli --task-suite language-expansion-holdout --task-ids kotlin-okio-buffer-flow --repeats 1 --repo-cache-dir target\oss-language-corpus\repos --materialize-repos --prepare-codestory-cache --jobs 1 --prepare-codestory-jobs 1 --out-dir target\agent-benchmark\segment13-kotlin-packet-runtime --timeout-ms 600000 --prepare-codestory-timeout-ms 1800000 --allow-failures
node scripts\codestory-agent-ab-score.mjs --packet-gate --packet-probe-jobs 1 --packet-gate-improved-from target\agent-benchmark\segment6-full-language-suite-r1-pathfix --task-ids kotlin-okio-buffer-flow --repeats 1 --out-dir target\agent-benchmark\segment13-kotlin-improved-gate-reuse-ab --reuse-baseline-from target\agent-benchmark\segment6-full-language-suite-r1-pathfix --timeout-ms 600000 --prepare-codestory-timeout-ms 1800000 --prepare-codestory-jobs 2
cargo test -p codestory-runtime packet_plan_derives_monolog_record_flow_symbol_probes -- --nocapture
cargo test -p codestory-runtime monolog_record_flow_source_claims_name_logger_records_and_handlers -- --nocapture
cargo build --release -p codestory-cli
node scripts\codestory-agent-ab-benchmark.mjs --packet-runtime --packet-runtime-mode cold-cli --task-suite language-expansion-holdout --task-ids cpp-fmt-formatting-flow,dart-http-client-flow,ruby-jekyll-site-build,php-monolog-record-flow,swift-alamofire-request-flow,bash-nvm-install-dispatch --repeats 1 --repo-cache-dir target\oss-language-corpus\repos --materialize-repos --prepare-codestory-cache --jobs 4 --prepare-codestory-jobs 2 --out-dir target\agent-benchmark\segment7-remaining-packet-triage --timeout-ms 600000 --prepare-codestory-timeout-ms 1800000 --allow-failures
node scripts\codestory-agent-ab-benchmark.mjs --packet-runtime --packet-runtime-mode cold-cli --task-suite language-expansion-holdout --task-ids php-monolog-record-flow --repeats 1 --repo-cache-dir target\oss-language-corpus\repos --materialize-repos --prepare-codestory-cache --jobs 1 --prepare-codestory-jobs 1 --out-dir target\agent-benchmark\segment7-php-packet-runtime --timeout-ms 600000 --prepare-codestory-timeout-ms 1800000 --allow-failures
node scripts\codestory-agent-ab-score.mjs --packet-gate --packet-probe-jobs 1 --packet-gate-improved-from target\agent-benchmark\segment6-full-language-suite-r1-pathfix --task-ids php-monolog-record-flow --repeats 1 --out-dir target\agent-benchmark\segment7-php-improved-gate-reuse-ab --reuse-baseline-from target\agent-benchmark\segment6-full-language-suite-r1-pathfix --timeout-ms 600000 --prepare-codestory-timeout-ms 1800000 --prepare-codestory-jobs 2
cargo test -p codestory-runtime packet_plan_derives_alamofire_request_flow_symbol_probes -- --nocapture
cargo test -p codestory-runtime alamofire_request_flow_source_claims_name_request_validation_and_callbacks -- --nocapture
cargo build --release -p codestory-cli
node scripts\codestory-agent-ab-benchmark.mjs --packet-runtime --packet-runtime-mode cold-cli --task-suite language-expansion-holdout --task-ids swift-alamofire-request-flow --repeats 1 --repo-cache-dir target\oss-language-corpus\repos --materialize-repos --prepare-codestory-cache --jobs 1 --prepare-codestory-jobs 1 --out-dir target\agent-benchmark\segment7-swift-packet-runtime --timeout-ms 600000 --prepare-codestory-timeout-ms 1800000 --allow-failures
node scripts\codestory-agent-ab-score.mjs --packet-gate --packet-probe-jobs 1 --packet-gate-improved-from target\agent-benchmark\segment6-full-language-suite-r1-pathfix --task-ids swift-alamofire-request-flow --repeats 1 --out-dir target\agent-benchmark\segment7-swift-improved-gate-reuse-ab --reuse-baseline-from target\agent-benchmark\segment6-full-language-suite-r1-pathfix --timeout-ms 600000 --prepare-codestory-timeout-ms 1800000 --prepare-codestory-jobs 2
target\release\codestory-cli.exe retrieval index --project target\oss-language-corpus\repos\nvm-sh-nvm --refresh full
target\release\codestory-cli.exe retrieval status --project target\oss-language-corpus\repos\nvm-sh-nvm
node scripts\codestory-agent-ab-score.mjs --task-ids python-requests-session-flow,java-commons-lang-string-utils,rust-ripgrep-search-pipeline,javascript-express-routing-flow,typescript-swr-hook-flow,cpp-fmt-formatting-flow,c-redis-command-loop,go-gin-route-dispatch,ruby-jekyll-site-build,php-monolog-record-flow,csharp-automapper-map-flow,kotlin-okio-buffer-flow,swift-alamofire-request-flow,dart-http-client-flow,bash-nvm-install-dispatch,html-mdn-form-validation,css-animate-base-and-keyframes,sql-chinook-schema-relations --repeats 1 --out-dir target\agent-benchmark\segment6-full-language-suite-r1-pathfix --timeout-ms 600000 --prepare-codestory-timeout-ms 1800000
node scripts\codestory-agent-ab-score.mjs --reanalyze-dir target\agent-benchmark\segment6-full-language-suite-r1-pathfix
cargo test -p codestory-runtime packet_exact_family_steering -- --nocapture
cargo test -p codestory-runtime monolog -- --nocapture
cargo fmt --check
cargo check -p codestory-runtime -p codestory-cli
cargo build -p codestory-cli
$env:CODESTORY_PACKET_EXACT_FAMILY_STEERING = '0'
node scripts\codestory-agent-ab-benchmark.mjs --packet-runtime --packet-runtime-mode cold-cli --task-suite language-expansion-holdout --task-ids python-requests-session-flow,php-monolog-record-flow,swift-alamofire-request-flow --repeats 1 --repo-cache-dir target\oss-language-corpus\repos --materialize-repos --prepare-codestory-cache --jobs 3 --prepare-codestory-jobs 2 --out-dir target\agent-benchmark\segment8-no-family-steering-smoke-packets-rebuilt --timeout-ms 600000 --prepare-codestory-timeout-ms 1800000 --allow-failures
node scripts\codestory-agent-ab-benchmark.mjs --packet-runtime --packet-runtime-mode cold-cli --task-suite language-expansion-holdout --repeats 1 --repo-cache-dir target\oss-language-corpus\repos --materialize-repos --prepare-codestory-cache --jobs 6 --prepare-codestory-jobs 3 --out-dir target\agent-benchmark\segment8-no-family-steering-all-packets --timeout-ms 600000 --prepare-codestory-timeout-ms 1800000 --allow-failures
node scripts\codestory-agent-ab-benchmark.mjs --packet-runtime --packet-runtime-mode cold-cli --task-suite language-expansion-holdout --task-ids python-requests-session-flow,cpp-fmt-formatting-flow,go-gin-route-dispatch,ruby-jekyll-site-build,swift-alamofire-request-flow,css-animate-base-and-keyframes --repeats 1 --repo-cache-dir target\oss-language-corpus\repos --materialize-repos --prepare-codestory-cache --jobs 1 --prepare-codestory-jobs 1 --out-dir target\agent-benchmark\segment8-no-family-steering-failed-serial --timeout-ms 600000 --prepare-codestory-timeout-ms 1800000 --allow-failures
node scripts\codestory-agent-ab-score.mjs --packet-gate --packet-probe-jobs 1 --task-ids python-requests-session-flow,rust-ripgrep-search-pipeline,go-gin-route-dispatch,swift-alamofire-request-flow,bash-nvm-install-dispatch --repeats 1 --out-dir target\agent-benchmark\segment8-no-family-steering-ab-passrows --reuse-baseline-from target\agent-benchmark\segment6-full-language-suite-r1-pathfix --jobs 2 --timeout-ms 600000 --prepare-codestory-timeout-ms 1800000 --prepare-codestory-jobs 1
cargo test -p codestory-runtime shell_version_use_guard_claim_survives_without_exact_family_steering -- --nocapture
cargo fmt --check
cargo build -p codestory-cli
node scripts\codestory-agent-ab-benchmark.mjs --packet-runtime --packet-runtime-mode cold-cli --task-suite language-expansion-holdout --task-ids bash-nvm-install-dispatch --repeats 1 --repo-cache-dir target\oss-language-corpus\repos --materialize-repos --prepare-codestory-cache --jobs 1 --prepare-codestory-jobs 1 --out-dir target\agent-benchmark\segment8-no-family-steering-bash-manifestfix-packet --timeout-ms 600000 --prepare-codestory-timeout-ms 1800000 --allow-failures
node scripts\codestory-agent-ab-score.mjs --packet-gate --packet-probe-jobs 1 --task-ids bash-nvm-install-dispatch --repeats 1 --out-dir target\agent-benchmark\segment8-no-family-steering-bash-manifestfix-ab --jobs 1 --timeout-ms 600000 --prepare-codestory-timeout-ms 1800000 --prepare-codestory-jobs 1
node scripts\codestory-agent-ab-score.mjs --packet-gate --packet-probe-jobs 1 --task-ids python-requests-session-flow,rust-ripgrep-search-pipeline,go-gin-route-dispatch,swift-alamofire-request-flow,bash-nvm-install-dispatch --repeats 1 --out-dir target\agent-benchmark\segment8-no-family-steering-ab-passrows-manifestfix-fresh --jobs 2 --timeout-ms 600000 --prepare-codestory-timeout-ms 1800000 --prepare-codestory-jobs 1
node --test scripts\tests\codestory-agent-ab-analyzer.test.mjs
node scripts\codestory-agent-ab-score.mjs --reanalyze-dir target\agent-benchmark\segment8-no-family-steering-ab-passrows-manifestfix-fresh
node scripts\codestory-agent-ab-score.mjs --reanalyze-dir target\agent-benchmark\segment8-no-family-steering-bash-manifestfix-ab
node --check scripts\codestory-agent-ab-score.mjs
$env:CODESTORY_PACKET_EXACT_FAMILY_STEERING = '0'
node scripts\codestory-agent-ab-score.mjs --packet-gate --packet-probe-jobs 1 --task-ids python-requests-session-flow,typescript-swr-hook-flow,c-redis-command-loop,go-gin-route-dispatch,dart-http-client-flow,bash-nvm-install-dispatch --repeats 1 --out-dir target\agent-benchmark\segment8-no-family-steering-current6-ab-postreboot-retryfix --jobs 1 --prepare-codestory-jobs 1 --prepare-codestory-timeout-ms 1800000 --timeout-ms 600000
node scripts\codestory-agent-ab-benchmark.mjs --packet-runtime --packet-runtime-mode cold-cli --task-suite language-expansion-holdout --repeats 1 --repo-cache-dir target\oss-language-corpus\repos --materialize-repos --prepare-codestory-cache --jobs 1 --prepare-codestory-jobs 1 --out-dir target\agent-benchmark\segment8-no-family-steering-full-packets-postreboot-serial --timeout-ms 600000 --prepare-codestory-timeout-ms 1800000 --allow-failures
node scripts\codestory-agent-ab-score.mjs --packet-gate --packet-probe-jobs 1 --task-ids python-requests-session-flow,rust-ripgrep-search-pipeline,typescript-swr-hook-flow,c-redis-command-loop,go-gin-route-dispatch,dart-http-client-flow,bash-nvm-install-dispatch --repeats 1 --out-dir target\agent-benchmark\segment8-no-family-steering-current7-ab-postreboot-retryfix --reuse-baseline-from target\agent-benchmark\segment8-no-family-steering-current6-ab-postreboot-retryfix --jobs 1 --prepare-codestory-jobs 1 --prepare-codestory-timeout-ms 1800000 --timeout-ms 600000
node C:\Users\alber\source\repos\autoresearch\plugins\codex-autoresearch\scripts\autoresearch.mjs benchmark-lint --cwd C:\Users\alber\source\repos\codestory
```

The most recent full 18-language paired A/B artifact predates the CSS and Java
generic source-shape repairs. It exits 0 and emits `with_quality=9/18`,
`without_quality=7/18`, `with_packet_manifest_quality_passes=7/18`,
`token_ratio=1.539`, `all_in_wall_ratio=1.550`, and `total_tool_ratio=0.598`.
It remains historical evidence for why there is no promotion claim yet, not the
current packet-gated A/B slice.

Incremental CSS and Java source-shape result:

The latest two packet repairs are structural source-shape extractors rather
than exact family citations:

- CSS animation flow: detects stylesheet animation concepts from source-owned
  custom properties, base animation classes, named animation classes, and
  matching `@keyframes` blocks. The standalone packet gate passes all manifest
  metrics at `1.0` with no missed anchors:

```text
target/agent-benchmark/segment8-no-family-steering-css-generic-shape-packet
```

- Java string predicate flow: detects `isBlank`/`isEmpty` style boolean
  methods from source/Javadoc text, null-or-length handling, whitespace checks,
  and absence of trim/strip behavior for empty checks. The final standalone
  packet gate passes all manifest metrics at `1.0` with no missed anchors:

```text
target/agent-benchmark/segment8-no-family-steering-java-generic-string-predicate-packet-v2
```

The CSS one-row A/B was an efficiency win with equal quality (`1/1` versus
`1/1`): `32,092` CodeStory tokens versus `256,284` baseline tokens, `39,011 ms`
all-in versus `117,092 ms`, and `1` tool call versus `22`.

The current nine-row A/B rolls both changes into the active comparison:

```text
target/agent-benchmark/segment8-no-family-steering-current9-ab-java-css-generic-shapes
```

This raises the disabled-steering packet gate from the post-reboot `7/18` pass
set to `9/18`, but it is still not promotion evidence because the other nine
language rows fail packet quality and this is a one-repeat slice.

## Remaining Work

- Decide whether compact packets that pass manifest quality but remain
  generically `partial` should become `sufficient`, or whether benchmark row
  quality should remain the only stop signal for these A/B runs.
- Improve packet manifest quality beyond the current `9/18` full-suite pass
  rate. The most urgent remaining rows are the rows that still fail that gate:
  JavaScript, C++, Ruby, PHP, C#, Kotlin, Swift, HTML, and SQL.
- Stop adding new exact library-family detectors as if they were broad wins.
  The anti-overfit gate now proves the generalized manifest-probe path only
  quality-passes `9/18` rows without hidden family steering. Use that gate as a
  required check for future packet work.
- Fix packet-probe parallelism reliability. `--jobs 6` caused six sidecar
  availability failures that recovered under serial retry; `--jobs 2` still
  caused five sidecar availability failures that recovered under serial retry.
  The score wrapper now automatically retries transient packet-gate sidecar
  failures in isolated serial rows before selecting A/B tasks; keep this path
  covered before raising packet-probe concurrency.
- Fix packet latency. The latest clean serial disabled-steering gate misses the
  `18,000 ms` packet retrieval SLA on `2/18` rows: Java and Redis.
- Structural source-shape claims (`request creation`, `validation hook`,
  `delegate callback`, `handler pipeline`, `schema relation`) still need to be
  selected from code evidence rather than exact library names.
- The current anti-overfit A/B slice is now both a quality and efficiency win
  (`9/9` CodeStory quality versus `6/9` baseline), but it is still limited to
  the `9/18` rows that pass the disabled-steering packet gate. The next target
  is broadening that gate without restoring hidden exact-library detectors.
- Swift still fails the current disabled-steering packet gate while missing the
  `Request.resume` and `DataRequest.validate` claims. That should be fixed
  through generic resume-task and validation-hook source-shape claims, not
  Alamofire-only canned answers.
- Re-run the full 18-language paired A/B suite with `--repeats 3` only after
  packet quality is materially better than this one-repeat run.
- Use `--sandbox danger-full-access` only for trusted local smoke runs if
  `workspace-write` keeps hitting the Windows nested-shell launch failure.
- Promote only after all rows pass manifest quality, packet-first and
  no-CodeStory-baseline gates, clean pinned checkout provenance, local-only
  CodeStory cache provenance, and no web/remote context blockers.
