# Language Expansion A/B Report

Date: 2026-06-17

## Verdict

The language-expansion work now has strong development evidence, but it is not
yet broad public promotion proof and it is not a first-class claim for every
language/framework.

The corrected frame is:

- Framework and domain semantics are product semantics. React/Next routes,
  Express middleware, Gin handlers, ASP.NET endpoints, Rails controllers,
  Django views/models, LINQ-style flows, and similar concepts are not overfit
  merely because they are language- or framework-aware.
- Benchmark overfit is different: production code must not depend on holdout
  task ids, pinned benchmark repo names, fixture paths, one-off route names, or
  expected-answer wording.
- Parser-backed language support is not the same thing as first-class
  framework/domain support.

Current evidence says CodeStory is quality-equal or better on every measured
language task and materially cheaper overall. The current comparison is an
offline reused-baseline composite at
`target/agent-benchmark/language-expansion-holdout-20260617-fixed-baseline-vs-round24-codeonly-offline`.
It combines the fixed no-CodeStory baseline with the Round 24 CodeStory-only
confirmation; no harness arm was executed to create that artifact, and no
no-CodeStory rows were rerun.

The main remaining product gap is proof breadth, not the current holdout
quality score. The 18-task holdout composite is strong development evidence,
but broad language/framework promotion still needs repeat, freshness, breadth,
and promotion metadata.

## Evidence Ledger

| Slice | Raw evidence | Result | Use it for |
| --- | --- | --- | --- |
| Current fixed-baseline comparison | `target/agent-benchmark/language-expansion-holdout-20260617-fixed-baseline-vs-round24-codeonly-offline/reanalyzed-summary.json` and `.md` | CodeStory success `54/54` vs baseline `54/54`; CodeStory quality `54/54` vs baseline `24/54`. CodeStory used `3,383,687 ms` all-in wall vs `7,943,578 ms`, `2,141,124` tokens vs `9,692,559`, `54` commands vs `471`, and `0` source reads vs `417`. | Current development comparison. Use for quality, packet-first, and operational evidence, but keep public promotion blocked until repeat/freshness/breadth metadata is recorded. |
| Superseded 2026-06-16 full paired A/B | `target/agent-benchmark/language-expansion-holdout-20260616-0.8.0-retry/reanalyzed-summary.json` and `.md` | CodeStory success `54/54` vs baseline `51/54`; CodeStory quality `16/54` vs baseline quality `19/51` successful rows. CodeStory used `6,411,835 ms` all-in wall vs `7,523,716 ms`, `7,859,161` tokens vs `9,087,330`, `54` commands vs `471`, and `0` source reads vs `417`. Baseline failed all Ruby/Jekyll repeats. | Historical diagnostic evidence. Superseded by the fixed-baseline comparison for current readout. |
| First 2026-06-16 full-suite attempt | `target/agent-benchmark/language-expansion-holdout-20260616-0.8.0` | Cache preparation failed on a transient `zoekt_unreachable` status for `BurntSushi-ripgrep`. A direct retrieval status check reported full mode afterward; `retrieval up` preceded the successful retry. | Sidecar startup-race context only; not a scored A/B result. |
| Full 18-language paired A/B | `target/agent-benchmark/segment6-full-language-suite-r1-pathfix/reanalyzed-summary.json` and `.md` | CodeStory quality `9/18`; no-CodeStory quality `7/17` scored with one unsuccessful row. CodeStory used `13,060,265` tokens vs `8,191,771`, `4,014,646 ms` runner wall vs `3,094,988 ms`, and `4,796,792 ms` all-in wall vs `3,094,988 ms`. | Historical negative/diagnostic evidence. |
| Packet-eligible paired A/B | `target/agent-benchmark/segment8-no-family-steering-current9-ab-java-css-generic-shapes/reanalyzed-summary.json` and `.md` | CodeStory quality `9/9` vs no-CodeStory `6/9`; CodeStory used `291,788` tokens vs `5,346,265`, `502,289 ms` all-in wall vs `1,881,683 ms`, `9` commands vs `282`, and zero source reads vs `228`. | Narrow positive evidence for rows that were packet-eligible in that run. |
| Fresh 18-row packet runtime before sidecar fix | `target/agent-benchmark/language-expansion-packet-runtime-current-28717906/packet-runtime-summary.md`, `packet-composition.md`, and `quality-debug.json` | `13/18` rows produced scored packets, `7/13` scored rows passed manifest quality, `4/13` were partial, and `5/18` failed as hard `retrieval_unavailable` command failures. | Current diagnostic baseline before the sidecar unresolved-candidate fix. |
| Five-row sidecar unresolved-candidate fix slice | `target/agent-benchmark/language-expansion-packet-runtime-sidecar-unresolved-fix/packet-runtime-summary.md` and `quality-debug.json` | The five previously hard-failing rows all produced packet output. Quality passed `3/5` (`java`, `c`, `css`) and failed expected-claim recall `2/5` (`express`, `swift`). All five remained packet-partial because unresolved candidates and compact-budget truncation are now surfaced as sufficiency gaps instead of command failures. | Regression evidence for the sidecar strictness fix; not a substitute for a fresh full 18-row run. |
| Two-row product-claim semantics fix slice | `target/agent-benchmark/language-expansion-packet-runtime-claim-semantics-fix/packet-runtime-summary.md` and `quality-debug.json` | Express and Swift/URLSession both passed manifest quality after source-derived product claims replaced generic "supports/inspect" wording. Both remain packet-partial. | Regression evidence for production framework/domain semantics without enabling eval-only probes. |
| Historical 18-row packet runtime after early fixes | `target/agent-benchmark/language-expansion-packet-runtime-current-after-claim-fixes/packet-runtime-summary.json`, `.md`, `packet-composition.md`, and `quality-debug.json` | `18/18` command pass, `18/18` scored, `12/18` manifest-quality pass, `9/18` packet sufficient, `9/18` packet partial. Packet retrieval SLA misses remained on Java (`30,931 ms`), Redis (`30,313 ms`), and Okio (`20,799 ms`). | Historical packet-composition diagnostic, superseded for current comparison by the June 17 fixed-baseline composite. |

