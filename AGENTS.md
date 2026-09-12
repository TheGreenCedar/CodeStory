# CodeStory Agent Guide

**Audience:** agents and contributors changing CodeStory. This file contains the
decisions that must remain visible while working; generated help, architecture
pages, runbooks, and workflows own detailed mechanics.

## Start Here

- Before mutating the tree, creating a lane, or delegating work, inspect the
  current branch, integration head, active worktrees, and open PR or issue
  ownership. Inspect release state only for release work. Do not reuse an
  active lane or implement routine work directly on `dev/codestory-next`.
- Run `node scripts/codex-worktree-setup.mjs` for a delegated worktree. Treat
  its printed base, child head, PR head, and proof target as authoritative
  before cache repair, readiness work, or verification. When changing setup,
  keep the PowerShell and POSIX implementations behaviorally aligned and run
  the setup self-tests from the testing matrix.
- Use the canonical CodeStory grounding skill when its MCP tools are visible
  and the task is repository discovery, relationship tracing, change-impact
  analysis, or CodeStory retrieval / installed-plugin validation. Skip that
  loop only for bounded inspection or editing of named files that does not
  require those operations; naming a file does not by itself forbid CodeStory.
  Every
  MCP call must carry the target repository's absolute `project` root. Call
  the tool that matches the task directly; tool gating owns readiness and
  managed preparation. Read status only after a call fails to converge. If the
  MCP tools are not visible, use ordinary source inspection and report the
  visibility gap. CLI diagnostics do not prove that the packaged plugin MCP is
  live in the agent host. Qualify CodeStory results as retrieval or
  installed-plugin evidence, not as live runtime behavior, unless the matching
  proof tier exists.
- When a linked document path is already known, bound `rg` or search to that
  file. Do not run a broad package or repository search merely to rediscover a
  documented path.
- Route reading by the task: the owning architecture page (from
  `docs/architecture/overview.md` and the subsystem page) when contracts
  change, `docs/contributors/debugging.md` for failures, and the relevant lane
  in `docs/contributors/testing-matrix.md` when selecting proof. Do not
  pre-read all four contributor docs for every large change.

## Ownership Boundaries

- `codestory-contracts`: shared DTOs, graph types, events, grounding and trail
  contracts.
- `codestory-workspace`: project discovery, inventories, refresh planning, and
  repository/project identity.
- `codestory-indexer`: parsing, extraction, intermediate projections, and
  semantic resolution.
- `codestory-store`: SQLite source of truth, snapshots, projections, and core
  publication.
- `codestory-retrieval`: lexical, semantic, and SCIP artifacts; immutable
  sidecar generations; manifests; health; and fail-closed query execution.
- `codestory-agent`: syntactic retrieval-seed planning and pure
  repository-derived packet compilation over typed admitted evidence. Only the
  seed plan may retain the unchanged question; the compiler cannot receive
  prompt terms, task classes, obligations, roles, carriers, or sufficiency
  policy. It depends on `codestory-contracts` alone and can never activate,
  store, execute retrieval, admit or hydrate candidates, retry a publication,
  or move readiness.
- `codestory-runtime`: the only product orchestration layer. Indexing,
  grounding, search, packet assembly, and retrieval execution belong here.
  Retrieval, admission, hydration, retry, and packet assembly belong here;
  pure seed planning and evidence selection belong in `codestory-agent`.
- `codestory-cli`: command and transport parsing, output rendering, process
  configuration capture, and managed sidecar lifecycle boundaries. Do not move
  product orchestration into adapters.
- `plugins/codestory`: host hooks, the packaged launcher, MCP routing, and the
  canonical agent skill. Plugin routing selects a project per request and
  reaches product behavior through the version-matched CLI.
- `codestory-bench`: measurement and benchmark support only; it does not own
  product contracts.

Dependency direction is
`contracts -> workspace/store/indexer/llama-sys/retrieval/agent -> runtime -> cli/adapters`.
Change the owning source-of-truth layer first; do not patch a derived view or
adapter to compensate for incorrect upstream state.

## Product Invariants

### Identity and configuration

