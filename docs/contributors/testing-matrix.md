# Testing Matrix

**Audience:** Contributors

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
    docs --> docs_checks["readback + git diff --check + check-doc-links.mjs"]
    always --> workspace["draft: fmt, check, lib clippy, focused tests"]
    indexer --> fidelity["fidelity_regression, tictactoe_language_coverage, integration"]
    store --> store_tests["cargo test -p codestory-store"]
    runtime --> runtime_tests["cargo test -p codestory-runtime and retrieval_eval"]
    cli --> cli_tests["cargo test -p codestory-cli"]
    bench --> bench_checks["cargo check -p codestory-bench --bench name"]
    e2e --> e2e_stats["final merge-ready head: release build + repo-scale stats"]
```

## Verification Lane Summary

Run Cargo commands serially in this repo.

| Lane | Commands |
| --- | --- |
| Docs only | `git diff --check`, `node .github/scripts/check-doc-links.mjs` |
| Draft code | On Ubuntu: `cargo fmt --check`, `cargo check --workspace --locked`, library clippy, and focused publication contracts |
| Exact-head source proof | After independent review accepts the current head: `cargo test --workspace --locked`, then `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`, once |
| Scoped macOS source proof | On both `macos-15` and `macos-15-intel`: `cargo check --workspace --locked` plus setup self-tests, only for an explicitly promoted Mac-scoped head |
| Concurrent publication | `cargo test -p codestory-cli --test stdio_protocol_contracts two_stdio_processes_observe_only_complete_generations_during_real_refresh -- --nocapture`; draft CI runs this focused contract on Ubuntu |
| Publication fault recovery | `cargo test -p codestory-runtime publication_transitions_fail_or_cancel_atomically -- --nocapture`; `cargo test -p codestory-store staged_promotion_abort_recovers_old_or_complete_new_and_cleans_artifacts -- --nocapture`; draft CI runs both focused proofs on Ubuntu |
| Release-blocking fidelity | `cargo test -p codestory-indexer --test fidelity_regression`, `cargo test -p codestory-indexer --test tictactoe_language_coverage`, `cargo test -p codestory-runtime --test retrieval_eval` |
| Heavy repo-scale timing | Once on a promoted final merge-ready head: `cargo build --release --locked -p codestory-cli`, then `cargo test --locked -p codestory-cli --test codestory_repo_e2e_stats -- --ignored --nocapture`; upload the output and append its metrics before the final commit |

Append fresh headline rows to
[`codestory-e2e-stats-log.md`](../testing/codestory-e2e-stats-log.md) when
default indexing, semantic persistence, embedding reuse, or cold-start behavior
changes.

## Whole Workspace

```sh
cargo fmt --check
cargo check
cargo test
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

These are the default broad checks for code changes after the lane picker says
workspace-wide proof is useful.

CLI integration tests must launch `codestory-cli` through
`tests/test_support::cli_command` (or `command` for a supplied binary). The
helper assigns a process-and-test-thread state root and explicitly isolates the
cache, stdio cache, install identity, and plugin data. Do not serialize the
workspace suite or clean the real user cache to make a test pass. Broker tests
that cross a worker-thread boundary must carry their injected test cache root
into that thread. The `test-support` feature only exposes isolation controls;
the CLI test binary explicitly activates automatic named-thread isolation.
Stdio fixtures must drain spawned repair workers before their temporary project
and state roots are removed. A teardown timeout terminates the process tree,
preserves the fixture roots for diagnosis, and fails the test.

Use the isolation contract as the focused regression gate:

```sh
cargo test -p codestory-cli --test test_state_isolation
cargo test -p codestory-cli --bin codestory-cli observe_broker_snapshot_
```

The first command exercises a controlled decoy user profile and fails if CLI
state escapes the injected root or an integration test constructs the CLI
directly. The second command keeps the historically competing broker snapshot
tests in one default-concurrency run.

Do not use `cargo test --workspace --all-targets` as the routine broad test
gate. `--all-targets` expands into benchmark targets; Criterion benches are
compiled or run through the bench lane below.