The 2026-06-17 fixed-baseline comparison supersedes the 2026-06-16 paired A/B
row for current development comparison evidence. The full packet-runtime
artifact remains useful for packet-composition archaeology, but it no longer
describes the current comparison result.

## Product Semantics vs Benchmark Overfit

### Keep

The framework route collectors in `crates/codestory-indexer/src/lib.rs` are
product semantics and should stay. They cover common route shapes for Express,
Fastify, Koa, Hono, React Router, SvelteKit, Next, Remix, Astro, Nuxt, Django,
Flask/FastAPI-style decorators, Spring, Axum/Actix/Rocket, Gin, Rails,
Laravel, and ASP.NET with explicit confidence labels. Ktor, Vapor, and Shelf
extractor fixtures exist, but they are not published in
`summary.framework_route_coverage` yet; treat them as extractor-level semantics
until the coverage matrix names status, gaps, and handler-link support.

These are not benchmark hacks. They are the kind of domain knowledge required
for first-class framework support.

### Move or Rename

Packet source-claim semantics have been moved out of the orchestrator into
named runtime profile modules:

- `packet_terms.rs` owns prompt/probe term extraction.
- `packet_source_patterns.rs` owns source-pattern primitives.
- `packet_claims.rs` owns ranked citation-to-claim synthesis and source
  definition claim extraction.
- `packet_claim_profiles.rs` owns product claim profiles such as server route,
  hook/cache, client-send, URLSession request lifecycle, string-predicate,
  stylesheet animation, SQL schema, runtime-formatting, and search-execution
  flows.
- `packet_command_profiles.rs` owns command-span probes and command-flow claim
  templates.
- `packet_evidence_roles.rs` owns typed citation role classification; labels
  leave that boundary only for user-facing text, trace rows, and claim keys.
- `packet_required_probes.rs` owns product-required probe expansion, concrete
  file probe adaptation, and citation/claim coverage matching.
- `packet_citations.rs` owns shared citation display/path/source helpers.
- `packet_capping.rs` owns citation budget-capping policy.
- `packet_sufficiency.rs` owns packet sufficiency thresholds, budget-blocking
  verdicts, gap text, command quoting, and follow-up command assembly.

That boundary is the intended architecture. New framework/domain steering
should land as a named profile or collector, not as another ad hoc branch inside
the orchestrator.

