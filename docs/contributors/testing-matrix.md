# Testing Matrix

Choose the smallest lane that can disprove the change. Run Cargo commands
serially because this workspace shares target locks. Draft work uses focused
tests; broad source and package gates run once on an accepted exact head.

All dependency-resolving Cargo commands use `--locked`. Do not use
`cargo test --workspace --all-targets` as a routine gate because it expands
Criterion targets.

## Lane summary

| Change | Focused proof | Exact-head proof |
| --- | --- | --- |
| Rust formatting or local logic | `cargo fmt --all -- --check`; owning crate tests | Workspace check/test/clippy |
| Store/publication | Store tests plus named fault/concurrency cases | Workspace source gate |
| Retrieval/embedding | Retrieval tests, runtime admission tests, engine proof self-test | Same-run performance gate, optional exact-candidate quality report, and required hardware proof |
| CLI/stdio | Named CLI contract suites | Workspace source gate and packaged proof when package behavior changed |
| Plugin launcher or CodeStoryDev staging | Installer tests plus `plugin-static` | Packaged plugin handoff |
| Worktree setup | Node suite plus one platform adapter smoke | Mac/Windows platform cell when adapter changed |
| Docs only | Read changed pages, doc links, `git diff --check` | No package matrix |
| Release/version | Release and workflow policy scripts | Main-only signing, notarization, publish, install, and live runtime proof |

`retrieval-engine-smoke.yml` runs the sub-second `architecture_contracts`
binary in its universal `linux-contracts` job. Store, indexer, workspace, and
contracts changes additionally run the path-scoped, artifact-free
`crate-durability.yml` lane with these serial commands:

```bash
cargo test --locked -p codestory-store
cargo test --locked -p codestory-indexer --test fidelity_regression
cargo test --locked -p codestory-indexer --test tictactoe_language_coverage
```

That lane has its own exact-key Cargo cache, derives the key from the Rust host
and manifests plus `Cargo.lock`, and saves only after all three commands pass.
It does not emit artifacts or turn unrelated crate changes into durability
work. Run broad source proof once on the frozen candidate rather than using
this focused durability lane as a second source-proof coordinator.

The same universal `linux-contracts` job also runs the merged proof suites as
a blocking per-PR lane, so evidence classification, packet sufficiency,
readiness leases, hook installation, and the confined workspace reader cannot
regress between dispatch-gated workspace proofs:

```bash
cargo test --locked -p codestory-runtime --lib agent::packet_evidence::
cargo test --locked -p codestory-runtime --lib agent::packet_sufficiency::
cargo test --locked -p codestory-runtime --lib agent::packet_batch::
cargo test --locked -p codestory-runtime --lib tests::search_scoring_tests::
cargo test --locked -p codestory-runtime --lib services::
cargo test --locked -p codestory-cli --lib
cargo test --locked -p codestory-workspace
```

## Draft source checks

Run the relevant focused commands while implementing. A typical Rust lane is:

```sh
cargo fmt --all -- --check
cargo test --locked -p <owning-crate> <focused-filter>
cargo check --locked -p <owning-crate>
```

Do not serialize tests to hide leaked global state. CLI integration tests use
their isolated test support, never the real user cache, and drain anything they
start.

MCP resource or snippet-contract changes run the complete
`stdio_protocol_contracts` binary, regenerate and check the MCP catalog, and
run `plugin-static`. Resource proof covers strict Unix/Windows path
round-tripping, malformed and conflicting selectors, static project-free
resources, observational status reads, and interleaved A/B/A repository and
node isolation. Snippet proof covers the canonical scope/context inputs, both
documented aliases, conflicts, unknown fields, and actual function-body
selection through the runtime owner.

Artifact-cache access-policy changes prove four separate boundaries with
focused tests: a file-backed `known_empty` full refresh still uses the
capacity-one pipeline without opening a reader; verified copied structural rows
use structural read-through while parser reads stay disabled; repeat
incremental work still reuses retained parser and structural rows; and an
injected writer or collector failure preserves the previous publication. Check
the parser and structural telemetry independently, including policy, logical
lookups, physical queries, hits, misses, reader opens, and lookup wall time.
Journal or checkpoint lanes are required only when those store contracts
change.

Projection-persistence changes prove one commit per nonempty owning batch with
SQLite commit/authorizer hooks, exact row/byte/statement counters, serial versus
bounded-pipeline parity, and atomic file-error plus dirty-state replacement.
Deny each persisted row family and the final commit in turn; every failure must
roll back graph rows, errors, and dirty markers together. Bound-input bytes are
logical statement payload, so representative evidence records database and WAL
bytes separately. Also inject cached metadata and cached-error-clear failures:
their error-only file outcomes must use the fallback replacement path without
discarding the previous projection. Journal/checkpoint policy and
multiple-writer changes remain separate lanes.

