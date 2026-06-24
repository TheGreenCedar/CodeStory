# Testing Matrix

Choose the verification lane before running broad checks. Run Cargo
verifications serially in this repo when the lane needs them; the workspace
shares build locks. Examples use POSIX shell syntax. On Windows PowerShell, use
environment assignments such as `$env:NAME = "value"`.

```mermaid
flowchart TD
    change["What changed?"] --> docs["Docs or README only"]
    change["What changed?"] --> always["Always consider the fast lane first"]
    change --> indexer["Indexer, graph, or language work"]
    change --> store["Store, snapshot, trail, or search-doc work"]
    change --> runtime["Runtime, search, grounding, or orchestration work"]
    change --> cli["CLI args or output boundary work"]
    change --> bench["Bench or perf-surface work"]
    change --> e2e["Repo-scale semantic or cold-start behavior"]
    docs --> docs_checks["readback + git diff --check"]
    always --> workspace["fmt, check, targeted tests, clippy"]
    indexer --> fidelity["fidelity_regression, tictactoe_language_coverage, integration"]
    store --> store_tests["cargo test -p codestory-store"]
    runtime --> runtime_tests["cargo test -p codestory-runtime and retrieval_eval"]
    cli --> cli_tests["cargo test -p codestory-cli"]
    bench --> bench_checks["cargo check -p codestory-bench --benches"]
    e2e --> e2e_stats["release build + codestory_repo_e2e_stats"]
```

## Whole Workspace

```sh
cargo fmt --check
cargo check
cargo test
cargo clippy --all-targets -- -D warnings
```

These are the default broad checks for code changes after the lane picker says
workspace-wide proof is useful.

## Release And Version Bumps

`crates/codestory-cli/Cargo.toml` is the release version source. When bumping a
release version, update every `codestory-*` workspace crate version and
`Cargo.lock` in the same change.

```sh
node .github/scripts/check-workflow-policy.mjs
python .github/scripts/check-codestory-release.py --version <version>
```

Do not create or push `v*` tags manually. A synchronized version bump merged to
`main` runs the auto-release workflow, which creates the GitHub tag, release,
cross-platform `codestory-cli` archives, and `SHA256SUMS.txt`.

Binary release assets are packaging evidence only. They are not packet/search
readiness proof; keep using the sidecar evidence tiers below before claiming
agent-facing packet/search readiness.

## Docs-Only Fast Path

If you only changed `README.md` or `docs/**`, use the smallest credible lane:

```sh
git diff --check
```

Read the changed pages back before finishing. Only escalate to broader Cargo
checks if the doc change depends on new code behavior or command output.

## Indexer And Graph Fidelity

```sh
cargo test -p codestory-indexer --test fidelity_regression
cargo test -p codestory-indexer --test tictactoe_language_coverage
cargo test -p codestory-indexer --test integration
cargo test -p codestory-indexer --test trait_interface_resolution
```

Run these whenever the change affects parsing, extraction, semantic resolution, or graph fidelity.
Use the full test binaries above instead of filtered `cargo test` invocations.
Use [language-support.md](../architecture/language-support.md) when deciding
whether a language claim is parser-backed graph, structural collector, or only
a candidate parser compatibility record.

The opt-in OSS corpus lane checks every public language-support profile against a
pinned medium-sized open source project and compares a raw filesystem baseline
with CodeStory indexing of the same file set:

```sh
CODESTORY_RUN_OSS_LANGUAGE_CORPUS=1 cargo test -p codestory-indexer --test oss_language_corpus -- --ignored --nocapture
```

See [oss-language-corpus.md](../testing/oss-language-corpus.md) for PowerShell commands,
language filtering, cache configuration, and the JSONL report path.

That corpus is not the strict agent A/B comparison. For language-level
packet-runtime promotion evidence, run the manifest-backed holdout suite:

```sh
cargo build --release -p codestory-cli
node scripts/codestory-agent-ab-benchmark.mjs \
  --packet-runtime \
  --packet-runtime-mode both \
  --task-suite language-expansion-holdout \
  --repeats 3 \
  --materialize-repos \
  --jobs 4 \
  --prepare-codestory-jobs 2 \
  --codestory-cli ./target/release/codestory-cli \
  --out-dir target/agent-benchmark/language-expansion-publishable-full-form-command-shapes \
  --timeout-ms 180000 \
  --max-source-reads-after-packet 0 \
  --publishable
```

The packet-runtime artifact bundle must cover cold and warm modes, three repeats, row
concurrency `--jobs 4`, prepared sidecars, full sidecar provenance, no
`--allow-failures`, no quality misses, no sufficiency gaps, no post-packet
source reads for packet-only promotion, and no SLA misses.
Keep `--prepare-codestory-jobs` lower or capped; examples use `2` unless the
prep lane is intentionally serial.