Indexing-flow packet probes now use product concepts such as indexing
entrypoint, file discovery, symbol extraction, storage persistence, search
projection, and snapshot refresh. Exact CodeStory fixture anchors such as
specific method names are test evidence or request-scoped diagnostics, not
production-required probes.

Search-execution packet probes and product claims now use generic product
concepts such as search entrypoint, flag parsing, candidate traversal, search
execution, parallel search, and result output. Ripgrep-shaped wording such as
`SearchWorker`, `haystack`, `walk_builder`, `PatternMatcher`, and
`flags::parse` remains benchmark/eval-only.

### Quarantine

Exact holdout probes and expected-claim shaping belong in benchmark manifests,
scorer inputs, request-scoped probes, or `eval_probes.rs`. The current runtime
quarantine is intentionally hard: in non-test builds,
`eval_probes_enabled()` returns `false`, so release CLI/runtime builds ignore
`CODESTORY_EVAL_PROBES`.

That means exact Requests, AutoMapper, Jekyll, and similar holdout probes are
not production steering in release builds. Keep that boundary. Express-style
route handoffs and URLSession request lifecycle claims are now production
semantics, but they are source-pattern-derived and pass the benchmark-overfit
lint instead of naming holdout repos or task ids. Exact ripgrep search-pipeline
wording stays in the holdout manifest and eval probes, while production search
semantics stay generic.

### Delete

No live production deletion target was confirmed in this pass. The concrete
bug found was not benchmark overfit; it was sidecar strictness. Packet batch
queries used to abort when a full-mode sidecar returned candidates from
docs/tests/non-symbol files that could not resolve to indexed graph symbols.
Those are now diagnostics and sufficiency gaps instead of command failures.

## Current Fixed-Baseline Read

### What Improved

- The with-CodeStory confirmation succeeded on every attempted row: `54/54`
  across 18 language tasks and three repeats.
- CodeStory quality-passed `54/54` rows versus `24/54` for the fixed
  no-CodeStory baseline.
- CodeStory reduced all-in wall time from `7,943,578 ms` to `3,383,687 ms`
  (`0.426` ratio).
- CodeStory reduced total tokens from `9,692,559` to `2,141,124` (`0.221`
  ratio).
- CodeStory reduced shell commands from `471` to `54` and direct source reads
  from `417` to `0`.
- CodeStory is quality-equal or better on every measured language task. Rows
  where baseline also passed `3/3` are still operational wins because
  CodeStory used fewer commands, fewer tokens, lower wall time, and zero direct
  source reads.

### What Still Fails

- This is an offline reused-baseline composite, not a fresh agent-executing
  paired run.
- The evidence is still holdout-shaped. It does not prove every supported
  framework/domain surface beyond the measured tasks.
- The promotion gap is now repeat/freshness/breadth metadata and out-of-sample
  support coverage, not the current holdout quality score.

### Current Quality Snapshot

Source:
`target/agent-benchmark/language-expansion-holdout-20260617-fixed-baseline-vs-round24-codeonly-offline/reanalyzed-summary.md`.