- Keep logical project, workspace, artifact scope, publication generation,
  task/request, run, lease, and process identity distinct. Never replace these
  contracts with a path spelling, PID, mutable environment variable, or global
  active-project value.
- Every MCP or plugin request selects its project explicitly. Hook-written
  active-state files are diagnostic only and must not route a runtime.
- Compare existing paths and executables by native filesystem identity. Use
  platform lexical rules only for missing paths; Unix path equality remains
  case-sensitive and Windows path equality case-insensitive.
- Capture user home, network trust, cache root, and runtime defaults once at
  process start. Retain immutable configuration per project. Switching projects
  must not mutate or re-read the process environment, and project config must
  not silently choose cache roots, credentials, or network-egress endpoints.

### Reads, activation, and publication

- Status, doctor, and other read surfaces are observational. They must not
  download assets, refresh indexes, start repair, or mutate sidecar state.
  Project-scoped product tool calls own activation and automatic managed
  preparation.
- Writers stage and validate a complete generation before publishing it.
  Current and rollback pointers change atomically; readers pin one complete
  old-or-new generation. Failure, cancellation, or concurrent source drift
  leaves the previous publication usable and schedules or reports a retry.
- Freshness depends on verified source/content and publication identity, not
  timestamps alone. Partial, unreadable, or bounded discovery cannot prove
  absence and must never schedule deletion.
- Cleanup may remove only resources proven CodeStory-owned by a current token,
  lease, manifest, generation, or proof marker. Do not perform broad Docker,
  process, port, or user-cache cleanup.
- Agent-facing packet/search/context must fail closed on stale or partial
  publications, ambiguous migration, changed runtime identity, non-`full`
  sidecars, dead required infrastructure, or missing required accelerator/embed
  proof. `retrieval_mode=full` proves infrastructure eligibility, not answer
  quality or claim sufficiency.

### Evaluation and surface boundaries

- Production packet/search behavior must not contain holdout repository names,
  fixture paths, expected-answer shapes, or benchmark-family steering.
- Benchmark-shaped probe catalogs and claim/source-truth scoring stay behind
  test-only evaluation boundaries.
- Language claims must name their tier: parser-backed graph coverage,
  structural source collectors, or agent-facing packet quality.
- `packet` owns broad retrieval. `drill` adapts that packet path and must not
  create a second search, readiness, bridge, or scoring system.
- Browser and HTTP adapters remain read-only and loopback-bound by default.
  Any broader browser, UI, or network surface must satisfy the browser surface
  gate in the testing documentation.
- Generated CLI help is the option source of truth. Keep user docs
  workflow-oriented instead of duplicating complete flag matrices.

## Verification By Change

- Choose the smallest credible lane from
  `docs/contributors/testing-matrix.md` before running broad checks. Run
  separate Cargo build, check, test, and clippy commands serially because this
  workspace shares build locks. Use locked dependency resolution in proof
  lanes.
- Do not use `cargo test --workspace --all-targets` as the routine broad gate;
  it expands Criterion targets. Draft work uses focused checks. The full
  workspace test and all-target/all-feature clippy gate run once on the source
  head accepted by the executable release freeze barrier.
- CLI integration tests must launch through
  `tests/test_support::cli_command` or its supplied-binary variant, use
  isolated cache/install/plugin state roots.
  Never clean or write the real user cache to make a test pass, and never
  serialize the suite to hide state leakage.
- Docs-only scope, including the exception that
  `docs/contributors/testing-matrix.md` is not in this lane and remains a
  crate-durability path pin, is owned by
  `docs/contributors/testing-matrix.md#docs-only-fast-path`. That stronger
  existing gate applies; this file does not redefine or reduce it. Read
  changed pages back, then run `git diff --check` and
  `node .github/scripts/check-doc-links.mjs`. Do not add tests that assert prose.
  Plugin adapter changes also run
  `node --test plugins/codestory/tests/plugin-static.test.mjs`.
- Indexer fidelity or language coverage requires the full binaries, not name
  filters:
  - `cargo test --locked -p codestory-indexer --test fidelity_regression`
  - `cargo test --locked -p codestory-indexer --test tictactoe_language_coverage`