With/without CodeStory A/B artifacts remain useful development comparisons for
elapsed time, tokens, estimated cost, observed tool calls, command counts,
source reads, post-packet source reads, and manifest quality gates. Stale
`--reuse-baseline-from` or fixed no-CodeStory comparisons are diagnostic unless
fingerprint-compatible, and they are never enough for packet-runtime promotion
by themselves.

## Store Changes

```sh
cargo test -p codestory-store
```

## Runtime Changes

```sh
cargo test -p codestory-runtime
cargo test -p codestory-runtime --test retrieval_eval
```

Run `retrieval_eval` when search or grounding quality may have changed. By default it verifies
that plain indexing fails closed for sidecar-primary search. To run the full quality assertions,
prepare real sidecars and set `CODESTORY_RETRIEVAL_EVAL_FULL_TESTS=1`.
The repo-scale runtime integration test is ignored by default because it indexes the full
`codestory` workspace and can exhaust memory on developer machines.
Only run it as an explicit heavy lane:

```sh
export CODESTORY_RUN_REPO_SCALE_TEST=1
cargo test -p codestory-runtime --test integration test_repo_scale_call_resolution -- --ignored --nocapture
```

## Repo-Scale Semantic And Cold-Start Checks

Run this lane when default `index` behavior, symbol-doc persistence, dense-anchor
persistence/reuse, embedding reuse, or cold-start performance changes:

```sh
cargo build --release -p codestory-cli
cargo test -p codestory-cli --test codestory_repo_e2e_stats -- --ignored --nocapture
```

The real-repo drill portion fails closed unless `CODESTORY_REAL_REPO_DRILL_CASES`
points at a prepared manifest. Use `CODESTORY_ALLOW_SKIP_REAL_REPO_DRILL_CASES=1`
only to make that separate drill skip explicit during local release-evidence
collection. A skipped drill means the release evidence is not real-repo drill
proof; it does not rename the `proof_tier` emitted by the stats JSON.

Append the emitted headline and phase metrics to
`docs/testing/codestory-e2e-stats-log.md`. Include graph seconds, semantic
seconds, symbol docs written, dense docs skipped, dense reason counts, dense
docs reused, dense docs embedded, total index seconds,
`repeat_full_refresh_seconds`, repeat graph/semantic/cache/search timings,
`retrieval_index_seconds`, `retrieval_status_seconds`, `report_seconds`,
`proof_tier`, any `warnings`, and whether
`sidecar_status_after_retrieval_index` plus `search.sidecar_shadow_retrieval_mode`
were `full`. The release stats harness reads the latest valid `Phase Metrics`
row in that log as its living warning baseline and reads that row's
`repeat full refresh <seconds>s` scenario text as the repeat full-refresh
blocker baseline.

Release-readiness evidence is tiered:

| Evidence tier | Required proof | Release meaning |
| --- | --- | --- |
| Stats-only / degraded sidecar | Diagnostic timing or contract evidence without prepared full sidecars, or stats output whose `proof_tier` is `stats_only` | Useful local regression signal only; not release proof for packet/search readiness. The current passing `codestory_repo_release_e2e_emits_stats` harness asserts full sidecar status instead of completing as a passing no-full-sidecar row. |
| Full sidecar | `codestory_repo_release_e2e_emits_stats` emits `proof_tier: "full_sidecar"` after local Zoekt, SCIP, and required dense-anchor Qdrant/llama.cpp are prepared; `retrieval index --refresh full` succeeds; `retrieval status --format json` reports `retrieval_mode: "full"` with current symbol-doc and dense-anchor manifest fields; and search shadow mode is `full` | Required before claiming agent-facing packet/search readiness on the current workspace. This is the normal tier for a passing stats JSON object from the release e2e stats harness. |
| Real-repo drill | `CODESTORY_REAL_REPO_DRILL_CASES` points at prepared manifests and the drill cases run without skip allowances | Required before claiming the release was exercised beyond the CodeStory checkout. |
| Promotion-grade benchmark | Full holdout packet-runtime rows cover cold and warm modes with three repeats, `--jobs 4`, prepared sidecars, `--publishable`, explicit `--max-source-reads-after-packet 0`, no `--allow-failures`, full sidecar provenance, no quality misses, no sufficiency gaps, and no SLA misses. Fixed-baseline A/B rows are supporting diagnostics only unless fingerprint-compatible. | Required for performance or retrieval-quality promotion claims. |