| Language | Task | With CodeStory quality | Baseline quality | With CodeStory file/citation recall | Read |
| --- | --- | ---: | ---: | ---: | --- |
| Python | Requests session flow | `3/3` | `3/3` | `100%` | Quality tie; CodeStory wins operationally and stays packet-first. |
| Java | Commons Lang string utility flow | `3/3` | `0/3` | `100%` | CodeStory quality win with lower command and source-read cost. |
| Rust | ripgrep search pipeline | `3/3` | `0/3` | `80%` | CodeStory quality win with lower command and source-read cost. |
| JavaScript | Express routing flow | `3/3` | `3/3` | `100%` | Quality tie; CodeStory wins operationally and stays packet-first. |
| TypeScript | SWR hook flow | `3/3` | `0/3` | `66.7%` | CodeStory quality win with lower command and source-read cost. |
| C++ | fmt formatting flow | `3/3` | `3/3` | `100%` | Quality tie; CodeStory wins operationally and stays packet-first. |
| C | Redis command loop | `3/3` | `3/3` | `100%` | Quality tie; CodeStory wins operationally and stays packet-first. |
| Go | Gin route dispatch | `3/3` | `3/3` | `100%` | Quality tie; CodeStory wins operationally and stays packet-first. |
| Ruby | Jekyll site build | `3/3` | `2/3` | `100%` | CodeStory quality win with lower command and source-read cost. |
| PHP | Monolog record flow | `3/3` | `2/3` | `100%` | CodeStory quality win with lower command and source-read cost. |
| C# | AutoMapper map flow | `3/3` | `0/3` | `100%` | CodeStory quality win with lower command and source-read cost. |
| Kotlin | Okio buffer flow | `3/3` | `0/3` | `66.7%` | CodeStory quality win with lower command and source-read cost. |
| Swift | Alamofire request flow | `3/3` | `1/3` | `100%` | CodeStory quality win with lower command and source-read cost. |
| Dart | HTTP client flow | `3/3` | `1/3` | `85.7%` | CodeStory quality win with lower command and source-read cost. |
| Bash | nvm install dispatch | `3/3` | `3/3` | `100%` | Quality tie; CodeStory wins operationally and stays packet-first. |
| HTML | MDN form validation | `3/3` | `0/3` | `100%` | CodeStory quality win with lower command and source-read cost. |
| CSS | animate.css base/keyframes | `3/3` | `0/3` | `100%` | CodeStory quality win with lower command and source-read cost. |
| SQL | Chinook schema relations | `3/3` | `0/3` | `100%` | CodeStory quality win with lower command and source-read cost. |

## Historical Packet Runtime Read

### What Improved

The five rows that previously failed before scoring produced packet output and
passed manifest quality in that historical full packet-runtime run:

- `java-commons-lang-string-utils`: quality pass, packet partial.
- `javascript-express-routing-flow`: quality pass, packet partial.
- `c-redis-command-loop`: quality pass, packet partial.
- `swift-alamofire-request-flow`: quality pass, packet partial.
- `css-animate-base-and-keyframes`: quality pass, packet partial.

This fixes the wrong failure mode. A full-mode sidecar candidate that cannot be
resolved to an indexed symbol is useful diagnostic evidence, not proof the
entire packet command is unavailable. It also shows that framework/domain
semantics can improve answer quality without leaking benchmark markers into
production code.

### What Still Fails

The remaining quality failures are mostly answer-semantics gaps, not missing
retrieval:

- Python Requests, Jekyll, Monolog, AutoMapper, Okio, and MDN/HTML still failed
  expected-claim recall in that historical packet-runtime run. Anchors were
  often present, but the answer surface does not consistently state causal
  handoffs.
- Some partial rows are compact-budget artifacts. They retain enough citations
  to be useful but still need follow-up commands before the packet can claim to
  be self-contained.
- Java, Redis, and Okio still miss the packet retrieval SLA.

## What This Proves

- The benchmark harness can compare strict no-CodeStory and CodeStory-first
  arms with wall time, token usage, command counts, direct source reads, web
  leakage, packet quality, and post-packet behavior.
- CodeStory is quality-equal or better on every measured language task in the
  current fixed-baseline composite and materially cheaper overall.
- CodeStory is clearly useful on the current packet-eligible slice and the
  current fixed-baseline development comparison.
- Parser-backed support exists for the languages listed in
  `crates/codestory-contracts/src/language_support.rs`, and HTML/CSS/SQL are
  explicitly structural-only.
- Sidecar unresolved-candidate handling no longer turns docs/tests/non-symbol
  hits into packet command failures.
- Express-style route and URLSession request lifecycle claims can be generated
  from source patterns in production builds without enabling eval-only probes.
- Runtime packet source claims are now named product profiles rather than
  generic orchestration branches.
- The next frontier is breadth and promotion proof across framework/domain
  surfaces, not simply adding more file extensions.

## What This Does Not Prove

- It does not prove a public broad 18-language A/B win by itself because the
  current readout is an offline reused-baseline composite.
- It does not prove broad answer-quality superiority across every supported
  framework/domain surface beyond the measured holdout tasks.
- It does not prove every public language-support profile has equal semantic
  resolution, graph depth, framework support, or packet sufficiency.
- It does not prove React, LINQ, Rails, Django, ASP.NET, or any other framework
  is complete. Framework support requires explicit framework/domain semantics.
- It does not justify public savings claims or default promotion language.

## Durable Boundaries

