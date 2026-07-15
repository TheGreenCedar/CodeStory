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
| Plugin launcher | `node --test plugins/codestory/tests/plugin-static.test.mjs` | Packaged plugin handoff |
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

## Retrieval engine

The supported product path is one executable with a process-wide in-process
CodeRankEmbed Q8 engine. It performs no model or backend download and starts no
helper process. `retrieval_mode=full` still gates agent packet/search.

Focused proof covers:

- embedded-model digest and atomic materialization;
- linked ggml build identity;
- explicit `accelerated` or `cpu_explicit` policy;
- prohibited silent CPU fallback and software-adapter rejection;
- live embedding smoke and layer-offload evidence;
- one engine/model load shared across repositories;
- generation-coherent query reads and producer migration;
- cleanup confined to proved owned generations.

Hosted source/package jobs may set:

```sh
CODESTORY_EMBED_ALLOW_CPU=1
```

They must report `cpu_explicit` and make no acceleration claim.

### Packaged proof

`.github/scripts/check-packaged-agent-proof.py` verifies a checksum-pinned
archive in an isolated offline environment. It requires one native executable,
version/help, full retrieval, plugin packet/search, multi-repository engine
reuse, restart/materialization reuse, and no helper-process lifecycle state.

Use `--plugin-handoff`, `--engine-policy`, `--expected-backend`, and `--offline`
to make the claim explicit. The harness self-test is:

```sh
python .github/scripts/check-packaged-agent-proof.py --self-test
```

### Hardware claims

| Workflow | Required claim |
| --- | --- |
| `.github/workflows/macos-metal-proof.yml` | Packaged Apple Silicon binary, CPU disallowed, Metal, physical adapter, live smoke, full layer offload |
| `.github/workflows/windows-vulkan-proof.yml` | Packaged Windows binary, CPU disallowed, Vulkan, physical adapter, live smoke, full layer offload |
| Linux protected Vulkan workflow | Required before any Linux GPU claim; hosted CPU proof is insufficient |

Signing and notarization are main-release concerns, not PR gates. A PR package
may be unsigned while still proving in-process behavior.

### Performance and quality

Before removing an incumbent embedding implementation, compare incumbent and
candidate in the same release build on the same machine. Keep the selector
private and delete it with the incumbent before merge.

Measure cold initialization, warm query latency, bulk documents/sec, process
RSS, GPU memory, vector parity, retrieval quality, multi-repository reuse, and
restart reuse separately. Quality cannot regress. A repeatable throughput,
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
- source drift at the publication fence;
- core and semantic generation changes during a query;
- handle-relative cleanup during an ancestor swap.

Evidence must show that failure leaves the previous complete publication usable
and never deletes an outside sentinel.

## CLI and plugin

CLI args/rendering use named contract suites before the broad gate. Stdio tests
must send an absolute `project` on every request and prove multi-project routing
does not depend on active-state files.

Plugin adapter changes run:

```sh
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
node .github/scripts/check-workflow-policy.mjs
node --test .github/scripts/check-workflow-policy.test.mjs
node .github/scripts/route-ci-proof.mjs --self-test
```

Draft pushes run focused checks and one Linux source check. Exact-head review
runs the broad source gate once. Packaged matrices and protected hardware run
only through the coordinator/platform-proof gate. New pushes cancel stale runs,
and each target is built once then reused by its proof steps.

Release signing, notarization, post-publish quarantine/Gatekeeper checks,
installed plugin readback, and live full retrieval run only from the main
release workflow. No version bump, tag, signing, notarization, or release is
part of ordinary remediation or embedding-engine PRs.

## Evidence reporting

State the exact SHA, commands, machine/backend, cache state, and highest proof
tier reached. Distinguish source, package, hardware, plugin, installed-runtime,
and live behavior evidence. Include skipped work and platform evidence still
owed; never upgrade a hosted CPU result into a Metal or Vulkan claim.
