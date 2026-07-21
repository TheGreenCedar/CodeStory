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
| Retrieval/embedding | Retrieval tests, runtime admission tests, engine proof self-test | Same-run quality/performance gate and required hardware proof |
| CLI/stdio | Named CLI contract suites | Workspace source gate and packaged proof when package behavior changed |
| Plugin launcher or CodeStoryDev staging | Installer tests plus `plugin-static` | Packaged plugin handoff |
| Worktree setup | Node suite plus one platform adapter smoke | Mac/Windows platform cell when adapter changed |
| Docs only | Read changed pages, doc links, `git diff --check` | No package matrix |
| Release/version | Release and workflow policy scripts | Main-only signing, notarization, publish, install, and live runtime proof |

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
behavior changed. Intermediate commits do not append telemetry.

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
- explicit `accelerated` or `cpu_explicit` policy;
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

The Linux x64 packaged-platform job additionally runs the named
`Prove clean-cache Node-absent network-denied offline release build` lane. It
seeds a new isolated Cargo home, mounts the source read-only into the pinned
build image, removes Node from the execution contract, denies container network
access, and runs both `cargo check --release --locked --offline` and
`cargo build --release --locked --offline` from a fresh target. The container's
`--network none` boundary is the network-denial proof; Cargo's offline flag
alone is not treated as OS-level denial.

Hosted source/package jobs may set:

```sh
CODESTORY_EMBED_ALLOW_CPU=1
```

They must report `cpu_explicit` and make no acceleration claim.

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

Runtime proof uses the ordinary plugin launcher with two independently started
host processes and different repositories. `--proof-tier calibration` may
collect draft measurements, but cannot satisfy a package, hardware, installed,
or release claim. A higher tier requires a frozen constant set and a retained
qualification record. `--produce-qualification-evidence` runs the private,
nonce-gated scenario orchestrator and writes the path passed to
`--qualification-evidence`; without the producer flag, the harness verifies an
existing record. Missing, stale, partial, self-selected, or wrong-tier evidence
fails.

macOS packages keep the selected backend built in. Windows and Linux packages
ship runtime-loaded native modules beside the executable. Hosted Linux proof
does not install a Vulkan loader before help, stdio initialization, or explicit
CPU execution, and it makes no Linux acceleration claim.

Use `--plugin-handoff`, `--engine-policy`, `--expected-backend`, and `--offline`
to make the claim explicit. Protected and installed tiers additionally name
their exact proof tier and retained qualification file. The harness self-test
uses synthetic fixtures only:

```sh
python .github/scripts/check-packaged-agent-proof.py --self-test
```

### Hardware claims

| Workflow | Required claim |
| --- | --- |
| `.github/workflows/macos-metal-proof.yml` | Exact Apple Silicon package, CPU disallowed, Metal, physical adapter, live smoke, full layer offload, and complete frozen server qualification |
| `.github/workflows/windows-vulkan-proof.yml` | Exact Windows x64 package, CPU disallowed, Vulkan, physical adapter, live smoke, full layer offload, and complete frozen server qualification |
| Linux protected Vulkan workflow | Required before any Linux GPU claim; hosted CPU proof is insufficient |

Signing and notarization are main-release concerns, not PR gates. A PR package
may be unsigned while still proving the named package/runtime tier.

### Performance and quality

Before replacing a model or native embedding implementation, compare incumbent
and candidate in the same release build on the same machine. Keep that
measurement selector private and delete it before merge. A server-ownership
cutover does not relabel pre-fault and post-fault searches as two
implementations: it consumes the existing exact-head
`publishable-three-repeat-packet/v1` artifact and derives the pass rate from
every row and repeat. Freeze every production timing value and qualification
threshold before running the unchanged qualification candidate; a result
cannot define its own pass threshold.