- Publication, identity, packaging, or platform changes require their named
  concurrency, fault, package, and native proof lanes from the testing matrix.
  Draft CI, exact-head source proof, platform proof, and integration proof are
  distinct stages. A persistent label never authorizes an unreviewed later SHA.
- Run the repo-scale CLI stats lane once on the promoted final merge-ready head
  only when default indexing, symbol/dense persistence, embedding reuse, or
  cold-start behavior changed. Intermediate commits do not append telemetry.
  `docs/testing/codestory-e2e-stats-log.md` is telemetry only and cannot
  authorize a release; release-significant decisions use the approved,
  attested profile and `scripts/codestory-release-evidence-gate.mjs`.

## Git, PR, and Evidence Workflow

- Routine implementation branches start from and target
  `dev/codestory-next`. Agent branches use `codex/` by default. Comparison-only
  reviewer branches may use `review/codestory-saga-*`.
- Every PR into `main` must come from the same repository's
  `dev/codestory-next` branch. Release, promotion, hotfix, and review work does
  not bypass this source-branch guard.
- Guarded PRs (`codex/*`, `review/codestory-saga-*`, `[codex]` titles, or the
  saga label) must close a PR-sized issue with `Closes`, `Fixes`, or `Resolves`.
  Use `Refs` for broader parents. A partial slice closes only its child issue;
  keep the parent open until its acceptance criteria are met.
- Before creating an issue, branch, worktree, or PR, search open and closed
  issues, merged PRs, and integration history for the requested outcome, then
  prove that outcome is absent from the current integration head.
- For PRs targeting `dev/codestory-next`, add both the issue and PR to the
  Project; computed linked-PR fields may not populate before default-branch
  promotion.
- Keep active ownership visible through the issue/PR lane: worktree, branch,
  base SHA, current head, role, checks, blockers, and next proof target. Keep
  PRs draft through implementation, independent review, exact-head re-review,
  and required CI. Ready means mergeable now.
- PRs should explain context, what changed, how to review, verification, risk,
  and follow-up. Include exact SHAs and distinguish completed proof from
  non-claims.
- Release handoffs must name the final intended source head, known future
  source changes, proof-triggering labels or actions, reusable and invalidated
  evidence, currently running workflows, and the next permitted mutation.
- Public GitHub status comments must use
  `node scripts/github-status-comment.mjs --issue <n> --body-file <file>` or
  stdin; the helper rejects literal `\\n` text.
- Keep generated comparisons, ledgers, CSVs, SVGs, and pre-release evidence in
  PR bodies, issue comments, Project updates, CI artifacts, or external
  storage unless they are intended as durable product documentation.
- Update `CHANGELOG.md` only for user- or operator-visible changes. Write for
  release-note readers: lead with what changed for them and omit implementation,
  refactor, CI, proof, issue, and PR mechanics. Contributor-only changes and
  internal release automation belong in their owning PR or issue unless they
  materially change what users or operators do or can rely on. Keep current
  release-note work under `Unreleased` until release preparation.
- Commit messages are short, lowercase, and imperative.

## Release Rules

Operator sequence, freeze states, invalidation commands, and bump surface lists
live in `docs/contributors/release-runbook.md`. Named proof lanes live in
`docs/contributors/testing-matrix.md`. This section owns policy. If prose and
machine policy disagree, stop and reconcile their owners before continuing.

### Authority, freeze, and proof budget

- Before any gate expected to exceed five minutes, record the exact commit and
  tree, confirm the worktree is clean and pushed, and confirm that every
  planned source or workflow change is already merged. Independent acceptance
  must execute the required hostile mutations on that exact head; diff review
  and existing green tests do not qualify. Any later commit revokes
  acceptance.
- Support PRs use focused checks only. Do not add a proof-triggering label or
  dispatch a broad source, package, calibration, or hardware gate until all
  support PRs are integrated into the release lane. Broad proof belongs to the
  final integration head, not every independently mergeable PR.
- Release order is: merge all blockers, focused checks, actual-host
  microprobes, source stabilization on the pushed final source head, calibrate,
  sole generated constant-set change, freeze that generated head, then
  qualify. If another source or workflow change becomes necessary, immediately
  invalidate the candidate and cancel every queued or running proof for it.