- Public language support claims come from
  `crates/codestory-contracts/src/language_support.rs`.
- Workspace filtering may keep compatibility-only extensions such as `svelte`,
  `vue`, `astro`, `cshtml`, `scss`, `sass`, `less`, `ps1`, and `psm1`, but those
  are not public parser-backed claims unless the registry says so.
- Benchmark-specific probes live outside production behavior.
- Ripgrep-shaped search-pipeline answer templates live outside production
  behavior.
- Production framework/domain semantics should stay named as profiles or
  collectors, not hidden as generic language steering.

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

Run a fresh packet-runtime diagnostic after runtime changes:

```powershell
cargo build --release -p codestory-cli
node scripts\codestory-agent-ab-benchmark.mjs `
  --packet-runtime `
  --packet-runtime-mode cold-cli `
  --task-suite language-expansion-holdout `
  --repeats 1 `
  --repo-cache-dir target\oss-language-corpus\repos `
  --materialize-repos `
  --jobs 4 `
  --prepare-codestory-jobs 2 `
  --out-dir target\agent-benchmark\language-expansion-packet-runtime-current `
  --codestory-cli target\release\codestory-cli.exe `
  --timeout-ms 180000 `
  --allow-failures
```

Run the repaired five-row slice:

```powershell
node scripts\codestory-agent-ab-benchmark.mjs `
  --packet-runtime `
  --packet-runtime-mode cold-cli `
  --task-suite language-expansion-holdout `
  --task-ids java-commons-lang-string-utils,javascript-express-routing-flow,c-redis-command-loop,swift-alamofire-request-flow,css-animate-base-and-keyframes `
  --repeats 1 `
  --repo-cache-dir target\oss-language-corpus\repos `
  --materialize-repos `
  --jobs 4 `
  --prepare-codestory-jobs 2 `
  --out-dir target\agent-benchmark\language-expansion-packet-runtime-sidecar-unresolved-fix `
  --codestory-cli target\release\codestory-cli.exe `
  --timeout-ms 180000 `
  --allow-failures
```

Run the focused claim-semantics slice:

```powershell
node scripts\codestory-agent-ab-benchmark.mjs `
  --packet-runtime `
  --packet-runtime-mode cold-cli `
  --task-suite language-expansion-holdout `
  --task-ids javascript-express-routing-flow,swift-alamofire-request-flow `
  --repeats 1 `
  --repo-cache-dir target\oss-language-corpus\repos `
  --materialize-repos `
  --jobs 2 `
  --prepare-codestory-jobs 2 `
  --out-dir target\agent-benchmark\language-expansion-packet-runtime-claim-semantics-fix `
  --codestory-cli target\release\codestory-cli.exe `
  --timeout-ms 180000 `
  --allow-failures
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

Run a full language comparison while reusing the fixed no-CodeStory baseline:

```powershell
node scripts\codestory-agent-ab-benchmark.mjs `
  --task-suite language-expansion-holdout `
  --arms without_codestory,with_codestory `
  --repeats 3 `
  --materialize-repos `
  --prepare-codestory-cache `
  --reuse-baseline-from target\agent-benchmark\language-expansion-holdout-20260617-baseline-j4 `
  --out-dir target\agent-benchmark\language-expansion-holdout `
  --timeout-ms 600000
```

Do not rerun the no-CodeStory arm for the current harness context. With
`--reuse-baseline-from`, matching without-CodeStory rows are reused from the
fixed artifact rather than executed. A new no-CodeStory control artifact is
only justified when the task suite, pinned repo state, harness contract, or
scorer boundary changes with explicit approval.

## Promotion Blockers

- Record promotion metadata for the current fixed-baseline comparison:
  freshness, repeat, breadth, and the exact no-CodeStory control artifact.
- Add out-of-sample breadth evidence for supported framework/domain surfaces
  that are not represented by the 18 holdout tasks.
- Keep newly added framework/domain claims source-pattern-derived, linted, and
  owned by named profiles or collectors.
- Keep sidecar strictness fail-closed for unavailable/degraded sidecar modes
  while preserving unresolved full-mode candidate diagnostics.
- Reuse the fixed no-CodeStory control. Generate a new control artifact only
  when the task suite, pinned repo state, harness contract, or scorer boundary
  changes with explicit approval.