An explicitly promoted Mac-scoped head runs source checks independently on
Apple Silicon (`macos-15`) and Intel (`macos-15-intel`) before packaging. Mac
setup changes also pass the retrieval setup self-test and the shared Node
worktree setup suite, which includes the current platform adapter smoke. Draft
pushes stay on Ubuntu. Run locally from an isolated cache when changing these
surfaces:

```sh
node scripts/setup-retrieval-env.mjs --self-test
node --test scripts/tests/codex-worktree-setup.test.mjs
```

The shared worktree suite proves that normal setup stops after the local
repository map and that only `--full-retrieval-proof` enters the full retrieval
lane. The adapter smoke verifies forwarding only; platform scripts do not own a
second setup implementation.

## Release And Version Bumps

`crates/codestory-cli/Cargo.toml` is the release version source. When bumping a
release version, update every `codestory-*` workspace crate version and
`Cargo.lock` in the same change.

```sh
npm ci --ignore-scripts
node .github/scripts/check-workflow-policy.mjs
node --test .github/scripts/check-workflow-policy.test.mjs
python .github/scripts/check-codestory-release.py --version <version>
```

The workflow policy checker parses YAML before inspecting triggers,
permissions, jobs, matrices, secrets, dependencies, conditions, and named step
commands. Keep its single exact-pinned parser dependency in `package-lock.json`;
comments and unrelated steps are not policy evidence.

After a `main` release, run the release version check on `dev/codestory-next`
with the released version before starting the next release lane.

After merging a `dev/codestory-next` promotion PR into `main`, verify the dev
branch survived the merge and still matches `main`:

```sh
git ls-remote --heads origin main dev/codestory-next
git rev-list --left-right --count origin/main...origin/dev/codestory-next
```

If GitHub deletes `dev/codestory-next`, restore it from the promoted `main`
commit before treating the release as complete.

Do not create or push `v*` tags manually. A synchronized version bump merged to
`main` runs the auto-release workflow, which creates the GitHub tag, release,
cross-platform `codestory-cli` archives, and `SHA256SUMS.txt`.

Binary release assets are packaging evidence only. They are not packet/search
readiness proof; keep using the sidecar evidence tiers below before claiming
agent-facing packet/search readiness. Release builds fail closed unless both
Mac binaries are Developer ID signed with hardened runtime and secure timestamp
and Apple notarization returns `Accepted` before the existing tarballs are
created. The transient notarization ZIP is evidence input, not a published asset.

Release and post-publish agent proof must also exercise the installed plugin
launcher with `--plugin-root plugins/codestory`. The packaged proof fails if
the MCP `resources/list` response does not expose both `codestory://status` and
`codestory://agent-guide`, and its `plugin-stdio-status.json` artifact records
the active plugin runtime plus observed and missing server-advertised MCP
resources. This proves the installed plugin launcher advertises the resources;
it is not Codex host/model visibility proof.

Packaged acceptance compiles each selected native target once. PR and
integration invocations package unsigned Mac candidates and reuse them for
package smoke, install checks, and protected Apple Silicon lifecycle proof.
Release invocation signs and notarizes each Mac binary after that single build
and before packaging, then reuses the signed archive for release checks and
publication. Its Metal job runs the same archive only for functional lifecycle
proof; signing evidence comes from the signing and post-publish checks. Draft
pushes never start this matrix. A
same-repository `platform-proof` label or coordinator dispatch rechecks the
exact PR head and requires a completed successful `full-source-gate` job for
that SHA, then routes script/test-guard-only changes to `none`, Mac lifecycle
changes to the two Mac targets, and cross-platform runtime or packaging changes
to all six targets. A coordinator can explicitly select `windows` to build only
the Windows x64 package for an external Windows proof; that scope skips Mac
source and Metal jobs and receives no signing credentials. Forks are rejected
before checkout and no proof lane uses `pull_request_target` to execute PR code.