## Exact-head source gate

After independent review finds no blocker, run once on the unchanged head:

```sh
cargo fmt --all -- --check
cargo check --workspace --locked
cargo test --workspace --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
```

Run the two indexer acceptance binaries in full when parser, extraction,
resolution, language coverage, or retrieval document production changed:

```sh
cargo test --locked -p codestory-indexer --test fidelity_regression
cargo test --locked -p codestory-indexer --test tictactoe_language_coverage
```

The repo-scale stats lane runs once on the final merge-ready head only when
default indexing, symbol/dense persistence, embedding reuse, or cold-start
behavior changed. It is standalone telemetry, not a platform-release gate, and
intermediate commits do not append it. Use the coordinator's explicit `none`
scope when final integration requires source proof without package or protected
hardware jobs.
Use the explicit `linux` scope when the same exact final-dev integration should
also build, install, and exercise the Linux x64 Vulkan candidate on its
single-project server-behavior boundary without scheduling Mac or Windows
protected runners.

Semantic document allocation changes use focused runtime proof before the
broad gate. Cover shared-file path cardinality, byte-identical and
deterministically ordered symbol documents and dense inputs, cancellation, and
injected staged-publication failure. Telemetry must distinguish selected
symbols, retained file/path state, streamed node pages, cache-isolated endpoint
rows/query batches, and peak page-local lookup entries. Full-refresh streaming
also requires an integer-primary-key query-plan proof with no temporary sort,
cross-page shared endpoints, exact component-report accumulation, and
old-or-new publication survival on cancellation or injected node/edge reads.
Incremental dependency-scope streaming remains a separate change.

Semantic projection-only publication additionally proves the v29-to-v30
publication-mode migration preserves the prior row, an explicit CLI writer is
the only entry point, and a complete core can republish after its source file is
removed. Missing or incompatible stored symbol documents must fail closed;
cancellation and a competing writer must preserve the previous complete
publication and leave no staged artifact. The proof must also show that the new
core uses `semantic_projection`, its dense and structural manifests bind the
new generation, and no retrieval generation is synthesized. Do not substitute
a corpus rerun for these focused identity and fault tests. The post-commit
`RuntimeCache` failure/cancellation lane must use the public controller path and
show that the committed core and prepared search generation converge, indexing
state clears, retrieval remains bound to the prior core, and no incomplete
search or staged database artifacts remain.

## Retrieval engine

The supported product path is one packaged executable whose hidden mode owns
one automatically spawned per-user CodeRankEmbed Q8 server. It performs no
model or backend download and opens no TCP port. Compatible clients use a
private same-user UDS or named pipe and have no in-process fallback.
`retrieval_mode=full` still gates agent packet/search.

Focused proof covers:

- canonical model-contract parsing by both acquisition and Cargo build paths;
- explicit offline acquisition, missing-source, and digest-mismatch failures;
- a process-free Cargo build boundary that requires an explicit regular file
  for release builds;
- embedded-model digest and atomic materialization;
- linked ggml build identity;
- explicit `accelerated` policy with CPU embeddings disabled;
- prohibited silent CPU fallback and software-adapter rejection;
- live embedding smoke plus post-encode backend observations for execution
  device/backend, layer placement, resident tensor count/bytes, execution nodes,
  and an advancing successful-encode counter;
- one endpoint authority, listener, server, engine owner, native worker, load
  generation, and model load shared across independent client processes;
- 64-entry query and bulk queues, FIFO within each class, query preference
  between bulk batches, bulk resumption, cancellation, useful retry state, and
  no project/scope round-robin or bounded-starvation claim;
- client death, server crash, worker stall, incompatible-owner handoff,
  whole-server freeze without takeover, 60-second true-idle exit, and automatic
  respawn;
- publication leases that retain one load generation through commit;
- generation-coherent query reads and producer migration;
- cleanup confined to proved owned generations.

The activation-independent contract lane is:

```sh
node --test scripts/tests/prepare-embedded-model.test.mjs
cargo test --locked -p codestory-llama-sys --test model_staging
cargo check --locked -p codestory-llama-sys
```

The Node test uses synthetic model bytes and proves the build script has no
process-launch surface. The Rust staging test executes deterministic short-copy,
partial-write, and competing-destination faults against the same staging module
used by `build.rs`; it proves partial bytes are never published and a racing
destination is never replaced. Protected package and hardware lanes remain
responsible for proving the real release model and accelerator runtime.