- Run the full workspace source proof exactly once per release candidate, as
  part of source stabilization before calibration. Calibration cannot start
  without that exact-head receipt. The generated constant-only head receives
  lineage validation, calibration-specific checks, hostile mutations, and
  native probes, but never repeats the workspace source proof. Any nonconstant
  change invalidates stabilization and returns the release to the source phase.
- Cancel a run whose head is no longer the intended release candidate. Never
  let an expensive obsolete run finish for information. Before dispatching,
  inspect both in-flight runs and whether any known source change will
  invalidate the result.
- After a platform-specific packaging or filesystem failure, do not run a full
  rebuild until a sub-90-second native probe reproduces the relevant path,
  link, staging, cache, or identity behavior on that operating system. Test the
  selector against the probe or captured artifact first.
- Use one implementer and one adversarial verifier. Give the verifier the exact
  mutation matrix and only the context needed to execute it. Its output is
  limited to counterexamples or acceptance evidence. After two failed
  revisions of the same shape, stop patching examples and redesign the seam.
- Freeze the selected release claim before qualification. The standard claim
  installs the candidate's exact archives on Apple Silicon macOS, Windows x64,
  and Linux x64; completes one real project-scoped `ground` on each with Metal
  or Vulkan as applicable; verifies archive checksums and bundled runtime
  identity; then promotes and publishes through the canonical workflow. It
  does not include answer-accuracy or performance thresholds, benchmark
  baselines, or same-attempt optional evaluation alignment. Do not start,
  repair, or rerun those lanes unless the selected claim or changed owning
  code requires them. A failure in optional evaluation machinery is not a
  release blocker.
- Reuse passing evidence for the same candidate. After two equivalent failures,
  do not make a third attempt without new evidence and a changed approach. When
  the selected claim is proved or the user says the evidence is sufficient,
  preserve the artifacts and stop. Signing, notarization, checksums, and
  publication remain owned by the canonical release workflow.

### Version, lineage, and CPU

- `crates/codestory-cli/Cargo.toml` is the release version source. Apply bumps
  only through `node scripts/bump-version.mjs`; `--check` reports drift. Do not
  edit version surfaces by hand. `producer.embedding_revision` is not a
  release surface: it keys persisted vectors, so bumping it discards every
  user's dense sidecars. Move it only when the embeddings themselves change
  (model, llama.cpp commit, pooling, normalization, dimension, prefixes, or
  vector schema). Surface list, plugin-only lane, and checksum sourcing are in
  the release runbook.
- Release ordering is **bump-then-calibrate**. Bump the version first,
  calibrate the per-user embedding server on the bumped tree, land the
  constant-set freeze commit, then package and release. The frozen-candidate
  `qualification` dispatch authenticates the calibration bundle and runs
  `Prove frozen calibration source lineage`. Every proof-only or publishing
  release preflight separately runs
  `.github/scripts/check-calibration-release-lineage.py` against its actual
  checked-out head, even when no bundle is supplied. Both bindings require the
  calibration commit to be an ancestor of the release commit and require
  `crates/codestory-llama-sys/per-user-embedding-server-constant-set.json` to be
  the only file that differs between them. A calibrate-then-bump ordering
  fails the guard by name; the fix is to move the bump ahead of calibration and
  recalibrate on the bumped tree, never to widen the allowed path set. Any other
  commit -- a doc fix, a CI tweak, a rebase -- between calibration and the
  package also fails, so recalibrate rather than reorder history.
- CPU embeddings are unsupported. Calibration and release-proof execution must
  use `accelerated` policy with CPU fallback disabled. Runtime-constant
  calibration requires exactly three fresh protected Apple Silicon Metal runs
  with one sample per metric per run. Optional Linux Vulkan calibration is a
  standalone, non-selecting diagnostic; it never joins or blocks calibration
  assembly. Calibration freezes runtime constants only. Lifecycle, fault,
  true-idle, memory, retrieval-quality, and accelerator qualification run later
  against the frozen candidate.
- Validate release changes with
  `python .github/scripts/check-codestory-release.py --version <version>` and
  `node .github/scripts/check-workflow-policy.mjs`.