The `review-accepted` label runs the full Ubuntu source gate once for that exact
head; the persistent label alone does not authorize later heads. After merge,
the coordinator selects integration mode in the same dispatcher for the
current `dev/codestory-next` SHA. That mode forces the full source, six-target
package, repo-scale stats, and packaged Metal workflows, then rechecks that dev
did not move while they ran.
Every lane cancels an older run for the same PR or proof identity. Cargo cache
keys use the toolchain, lockfile, feature set, and target triple, never a commit
SHA.

| Asset | Native runner | Required packaged proof |
| --- | --- | --- |
| Linux x64 | `ubuntu-latest`, plus `ubuntu:20.04` | Version, help, stdio shape, managed provisioning, stale-local grounding convergence, terminal shared-agent evidence, cleanup, the full-sidecar agent proof, and packaged-archive execution on glibc 2.31 |
| Linux arm64 | `ubuntu-24.04-arm` | Version, help, stdio shape, managed provisioning, stale-local grounding convergence, terminal shared-agent evidence, and cleanup |
| Windows x64 | `windows-latest` | Version, help, stdio shape, installer ownership self-test, managed provisioning, stale-local grounding convergence, terminal shared-agent evidence, and cleanup |
| Windows arm64 | `windows-11-arm` | Version, help, stdio shape, managed provisioning, stale-local grounding convergence, terminal shared-agent evidence, and cleanup |
| macOS x64 | `macos-15-intel` | Unsigned in PR/integration cells; Developer ID signed and notarized only in release/post-publish cells. Version, help, stdio shape, managed provisioning, stale-local grounding convergence, checksum-pinned native CPU retrieval without Docker/Qdrant or configuration, terminal shared-agent evidence, and cleanup; never Metal |
| macOS arm64 | `macos-15` | Unsigned in PR/integration cells; Developer ID signed and notarized only in release/post-publish cells. Version, help, stdio shape, managed provisioning, stale-local grounding convergence, terminal shared-agent evidence, and cleanup |

The managed convergence proof on every native runner uses an isolated project
with a complete publication, then introduces source drift while leaving the
sidecar manifest absent. It proves that managed status is observational,
`ground` serves a complete publication and owns activation, one shared-agent
attempt reaches durable terminal evidence, a newer local generation is
published, and packet/search remain blocked without verified accelerator smoke.
The terminal repair may be a fail-closed result on hosted hardware; this does
not prove full sidecars or GPU execution. macOS x64
package execution does not make Apple Silicon acceleration claims. macOS arm64
package execution does not close #887; live managed Metal endpoint survival
still needs reporter or equivalent Apple Silicon hardware evidence. The
minimum supported GNU/Linux userspace is glibc 2.31, proven by executing
the Linux x64 build in a digest-pinned Rust 1.95.0 Debian Bullseye container,
then executing the packaged archive in a digest-pinned `ubuntu:20.04`
container. The baseline probe records the container's glibc version and captures stdout,
stderr, and exit status for version, help, and stdio initialize, so loader or
symbol-version failures fail the job without losing diagnostics. This does not
claim musl support or extend the glibc baseline claim to Linux arm64.

The separate protected Apple Silicon workflow runs the packaged CLI and plugin
launcher on a self-hosted macOS 15 ARM64 runner. It is release-blocking and must
preserve cold, warm, endpoint-death, and repaired status/log/packet artifacts.
The cold path seeds a complete local publication, makes it stale with Agent
sidecars absent, confirms `codestory://status` changes no ownership, and invokes
only managed MCP `ground`. Before activation returns, MCP must observe the
worker adopt its exact cache-owned repair reservation. Canonical status polling
must observe one successful
`shared-agent` attempt, a newer local generation, `native_spawned` Metal,
an independent Metal runtime-init marker, positive offload, a bounded live
embed smoke, `gpu_proof=verified`, and full
retrieval before packet/search run through that same MCP session. A fresh MCP
process must then reuse the exact native PID and launch fingerprint without a
duplicate server. The remainder of the workflow proves readiness blocking after
endpoint death, explicit recovery, packet/search, and proof-owned cleanup. The
same protected run proves dynamic endpoint selection, live process identity,
and exact cache/process/container/port ownership before marker-scoped cleanup;
the following run also cleans a marker-owned prior attempt if cancellation
prevented the prior `always()` step. Contract tests or hosted package smoke
cannot replace this hardware evidence.