The manually dispatched `qualification` mode runs the named
`Prove fresh-target Node-absent network-denied Cargo release boundary` once,
after all selected packages succeed. Corrective package-only iterations,
ordinary platform reruns, calibration, integration, and the later main release
do not repeat it. The job
seeds a new isolated Cargo home, mounts the source read-only into the pinned
build image, removes Node from the execution contract, denies container network
access, and runs both `cargo check --release --locked --offline` and
`cargo build --release --locked --offline` from a fresh target. The container's
`--network none` boundary is the network-denial proof; Cargo's offline flag
alone is not treated as OS-level denial. This proves the Cargo release boundary,
not that the separately packaged Linux archive was produced by that discarded
fresh-target build.

CPU embeddings are unsupported in package, calibration, qualification, and
release-proof jobs. Source-only contract tests may exercise CPU rejection, but
cannot emit product or release evidence.

Runtime-constant calibration runs three clean protected Apple Silicon Metal
generations with one sample per metric per run. It builds and packages once,
prepares the projects and model once, and performs no lifecycle, fault,
true-idle, memory, retrieval-quality, or accelerator qualification. A manually
dispatched Linux Vulkan calibration may emit optional diagnostic evidence, but
it does not feed or block the frozen calibration bundle.

Frozen-candidate qualification is a separate one-run-per-platform lane.
Metal and Windows Vulkan each run the full lifecycle, fault, true-idle, memory,
and accelerator suite once. Protected Linux Vulkan may run that same
qualification through a standalone dispatch when its GPU runner is online; it
is not a coordinator closeout dependency and cannot block qualification when
that runner is absent.

Answer quality is a separate, optional frozen-candidate adjunct. After the
protected Metal package proof, it runs the checksum-bound Axios JavaScript and
TypeScript v2 task for three cold-CLI repeats against the same authenticated
macOS archive. Its failure or absence cannot block Metal, Windows, Linux, or
closeout, and the standard release makes no answer-quality claim. Promotion and
release decisions consume the coordinator's `closeout` job result directly;
they do not wait for this optional job or for workflow-wide completion.

### Packaged proof

`.github/scripts/check-packaged-agent-proof.py` verifies a checksum-pinned
archive in an isolated offline environment. Packaging first inspects the
executable format, architecture, and actual PE imports, ELF `DT_NEEDED`, or
Mach-O load commands. It requires the target-specific linkage/loading contract,
rejects a mandatory Vulkan-loader dependency in the base Windows/Linux
executable, and verifies every packaged core/CPU/Vulkan module and native
dependency against the engine marker, embedded model contract, compiled
backends, llama source, and producer version. The resulting
`codestory-native-manifest.json` records compiled capability without claiming
runtime accelerator execution. Manifest schema 3 also binds the exact source
commit and tree, executable digest, server protocol, accepted constant set, and
measurement protocol. `--version-only` proves package structure, version, and
help; it does not prove a running server.

Protected and installed qualification use the ordinary plugin launcher with
two independently started host processes and different repositories.
`--server-behavior-only` is the smaller release path: one host grounds one
project, waits for search readiness in that same project, and verifies the
resident engine and server against the package manifest. It rejects
calibration and quality inputs and makes no two-host or broader lifecycle
claim.

`--proof-tier calibration` collects draft runtime-constant measurements from a
private synthetic project, but cannot satisfy a package, hardware, installed,
or release claim. It never accepts a repository project, plugin root, or plugin
handoff. A higher qualification tier requires a frozen constant set and a
retained qualification record.
A packaged proof handed an authenticated calibration bundle -- the manually
dispatched `qualification` frozen-candidate lane -- runs one extra
`--version-only --proof-tier hosted_package` invocation with
`--enforce-calibration-freeze-lineage`, which requires the calibration commit to
be an ancestor of the packaged commit with
`crates/codestory-llama-sys/per-user-embedding-server-constant-set.json` as the
only differing path. Release ordering is therefore bump-then-calibrate: bump the
version, calibrate on the bumped tree, then freeze and release. A
calibrate-then-bump ordering fails the guard, which names the offending paths
and the required ordering in its failure message. Dropping the flag does not
weaken that invocation, it breaks it: a `--version-only` proof rejects
calibration inputs unless the lineage is enforced.
`--produce-qualification-evidence` requires the separate
`codestory-embedding-qualification` driver through `--qualification-driver`.
The harness passes the exact packaged executable to that driver through
`--cli`; the driver orchestrates private nonce-gated worker calls without
shipping the suite inside `codestory-cli`. It writes the path passed to
`--qualification-evidence`; without the producer flag, the harness verifies an
existing record. Missing, stale, partial, self-selected, or wrong-tier evidence
fails.