Measure existing-owner connect, listener spawn, first residency, first product
ready, warm query/bulk IPC, bulk documents and tokens per second, useful retry
latency, true-idle exit, total CodeStory process memory, accelerator residency,
retrieval quality, multi-process reuse, and restart reuse separately. Use
awake-time monotonic clocks within each process; never subtract timestamps from
different process origins. Quality cannot regress. A repeatable throughput,
warm-latency, or memory regression blocks the cutover; 5% is measurement noise,
not an accepted sustained loss.

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
node .github/scripts/route-ci-proof.mjs --self-test
```

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
`ready_command` integration target with explicit CPU runtime permission. Its
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

That Windows lane is source and protocol evidence on a hosted CPU runner. The
SDK preserves the production-default native compile topology; it does not prove
Vulkan execution, a packaged archive, an installed runtime, or protected
hardware behavior. Those claims remain with the package and protected Windows
Vulkan proof lanes.

`release-claims.json` is the release claim and proof-tier source of truth. It
binds each claim to its evidence identity, expiry, dependency, executable
prerequisite, non-claims, accepted risks, and higher-tier proof lanes. The
release evidence gate evaluates that graph; workflow policy consumes its
runner, target, promotion, retention, and proof-chain facts. Update the graph
instead of copying those facts into contributor prose.

The same graph declares the exact release-closeout cells. The closeout
coordinator expands native cells from `workflow_policy.package_matrix`, keeps
protected hardware cells explicit, invokes the claim evaluator for each cell
and its graph dependencies, then retains canonical copies under `manifests/`
and `evaluations/` beside `ledger.json` and `summary.json`. A pre-publish run
accepts 12 cells: exact source, six package targets, two protected-hardware
targets, and the three release-evidence claims. A post-publish run accepts 30
cells after adding platform, installed-runtime and downloaded-byte proof for
all six targets. Package rows record each archive name, byte count, and SHA-256.
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

An accepted model-microbenchmark exception is a separate authenticated input,
not a manifest assertion. The release-evidence container retains
`codestory.release-closeout-exceptions/v1`; closeout verifies its producer,
includes same-run `answer_quality` in the performance-cell evaluation, and
passes the trusted exception map to the claim evaluator. Both closeout jobs
retain the provenance map and exception input with canonical manifests,
individual evaluations, ledger and summary.

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
  --trusted-exceptions <selected-release-evidence-container/trusted-exceptions.json> \
  --manifest-dir <unflattened-selected-artifact-directories> \
  --out-dir <new-closeout-directory>
```

For `--phase post_publish`, pass every graph-owned cell plus
`--pre-publish-ledger <accepted-pre-publish-ledger.json>`. The framework can be
tested and merged without final evidence; an accepted ledger still requires
the frozen exact-head producer manifests and does not upgrade source or package
proof into installed, protected-hardware, or live-behavior proof.

Pre-publish authorization deliberately has no candidate-installed cell. A
marketplace install cannot exist until publication, and candidate-managed proof
does not replace it. The six installed-runtime cells remain post-publish and
the real two-session/one-server installed-runtime qualification remains the
separate #1221 evidence tier.

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
draft work. Exact source and platform coordinators run only when their label is
applied; their concurrency and cache identities include the exact Actions SHA,
so a later push cannot cancel or populate proof for an accepted old head. Each
target is built once then reused by its proof steps.

Release signing, notarization, post-publish quarantine/Gatekeeper checks,
installed plugin readback, and live full retrieval run only from the main
release workflow. No version bump, tag, signing, notarization, or release is
part of ordinary remediation or embedding-engine PRs.

## Evidence reporting

State the exact SHA, commands, machine/backend, cache state, and highest proof
tier reached. Distinguish source, package, hardware, plugin, installed-runtime,
and live behavior evidence. Include skipped work and platform evidence still
owed; never upgrade a hosted CPU result into a Metal or Vulkan claim. A passing
lower-tier row cannot satisfy a higher-tier claim, and one current row cannot
hide stale historical evidence for the same requirement.