Proof cleanup validates the marker and archive before invoking the packaged
CLI's internal owned-deletion boundary. Cache and proof-root names are removed
relative to a pinned runner-temp handle; Unix traversal is descriptor-relative
and no-follow, while Windows rejects reparse ancestors and deletes by handle.
The platform boundary test swaps the ambient ancestor after the trusted handle
opens and must preserve an outside sentinel.

An actual Mac host reboot remains a separate two-phase operator proof because a
GitHub job cannot safely reboot its own self-hosted runner and resume the same
job. Preserve the pre-reboot PID/launch/status bundle, reboot the protected host,
then run the packaged warm/recovery workflow and attach the post-reboot
status/packet/search bundle. Do not describe the automated hardware job alone as
host-restart evidence.

PR and integration proofs deliberately package unsigned Mac candidates and do
not enter the `macos-release-signing` environment. The protected Metal job uses
that exact unsigned package for functional lifecycle proof. Only the
main-triggered release workflow requests Developer ID credentials; it signs and
notarizes each Mac binary once before publication, and the resulting artifact is
the one published and checked again after download. Apple service availability
therefore cannot block review, while a signing or notarization failure still
blocks the release before any Mac asset is published.

After publication, both Mac tarballs receive a download quarantine before
extraction. Because command-line tar does not propagate that xattr, the proof
records the archive quarantine and transfers the same event to the extracted
binary before `codesign --verify`, an `spctl` diagnostic, and native version
and help execution on each architecture. `spctl` treats a bare Mach-O CLI as
"not an app" even when Apple notarization is accepted, so quarantined native
execution is the release gate; the diagnostic is retained without requiring
application-bundle acceptance. Preserve the
release-time `notarytool` result and post-publish quarantine,
codesign/execution artifacts. Signing/notary material is scoped to the
protected `macos-release-signing` environment and written with owner-only
permissions. Release-time and post-publish proof require both the reported
`TeamIdentifier` and the designated-requirement certificate OU to match Apple
team `PKUJNR8D6F`. A Mac release is blocked when those credentials are
unavailable; publishing an unsigned fallback is not allowed.

Provision the signing values as environment secrets on
`macos-release-signing`, not only as repository secrets:

- `APPLE_DEVELOPER_ID_P12_BASE64`
- `APPLE_DEVELOPER_ID_P12_PASSWORD`
- `APPLE_SIGNING_IDENTITY` (the exact `Developer ID Application` certificate
  common name)
- `APPLE_NOTARY_KEY_P8_BASE64`
- `APPLE_NOTARY_KEY_ID`
- `APPLE_NOTARY_ISSUER_ID`

Create `macos-metal-release` with the required release protection rules, and
authorize a protected Apple Silicon self-hosted runner labelled `macOS`,
`ARM64`, and `codestory-metal`. Repository configuration is part of the release
gate: a checkout containing these workflows is not sufficient on its own.

Proof cleanup validates the exact current `lexical_data_dir`. During the v0.15
migration window it accepts the removed `zoekt_data_dir` spelling only as
read compatibility for an otherwise proof-owned state file; remove that alias
in v0.16. New state must emit only lexical path fields. When local retrieval
indexing fails, the proof records Compose process state and bounded Qdrant logs
before cleanup so the primary failure remains diagnosable; cleanup failure is
retained as secondary evidence and must not replace the primary gate failure.

Each native managed-plugin handoff also starts with a verified prior managed
CLI whose executable can answer only its version probe. The requested packaged
version must become the MCP server and active retention entry; the prior version
must remain only as the verified rollback. This is deterministic upgrade
convergence proof, not Apple Silicon endpoint-survival or older-glibc evidence.