macOS packages keep the selected backend built in. Windows and Linux packages
ship the runtime executable and native modules in one immutable generation
selected by the public launcher through a single atomic pointer. Help, status,
and local navigation do not require a Vulkan loader, but broad retrieval does.
Optional Linux constant calibration runs only on the protected Vulkan host and
cannot feed or block the frozen bundle.

Non-calibration protected and installed tiers use `--plugin-handoff`,
`--engine-policy accelerated`, `--expected-backend`, and `--offline` to make
the claim explicit. Constant calibration uses only its synthetic-project
collector flags and keeps proof output outside the initially empty retained
calibration directory. The harness self-test uses synthetic fixtures only:

```sh
python .github/scripts/check-packaged-agent-proof.py --self-test
```

### Hardware claims

| Workflow | Required claim |
| --- | --- |
| `.github/workflows/macos-metal-proof.yml` | Exact Apple Silicon package, CPU disallowed, Metal, physical adapter, live smoke, full layer offload, and project-scoped grounding |
| `.github/workflows/windows-vulkan-proof.yml` | Exact Windows x64 package, CPU disallowed, Vulkan, physical adapter, live smoke, full layer offload, and project-scoped grounding |
| `.github/workflows/linux-vulkan-proof.yml` | Exact Linux x64 package, CPU disallowed, Vulkan, physical adapter, live smoke, and project-scoped grounding |

Signing and notarization are main-release concerns, not PR gates. A PR package
may be unsigned while still proving the named package/runtime tier.

### Performance and quality

Before replacing a model or native embedding implementation, compare incumbent
and candidate in the same release build on the same machine. Keep that
measurement selector private and delete it before merge. A server-ownership
cutover does not relabel pre-fault and post-fault searches as two
implementations. The separate frozen-candidate quality adjunct consumes the
existing `publishable-three-repeat-packet/v1` evaluation contract and derives
the pass rate from every scoped Axios v2 row and repeat. Freeze every
production timing value and qualification threshold before running the
unchanged qualification candidate; a result cannot define its own pass
threshold.

Measure existing-owner connect, listener spawn, first residency, first product
ready, warm query/bulk IPC, bulk documents and tokens per second, useful retry
latency, true-idle exit, total CodeStory process memory, accelerator residency,
retrieval quality, multi-process reuse, and restart reuse separately. Retrieval
quality remains evaluation evidence, not a required lifecycle-qualification
metric. Use
awake-time monotonic clocks within each process; never subtract timestamps from
different process origins. Report quality separately as unclaimed optional
evidence; its absence or result does not gate qualification or release. A
repeatable throughput, warm-latency, or memory regression blocks the cutover;
5% is measurement noise, not an accepted sustained loss.

Historical reference: 368-372 documents/sec, 84.7 ms cross-repository search
p95, MRR@10 0.9824, Hit@10 1.0, Hit@1 0.973, and 829-1,020 MB peak working set.

## Store and publication

Changes to promotion or pinned reads run the owning store/retrieval tests plus
named fault and race cases for:

- prepared versus committed journal recovery;
- cleanup failure after a committed publication;
- stale/invalid backup ambiguity;
- successful first and replacement publication telemetry, including
  incremental live-to-staged copy bytes, optional rollback-backup phases,
  candidate/prior/backup SQLite logical bytes (`page_count * page_size`), and
  exact named-plus-residual reconciliation inside the promotion wall;
- structural-unit descriptor determinism across all twelve unit collectors,
  exact source spans, cross-file content-versus-placement identity, and
  zero-unit projection completeness;
- dedicated workflow, Compose, Cargo, OpenAPI JSON/YAML, and parser-backed Bash
  precedence over generic structural routing;
- centralized path-policy rejection before metadata/content reads, source-byte
  and unit-count bounds, ancestor-name independence, incremental removal of
  pre-policy rows, cache-version migration, and no partial projection or cache
  rows after a bound;
- Markdown fence, YAML block-scalar/URL, TOML multiline-string, shell heredoc,
  and PowerShell block-comment false-anchor suppression;
- distinct malformed, binary/non-UTF-8, and unreadable coverage round trips,
  plus previous-publication survival for those outcomes;
- structural cache compatibility, corruption, restored-mtime source changes,
  per-file incremental replacement, and structural-only copy-forward;
- missing, legacy, corrupt, or source-drifted structural manifests at full,
  incremental, promotion, and rollback fences;
- source drift at the publication fence;
- core, retrieval, vector-evidence, and engine changes during a query;
- exact dense-anchor ID/hash coverage and corrupt/non-finite/unnormalized vector rejection;
- evidence serialization, unknown schema, incompatible model/semantics/engine,
  and publication-identity mismatch;