### Promotion, tags, evidence tiers, and catalog truth

- Never create or push `v*` tags manually. A synchronized version bump on
  `main` triggers the release workflow that creates the tag, GitHub release,
  native archives, and `SHA256SUMS.txt`.
- Source checks, built binaries, packaged archives, installed plugin launchers,
  fresh host sessions, and live full-retrieval behavior are distinct proof tiers.
  A lower tier cannot support a higher-tier claim. Use the named testing-matrix
  lane for packet/search readiness, accelerator execution, signing/notarization
  and Gatekeeper, restart survival, host visibility, or another architecture.
- After promoting `dev/codestory-next` into `main`, verify the dev branch still
  exists and matches `main` with:
  - `git ls-remote --heads origin main dev/codestory-next`
  - `git rev-list --left-right --count origin/main...origin/dev/codestory-next`
  Restore a deleted dev branch from the promoted `main` commit before declaring
  the release complete.
- Release closeout includes the required source, package, native, protected
  hardware, post-publish, installed-runtime, and live behavior evidence for the
  claims being shipped. A merge, tag, or downloadable archive alone is not
  release completion.
- Catalog publication is delivery, not a release gate
  (`workflow_policy.catalog_delivery.release_gate: false`). Do not hand-edit
  the catalog. A release may say the catalog was updated only when the push
  actually landed; otherwise the honest outcome is "released, catalog sync
  deferred". `published` and `deferred` are distinct end to end: each has its
  own installer identity, its own `marketplace.repository` in the install
  attestation (`local:candidate-pinned-marketplace-fixture` for a fixture),
  and its own accepted shape in `marketplace_installation.py`. The live shape
  must never be relaxed to admit a local source, a marketplace root outside
  the Codex home, and no pinned `ref`. A deferred install must resolve a
  catalog carrying the `.codestory-marketplace-fixture.json` marker naming the
  exact released commit. The three names live in
  `.github/scripts/marketplace-delivery-identity.mjs`; add a state there, in
  the Python predicate, and in `release-claims.json` together or not at all.
  Closeout reads the mark: every named post-publish cell must agree on one
  declared installer identity; an undeclared installer or disagreement
  rejects the closeout. Recovery procedure is in the release runbook.
- For a local plugin-source change a host must observe outside a release,
  refresh the installed package and verify the managed runtime path/version
  plus project-scoped status. CodeStory repository state alone does not
  update an already-installed host.

## Platform and Security Notes

- On Windows, invoke the Codex npm shim as `codex.cmd`; the extensionless shim
  can fail with `os error 193`.
- On many Windows development hosts, source builds use Visual Studio 18
  Community's bundled CMake and Ninja. Prepend its
  `Common7\IDE\CommonExtensions\Microsoft\CMake\CMake\bin` and
  `Common7\IDE\CommonExtensions\Microsoft\CMake\Ninja` directories to `PATH`,
  set `CMAKE_GENERATOR=Ninja`, and set `CARGO_TARGET_DIR` to a short path such
  as `C:\tmp\codestory-target`; the repository-local target path can exceed the
  nested Vulkan build's CMake path limit.
- Keep secrets out of the repository. Pass credentials through approved
  environment or protected CI secret surfaces, and keep private material out
  of logs, fixtures, generated artifacts, and comments.

## Canonical References

- Architecture and ownership: `docs/architecture/overview.md` and
  `docs/architecture/subsystems/`
- Contributor setup and debugging: `docs/contributors/getting-started.md` and
  `docs/contributors/debugging.md`
- Verification, CI maturity, release proof, and evidence tiers:
  `docs/contributors/testing-matrix.md`
- Release operator sequence and bump surfaces:
  `docs/contributors/release-runbook.md`
- Retrieval design and operations: `docs/architecture/retrieval-design.md`,
  `docs/testing/retrieval-architecture.md`, and
  `docs/ops/retrieval-engine.md`
- Language claim tiers: `docs/architecture/language-support.md`
- Agent operational contract:
  `plugins/codestory/skills/codestory-grounding/SKILL.md` and its references
- Current CLI syntax: `codestory-cli --help` and subcommand help