Release closeout is not complete until the Mac workspace jobs, protected Metal
workflow, and every post-publish asset cell pass; a real fresh Codex install
from the marketplace reaches the expected status/packet behavior; the separate
two-phase protected-host reboot bundle is attached; and the
corresponding CodeStory plugin-source update is committed or merged in
`TheGreenCedar/AgentPluginMarketplace`. Finish with marketplace refresh plus
installed managed-runtime path/version and project-scoped status readback.
Source CI cannot substitute for that external pointer, fresh host install, or
installed-runtime proof.

## Docs-Only Fast Path

If you only changed documentation or plugin doc surfaces, use the smallest credible
lane. Scope matches the markdown link checker:

- `README.md`
- `docs/**` (including templates)
- `plugins/codestory/README.md`
- `plugins/codestory/docs/**`
- `plugins/codestory/skills/**`

```sh
git diff --check
node .github/scripts/check-doc-links.mjs
```

When plugin adapter files change, also run:

```sh
node --test plugins/codestory/tests/plugin-static.test.mjs
```

Read the changed pages back before finishing. Only escalate to broader Cargo
checks if the doc change depends on new code behavior or command output.

Do not add unit tests that assert documentation prose or required phrases.
Structure gates only: links, whitespace, plugin/runtime shape.

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
cargo build --release --locked -p codestory-cli
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

Run this lane once on the final merge-ready head when default `index` behavior,
symbol-doc persistence, dense-anchor persistence/reuse, embedding reuse, or
cold-start performance changes. Intermediate checkpoints do not append rows:

```sh
cargo build --release --locked -p codestory-cli
cargo test --locked -p codestory-cli --test codestory_repo_e2e_stats -- --ignored --nocapture --test-threads=1
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
`proof_tier`, and whether
`sidecar_status_after_retrieval_index` plus `search.sidecar_shadow_retrieval_mode`
were `full`. The log is telemetry only and cannot become a release baseline by
appending a row. Use the approved profile and release decision command in
[`performance-review-playbook.md`](../testing/performance-review-playbook.md).
The harness still emits its prior latest-row warnings and repeat-refresh
blocker as diagnostics; release workflow authorization comes only from the
attested gate below.

Release-readiness evidence is tiered:

Linux accelerator cells have the same evidence boundary. CI runs prove resolver,
manifest, compose, and log-marker contracts only; they do not prove CUDA, HIP,
Vulkan, SYCL, or OpenVINO live GPU execution unless the run is explicitly backed
by a GPU runner artifact. For Linux sidecar backend changes, attach manual or
nightly evidence for every backend described as live-supported. Contract-only
cells must stay labeled as contract-only in PRs, issues, and release notes.

| Evidence tier | Required proof | Release meaning |
| --- | --- | --- |
| Stats-only / degraded sidecar | Diagnostic timing or contract evidence without prepared full sidecars, or stats output whose `proof_tier` is `stats_only` | Useful local regression signal only; not release proof for packet/search readiness. The current passing `codestory_repo_release_e2e_emits_stats` harness asserts full sidecar status instead of completing as a passing no-full-sidecar row. |
| Full sidecar | `codestory_repo_release_e2e_emits_stats` emits `proof_tier: "full_sidecar"` after the project-local SQLite lexical shard, SCIP, and required dense-anchor Qdrant/llama.cpp are prepared; `retrieval index --refresh full` succeeds; `retrieval status --format json` reports `retrieval_mode: "full"` with current symbol-doc and dense-anchor manifest fields; and search shadow mode is `full` | Required before claiming agent-facing packet/search readiness on the current workspace. This is the normal tier for a passing stats JSON object from the release e2e stats harness. |
| Real-repo drill | `CODESTORY_REAL_REPO_DRILL_CASES` points at prepared manifests and the drill cases run without skip allowances | Required before claiming the release was exercised beyond the CodeStory checkout. |
| Promotion-grade benchmark | Full holdout packet-runtime rows cover cold and warm modes with three repeats, `--jobs 4`, prepared sidecars, `--publishable`, explicit `--max-source-reads-after-packet 0`, no `--allow-failures`, full sidecar provenance, no quality misses, no sufficiency gaps, and no SLA misses. Fixed-baseline A/B rows are supporting diagnostics only unless fingerprint-compatible. | Required for performance or retrieval-quality promotion claims. |

Packet/drill adapter promotion proof is a separate executable gate over one
already-finalized Agent sidecar generation:

```sh
node scripts/prove-drill-packet-parity.mjs \
  --project . \
  --run-id drill-packet-parity \
  --question "RuntimeContext" \
  --anchor RuntimeContext \
  --output-dir target/drill-packet-parity