- handle-relative cleanup during an ancestor swap.

Evidence must show that failure leaves the previous complete publication usable
and never deletes an outside sentinel. Query drift must return typed
`publication_changed`; runtime may retry the complete query-and-resolution
operation once, never an internal fragment against a newly current generation.
Telemetry-only promotion work must also keep candidate, previous, backup,
promoted-live, manifest, quick-check, journal, fsync, restore, rollback, and
cleanup ordering unchanged; the measurements are successful-path diagnostics
and do not weaken failure behavior. Use a bounded generated SQLite image for
copy/byte accounting and do not substitute a repo-scale or corpus run.
Structural evidence tests must also show that grounding, search, details, and
packet paths read persisted producer/tier/resolution metadata in batches where
the surface is batched, never infer provenance from a filename, and retain
structural evidence as diagnostic and non-sufficient.

## CLI and plugin

CLI args/rendering use named contract suites before the broad gate. Stdio tests
must send an absolute `project` on every request and prove multi-project routing
does not depend on active-state files.

Packet-probe changes additionally prove deterministic tagged serialization,
legacy normalization, native Unix/Windows exact-path containment,
valid-uncovered and text-only distinctions, stable ambiguity ordering,
stale-ID and continuation rejection, CLI/stdio schema parity, and that probes
cannot promote sufficiency or route order. A named exact-path fixture resolves
without first invoking broad grounding or retrieval. Stable-ID fixtures use
duplicate display names to prove exact citations retain node identity, and
schema/adapter fixtures enforce the combined 16-probe and 240-character limits.

Plugin adapter changes run:

```sh
node --test scripts/tests/install-codestory-dev-plugin.test.mjs
node --test plugins/codestory/tests/plugin-static.test.mjs
```

The normal user surface reports `ready`, `preparing`, or `unavailable` and does
not expose engine lifecycle or ask for consent. Maintainer diagnostics may show
backend/device identity.

## Worktree setup

The Node dispatcher owns CLI selection/version validation, optional `sccache`,
locked fallback build, rehydrate, refresh, and retrieval status. Shell and
PowerShell are thin adapters.

```sh
node --test scripts/tests/codex-worktree-setup.test.mjs
```

The suite includes one adapter smoke on the current platform. Mac and Windows
cells supply the other platform evidence when those adapters change.

## Docs-only fast path

Docs-only scope is `README.md`, `docs/**`, `plugins/codestory/README.md`,
`plugins/codestory/docs/**`, and `plugins/codestory/skills/**`.

```sh
node .github/scripts/check-doc-links.mjs
git diff --check
```

Read every changed page back. Do not add tests that assert prose.

## Workflow and release automation

Workflow edits run:

```sh
npm ci --ignore-scripts
node scripts/codestory-release-claims.mjs validate --repo .
node --test scripts/tests/codestory-release-claims.test.mjs scripts/tests/codestory-release-cell-manifest.test.mjs scripts/tests/codestory-release-closeout.test.mjs scripts/tests/codestory-release-evidence-gate.test.mjs
node --test .github/scripts/run-actionlint.test.mjs
node .github/scripts/run-actionlint.mjs
node .github/scripts/check-workflow-policy.mjs
node --test .github/scripts/check-workflow-policy.test.mjs
node --test .github/scripts/windows-link-timing.test.mjs
node .github/scripts/route-ci-proof.mjs --self-test
```

Windows package timing reports cache restore, native setup, `cargo_graph`,
`msvc_link`, feature probe, packaging, and artifact transfer as separate
intervals. `msvc_link` comes from `.github/scripts/windows-link-timing.mjs`,
which selects explicit `link /TIME` boundaries out of the captured build trace
and writes `windows-link-timing.json` beside it; a build log that only mentions
the crate named `time` leaves the phase `unavailable`. Linker timing is
observational, so an unavailable phase never invalidates an authenticated
package.

Exact source and package jobs keep dependency downloads, compiler objects, and
release artifacts separate. Compiler keys end in the exact candidate SHA but
restore through a compatibility prefix bound to the platform, target, Rust and
native toolchains, generator, features, lockfile, Cargo configuration, and
relevant native inputs. A restored compiler cache still produces and verifies a
fresh exact-head binary and archive. The isolated sccache store is bounded at
1 GiB, except for Windows packaging's 2 GiB mixed Rust, MSVC, Vulkan, and
embedded-model working set; dependency inputs have their own 1 GiB bound.
Successful compilation is saved before tests, signing, packaging, or protected
proof. Cache logs name the requested and restored keys, compatibility hit,
restored bytes, compilation time, and save result.