When logging release evidence, state the highest tier reached and the exact
skip env vars used. The stats JSON reports `proof_tier` as the highest tier
proven by that stats object. If `CODESTORY_ALLOW_SKIP_REAL_REPO_DRILL_CASES=1`
was used, record that the real-repo drill was intentionally skipped, but preserve
the stats JSON tier exactly; for example, a passing full-sidecar stats object
remains `full_sidecar`, not `stats_only`. Warning-free full-sidecar stats must
not be promoted to real-repo drill or promotion-grade evidence by themselves.

The stats JSON also reports `warnings` for performance thresholds that should
stay visible in logged evidence. Total index time and semantic phase warnings
are computed against the latest `Phase Metrics` row in
[`codestory-e2e-stats-log.md`](../testing/codestory-e2e-stats-log.md), not
against older hard-coded timing rows:

| Warning | Threshold |
| --- | --- |
| Total index time | `index_seconds` is more than 25% above the latest stats-log phase baseline |
| Semantic phase time | `semantic_phase_seconds` is more than 25% above the latest stats-log phase baseline |
| AST-first cold index gate | cold CodeStory product index is not under 180s or `semantic_embedding_ms` is not at least 70% below same-run baseline |

Preserve those warning strings when copying the run into release evidence. An
empty `warnings` array only means the measured run stayed under these warning
thresholds; it does not raise the proof tier.

For the current repo-scale baseline, use the latest row in
[`codestory-e2e-stats-log.md`](../testing/codestory-e2e-stats-log.md). Older
rows, including the 2026-04-18 durable-scope measurements, are historical
examples only; do not copy them into current performance claims.

## CLI Boundary And Output Changes

```sh
cargo test -p codestory-cli
```

Prefer this lane before `cargo test` for the whole workspace when the change is isolated to CLI args, rendering, or contract envelopes.

For CI agents or container images that need a single machine-readable local
readiness check, run:

```sh
codestory-cli smoke --project <repo> --profile ci-agent --format json
```

The profile indexes the local graph, grounds the repo, resolves one indexed
symbol, runs `affected` on a fake changed path, and reports sidecar full mode
only when the existing sidecar status already proves it. Non-full sidecars are
listed under `skipped_optional_surfaces` with repair hints.

Runtime-backed CLI fixture flows are a separate heavier lane:

```sh
cargo test -p codestory-cli --test runtime_backed_flows -- --ignored
```

Run that lane only when the change crosses CLI and runtime behavior together, such as auto-refresh handling or file-filtered symbol resolution.

The local real-repo agent-quality lane is ignored by default and must evaluate
at least one sibling repository when run:

```sh
cargo test -p codestory-cli --test agent_quality_eval -- --ignored --nocapture
```

Set `CODESTORY_ALLOW_SKIP_LOCAL_REAL_AGENT_QUALITY=1` only when intentionally
collecting skip-only local evidence because none of the sibling repositories are
present. A zero-evaluated run is not quality proof.

## Bench Surface Checks

```sh
node scripts/semantic-doc-leakage-check.mjs
cargo check -p codestory-bench --benches
```

When changing embedding backends, model profiles, pooling, prefixes, batching,
hardware-provider settings, generated symbol-doc text, or dense-anchor text, run
the semantic-doc leakage check before trusting benchmark scores. It fails when
production generated-doc concept phrases copy or closely overlap benchmark query
text. Also rerun the speed and retrieval-quality comparison described in
[`embedding-backend-benchmarks.md`](../testing/embedding-backend-benchmarks.md).
Start from the human summary in [`research.md`](../research.md). For new
research lanes, keep the benchmark case shape, quality signal, speed signal,
and decision current in the matrix instead of adding raw run transcripts.

For indexing performance work, run the full bench when practical:

```sh
cargo bench -p codestory-bench --bench indexing
```

For browser-scale stress work, start with the smoke lane and only opt into
larger synthetic repos when the machine and change justify it:

```sh
cargo bench -p codestory-bench --bench browser_stress
export CODESTORY_STRESS_SCALE=large # 1k + 10k
export CODESTORY_ALLOW_HEAVY_STRESS=1
cargo bench -p codestory-bench --bench browser_stress
```

The full `100k` synthetic lane is intentionally opt-in with
`CODESTORY_STRESS_SCALE=full`, `CODESTORY_ALLOW_HEAVY_STRESS=1`, and
`CODESTORY_ALLOW_100K_STRESS=1`. The Criterion concurrency lane is a
browser-service proxy for stdio/HTTP-shaped work, not transport promotion
proof. Synthetic stress results are promotion scouts only; promotion requires
at least one real repository run recorded with the same commit and command
shape. See
[`codestory-stress-lanes.md`](../testing/codestory-stress-lanes.md).