```

The gate records both retrieval-status reads, the packet and drill command
transcript, generation identity, and artifact names in
`drill-packet-parity-evidence.json`. It fails before packet/drill execution when
`retrieval_mode` is not `full`, and reports the exact degraded reason instead of
promoting contract fixtures to live sidecar evidence. A passing run requires the
generation to remain unchanged and packet/drill sufficiency, citations, explicit
probes, and follow-up commands to match. It also rejects supplemental, anchor, or
separate bridge commands in the drill report. Evidence status is `blocked` only
for an observed non-full preflight; command, parsing, parity, and artifact errors
are `failed`.

When logging release evidence, state the highest tier reached and the exact
skip env vars used. The stats JSON reports `proof_tier` as the highest tier
proven by that stats object. If `CODESTORY_ALLOW_SKIP_REAL_REPO_DRILL_CASES=1`
was used, record that the real-repo drill was intentionally skipped, but preserve
the stats JSON tier exactly; for example, a passing full-sidecar stats object
remains `full_sidecar`, not `stats_only`. Full-sidecar stats must
not be promoted to real-repo drill or promotion-grade evidence by themselves.

Release-significant performance decisions are fail-closed. Normalize the raw
stats and packet-runtime artifacts into the seven-metric candidate described in
the performance playbook, then run
`scripts/codestory-release-evidence-gate.mjs`. The selected machine profile
must be approved, release-eligible, pinned to attested raw evidence, and match
the candidate's corpus, cache, and machine fingerprint. Candidate artifacts are
produced on the same clean full SHA by the reusable
`release-candidate-evidence.yml` workflow. `release.yml` calls it after
preflight and requires it to pass before packaged proof starts. A rejected
metric blocks the release unless a non-expired exception binds the exact
candidate hash, baseline id/hash, profile, metric, measured value, threshold,
owner, ISO date, and rationale. Preserve the emitted decision JSON; it carries
status, metric, decision, commit, and artifact paths/hashes.

The dedicated Linux ARM64 evidence profile explicitly allows CPU embeddings
from its checksum-verified preseeded model. Fresh measurements retain the raw
repo-scale log, stats JSON, and complete real-repo drill tree. Before a distinct
release-eligible baseline exists, artifact production succeeds but evaluation
must reject the first run; retained output alone is not acceptance.

The main-triggered release and the manual `release-evidence` mode of the
registered platform-proof coordinator can reach that self-hosted runner. The
coordinator's hosted resolver first requires a same-repository PR into
`dev/codestory-next`, its exact expected head, the `review-accepted` label, and
a successful exact-head source proof. Both entry points call the reusable
evidence workflow with fixed profile and runner paths. The selected SHA is
checked out behind the protected `release-evidence` environment; an ambient
dispatch ref is never substituted. Runner groups are unavailable for this
personal-account repository, so environment approval is the allocation guard.
The two live tests run serially and must prove their owned sidecars are gone
before the packet benchmark starts.

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
cargo test -p codestory-bench --test agent_quality_eval -- --ignored --nocapture
```

Set `CODESTORY_ALLOW_SKIP_LOCAL_REAL_AGENT_QUALITY=1` only when intentionally
collecting skip-only local evidence because none of the sibling repositories are
present. A zero-evaluated run is not quality proof.

## Bench Surface Checks

```sh
node scripts/lint-retrieval-generalization.mjs
cargo check -p codestory-bench --bench <name>
```

Criterion benches opt out of broad workspace test selection. Run them
explicitly with `cargo bench -p codestory-bench --bench <name>` when the lane
needs performance numbers. Use the same explicit `--bench <name>` form for
compile-only proof; aggregate `--benches` does not select benches that opt out
of broad workspace test selection.

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