The base-branch retrieval lane seeds the five draft publication-proof test
targets with serial `cargo test --no-run` commands before it saves its cache.
Draft CI first requests the complete retrieval key, then same-topology prior-lock
draft and retrieval prefixes. Those prefixes retain runner, Rust version, host
target, feature topology, proof-topology version and command digest, and the
complete workspace-manifest hash; only the lockfile hash is omitted. A full
retrieval-key match is a compatible seed. A prior-lock prefix match is partial
Cargo reuse even though both are reported as `cache-hit=false` against the draft
primary, so evidence must use the reported matched key to distinguish them.

The workflow-dispatch-only Windows manifest-missing lane installs the repository's
checksum-pinned Vulkan SDK before it compiles and runs the real locked
`ready_command` integration target. Any CPU-selector coverage in that lane is
a test-only rejection or compatibility contract, not runtime evidence. Its
exact-only cache binds the hosted OS, Rust release, host target, versioned proof
shape, Ninja generator, CMake and Ninja versions, default feature topology,
workspace and vendor manifests, installer script, and lockfile. It has no
fallback prefixes, reruns the full contract on an exact hit, and saves the
exact primary only after the proof succeeds.

Every Windows source-build proof lane sets `CMAKE_GENERATOR=Ninja`. This keeps
llama.cpp nested native builds serialized under the repository's supported
generator instead of inheriting a hosted Visual Studio/MSBuild generator. The
hosted package cache also binds that generator and its CMake/Ninja tool versions;
the protected Vulkan lane pins the same generator before building its package
and records both tool versions in the retained host evidence.

That Windows lane is source and protocol evidence on a hosted runner without
protected GPU evidence. The
SDK preserves the production-default native compile topology; it does not prove
Vulkan execution, a packaged archive, an installed runtime, or protected
hardware behavior. Those claims remain with the package and protected Windows
Vulkan proof lanes.

`release-claims.json` is the release claim and proof-tier source of truth. It
separates the six standard release claims from optional performance and
answer-quality evaluations. The standard release requires
exact source, package, native platform, protected accelerator, and installed
runtime evidence plus bounded packet/search readiness for all three supported
targets.
Optional evaluation may reject its
own run, but it is not a dependency of packaging, hardware proof, closeout, or
publication. Workflow policy enforces that separation.

The same graph declares the exact release-closeout cells. For v0.16,
`workflow_policy.package_matrix` contains `macos-arm64`, `windows-x64`, and
`linux-x64`.
The coordinator retains canonical copies under `manifests/` and `evaluations/`
beside `ledger.json` and `summary.json`. A pre-publish run accepts ten cells:
exact source, three package identities, three accelerator-execution receipts,
and three candidate-installed behavior receipts. A post-publish run accepts
twenty-two cells after adding platform, marketplace-catalog-resolved behavior,
downloaded-byte proof, and protected-package retrieval-readiness for all three
targets.
Package rows record each archive name, byte count, and SHA-256.
A post-publish run requires
that accepted pre-publish ledger, requires its current package manifests to
match the retained rows, and rejects any downloaded archive whose bytes do not
match the retained digest. Producer and installed-runtime versions must equal
the independently supplied closeout version. Platform and installed-runtime
hosts must match the OS and architecture derived from the package matrix's Rust
target. Do not use `matrix`, `mixed`, or another
aggregate placeholder for a host, runner, backend, installer, or native-engine
identity.

Production producers use `scripts/codestory-release-cell-manifest.mjs`. They
emit cells only after their job succeeds and bind workflow, job, run, attempt
and Actions artifact identity. Artifact names are immutable and attempt
qualified. The closeout job queries the current run's Actions artifact and job
APIs, selects the highest attempt in which each graph-owned job actually ran,
requires that latest execution to have succeeded, and binds the selected
container id, digest, creation window and unflattened directory to its cells in
`codestory.release-actions-provenance/v1`. Loose JSON, expired or duplicate
containers, a failed newer execution, and artifacts outside the selected job's
time window are rejected. This permits **Re-run failed jobs** after a partial
post-publish failure: cells from jobs that did not rerun retain their earlier
attempt, while rerun cells use the newer successful attempt. Do not use
**Re-run all jobs** as post-publish recovery; publication is intentionally not
repeatable after the tag and release exist.

Every other release-chain upload is rerun-safe as well. Retained diagnostics
use attempt-qualified names. Stable intermediate artifacts that a later job
downloads by name use explicit replacement from a policy-owned allowlist, so a
retried producer cannot fail on an immutable same-name artifact before it emits
its authenticated cell. Terminal evidence is never overwriteable.

The v0.16 closeout consumes physical Metal and Vulkan execution evidence and
makes those accelerator claims for the released targets. Accuracy, latency,
and throughput remain independent evaluator lanes and release non-claims.

### Withheld accelerator claims

The repository owns one host per accelerator, so a host that loses its
connection mid-proof used to cost the whole release. Recovery is automatic and
bounded, and it never waits on a human click.

`.github/scripts/lost-runner-recovery.mjs` classifies a failed job as a runner
communication loss only when all three parts of the Actions signature are
present at once: the exact `The self-hosted runner lost communication with the
server.` annotation, at least one step that completed with an empty conclusion,
and no uploaded log blob. A proof that ran and failed its own assertions has a
real conclusion on every step and a log, so it is classified as an assertion
failure and is never re-dispatched and never withheld. The classifier never
reads job names.

All three parts are read by `.github/scripts/collect-actions-job-evidence.sh`,
which fails closed on every one of them. The annotation endpoint needs the
`checks: read` token scope; without it the call 403s, and a 403 reported as "no
annotations" would make the signature unmatchable and the whole recovery path
inert. The collector treats any answer other than a successful read as an
error, and only a `404` from the log-blob endpoint counts as "the runner
uploaded no log". `.github/scripts/check-workflow-policy.mjs` refuses any
workflow that runs the collector without `checks: read`, including the
reusable-workflow callers whose grant is the ceiling for what they call.

`.github/workflows/lost-runner-rerun.yml` watches completed release runs and
re-dispatches the individual lost jobs by id. The bound is
`non_claim_policy.maximum_run_attempts` (2, meaning one automatic recovery
attempt) and it counts **lost executions of that job**, not run attempts: a
release re-run for an unrelated reason has spent no recovery on any host, so
the first loss of a runner is still owed its one retry. The collector reads
every attempt of the run to make that count possible, and counts a job Actions
carried forward unchanged once. Jobs that failed on their own assertions are
not named in the rerun request and stay red.

If a host is lost twice, `release.yml`'s `accelerator-non-claim` job records a
**populated non-claim** for that host in place of the cells that host would
have produced. It mirrors the package manifest's own shape:
`runtime_execution: not_proven_by_package` with a `non_claim_reason`. Every
cell that host owns -- accelerator execution, candidate-installed behavior, and
retrieval readiness -- is written with evidence status `withheld`, naming the
target, backend, runner, the unavailable producer job, the exact annotation,
and every claim the missing proof would have carried.

The closeout does not take the producer's word for any of that. Its own job
runs the same collector and `buildTrustedProducerMap` re-derives the signature
before it will authenticate a cell against the non-claim producer, so a red
accelerator job cannot become an accepted withheld claim through a bug or a
future edit in the producer alone.

A withheld cell is recorded as `withheld` in `ledger.json` and in
`summary.json`'s `withheld_cells`, `withheld_hosts`, and `counts.withheld`, and
is never counted as passed. Any cell that still claims a pass while something
it rests on was withheld fails closeout validation, so a withheld accelerator
claim cannot be inherited as a silent pass by retrieval readiness.

**How much may be withheld** is `non_claim_policy.withhold_policy` in
`release-claims.json`, and the closeout enforces it:

- `maximum_withheld_hosts` (1) bounds how many protected hosts may be silent at
  once. The graph refuses a cap that does not leave at least one host proven, so
  "no accelerator was proven anywhere" is unrepresentable rather than merely
  discouraged.
- `claims_requiring_proof` names the claims that must keep at least one
  *passing* cell in any phase that closes them. A withheld cell records a
  non-claim and can never satisfy one.

Breaking either records a named `input_errors` entry and the closeout decision
becomes `reject`, so `pre-publish-closeout` fails and `publish` is skipped.

The two claim lists in the ledger are literal in both directions:
`withheld_claims` is what nothing in that phase proved, and
`partially_withheld_claims` is what a withheld cell rested on but another host
still proved. Their union is every claim a withheld cell touched.

The published surfaces say the same thing. The GitHub release notes' platform
section is rendered from the accepted ledger --
`codestory-release-claims.mjs release-platform-notes` requires `--ledger` and
has no graph-only mode -- and `release-closeout-summary.json` ships as a
release asset, so a consumer can read what a specific release proved without
reaching into a 30-day Actions artifact.

Run the coordinator only with retained producer manifests and a fresh output
directory:

```sh
node scripts/codestory-release-closeout.mjs evaluate \
  --repo . \
  --expected-sha <full-commit> \
  --version <version> \
  --phase pre_publish \
  --evaluated-at <canonical-ISO-timestamp> \
  --trusted-producers <actions-provenance-map.json> \
  --manifest-dir <unflattened-selected-artifact-directories> \
  --out-dir <new-closeout-directory>
```

For `--phase post_publish`, pass every graph-owned cell plus
`--pre-publish-ledger <accepted-pre-publish-ledger.json>`. The framework can be
tested and merged without final evidence; an accepted ledger still requires
the frozen exact-head producer manifests and does not upgrade source or package
proof into installed, protected-hardware, or live-behavior proof.

Pre-publish authorization deliberately uses candidate-managed installations
because a marketplace-catalog-resolved package cannot exist before publication.
These receipts must come from isolated installs of the exact candidate archive
on Apple Silicon, Windows x64, and Linux x64. Each initializes MCP, completes
one real project-bound `ground`, waits for same-project search readiness, and
verifies the expected Metal or Vulkan engine and package identity. They do not
replace the three marketplace-catalog-resolved post-publish receipts.

For v0.16, `check-packaged-agent-proof.py --server-behavior-only` is the
fail-closed receipt mode for that claim. It deliberately skips multi-host
sharing, broader server lifecycle, accuracy, and performance claims.
`--ground-only` remains a lower-tier launcher and provenance check: it stops
after the project-bound ground request and cannot claim search readiness,
server identity, or accelerator execution.

The command-line evaluator derives repository, commit, and source-tree identity
from `--repo` and the full `--expected-sha`; evidence documents cannot supply
those trusted values. Other required CLI identities and exceptions use
`--expected-identity` and `--expected-exceptions` JSON files from separately
trusted inputs; release-evidence library callers bind them from the approved
candidate profile or graph constraints. Risk-bearing dependencies must be named
as requested claims with their own accepted risks. Current full-product metrics
and user-facing SLOs are non-waivable. Only a separately trusted, exact-artifact
model microbenchmark regression over 5% and at least three repeats may remain
`pass_with_exception`; it must cite passing same-run full-product benefit,
bind the release key, owner, rationale, rollback, and expire within 14 days or
when the next release key is selected. It never becomes an unqualified pass.

Workflow syntax and repository semantics are separate gates. The actionlint
wrapper checks every workflow with `.github/actionlint.yaml` using the declared
v1.7.12 binary or a checksum-verified official archive, and must reject the
controlled-invalid syntax fixture. Its unit tests cover every declared host
platform, archive checksum failure, and cached-binary version/provenance.
Workflow policy then
checks CodeStory-specific exact-SHA, protected-environment, least-privilege,
secret-forwarding, artifact-retention, matrix, and promotion contracts. Job
permission overrides are part of the effective permission set; protected
reusable callers cannot inherit secrets or forward undeclared names. Semantic
controlled-invalid fixtures must retain their class-prefixed diagnostics.

Draft pushes run focused checks and one Linux source check. Exact-head review
runs the broad source gate once. Packaged matrices and protected hardware run
only through the coordinator/platform-proof gate. Draft pushes cancel stale
draft work. Exact source and platform coordinators are reusable
`workflow_call` workflows with a `workflow_dispatch` entry; no pull-request
label starts them. A maintainer dispatches `packaged-platform-pr.yml` against
an exact accepted head, and it calls the coordinators. Their concurrency and
cache identities include the exact Actions SHA, so a later push cannot cancel
or populate proof for an accepted old head. Each target is built once then
reused by its proof steps.

Release signing, notarization, post-publish quarantine/Gatekeeper checks,
installed plugin readback, and live full retrieval run only from the main
release workflow. No version bump, tag, signing, notarization, or release is
part of ordinary remediation or embedding-engine PRs.

Maintainers may manually dispatch `release.yml` from an exact
`dev/codestory-next` SHA to authenticate the graph-declared pre-publish ledger without
publishing. The required `expected_head_sha` must equal both the workflow SHA
and the live dev branch head. Manual dispatch exposes no publication input;
only the automatic reusable-workflow caller on the live `main` head passes the
fail-closed publication authority used by publish and post-publish jobs.

```text
gh workflow run release.yml --ref dev/codestory-next \
  -f version=<version> \
  -f expected_head_sha=<exact-full-dev-sha>
```

There is intentionally no `publish_release` field on this manual command.

## Evidence reporting

State the exact SHA, commands, machine/backend, cache state, and highest proof
tier reached. Distinguish source, package, hardware, plugin, installed-runtime,
and live behavior evidence. Include skipped work and platform evidence still
owed; never upgrade a hosted source or package result into a Metal or Vulkan
claim. A passing
lower-tier row cannot satisfy a higher-tier claim, and one current row cannot
hide stale historical evidence for the same requirement.
