# CodeStory Agent Guide

**Audience:** agents and contributors changing CodeStory. This file contains the
decisions that must remain visible while working; generated help, architecture
pages, runbooks, and workflows own detailed mechanics.

## Start Here

- Establish current truth before choosing work: inspect the current branch and
  integration head, active worktrees, open PR or issue ownership, and the
  release state. Do not reuse an active lane or implement routine work directly
  on `dev/codestory-next`.
- Run `node scripts/codex-worktree-setup.mjs` for a delegated worktree. Treat
  its printed base, child head, PR head, and proof target as authoritative
  before cache repair, readiness work, or verification. When changing setup,
  keep the PowerShell and POSIX implementations behaviorally aligned and run
  the setup self-tests from the testing matrix.
- Before source claims, planning edits, choosing tests, or reviewing changes,
  use the canonical CodeStory grounding skill when its MCP tools are visible.
  Every MCP call must carry the target repository's absolute `project` root.
  Call the tool that matches the task directly; tool gating owns readiness and
  managed preparation. Read status only after a call fails to converge. If the
  MCP tools are not visible, use ordinary source inspection and report the
  visibility gap. CLI diagnostics do not prove that the packaged plugin MCP is
  live in the agent host.
- For a large change, read `docs/architecture/overview.md`, the owning
  subsystem page, `docs/contributors/debugging.md`, and
  `docs/contributors/testing-matrix.md` before editing.

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
- `codestory-runtime`: the only product orchestration layer. Indexing,
  grounding, search, packet construction, and agent flows belong here.
- `codestory-cli`: command and transport parsing, output rendering, process
  configuration capture, readiness-broker integration, and managed sidecar
  lifecycle boundaries. Do not move product orchestration into adapters.
- `plugins/codestory`: host hooks, the packaged launcher, MCP routing, and the
  canonical agent skill. Plugin routing selects a project per request and
  reaches product behavior through the version-matched CLI.
- `codestory-bench`: measurement and benchmark support only; it does not own
  product contracts.

Dependency direction is
`contracts -> workspace/store/indexer/retrieval -> runtime -> cli/adapters`.
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
  workspace test and all-target/all-feature clippy gate run once on an
  independently accepted exact head.
- CLI integration tests must launch through
  `tests/test_support::cli_command` or its supplied-binary variant, use
  isolated cache/install/plugin state roots.
  Never clean or write the real user cache to make a test pass, and never
  serialize the suite to hide state leakage.
- Docs-only scope is `README.md`, `docs/**`, `plugins/codestory/README.md`,
  `plugins/codestory/docs/**`, and `plugins/codestory/skills/**`. Read changed
  pages back, then run `git diff --check` and
  `node .github/scripts/check-doc-links.mjs`. Do not add tests that assert prose.
  Plugin adapter changes also run
  `node --test plugins/codestory/tests/plugin-static.test.mjs`.
- Indexer fidelity or language coverage requires the full binaries, not name
  filters:
  - `cargo test -p codestory-indexer --test fidelity_regression`
  - `cargo test -p codestory-indexer --test tictactoe_language_coverage`
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

- `crates/codestory-cli/Cargo.toml` is the release version source. Synchronize
  every `codestory-*` workspace crate, `Cargo.lock`, the
  `producer.version` in `crates/codestory-llama-sys/model-contract.json`, and
  these plugin manifests:
  - `plugins/codestory/.codex-plugin/plugin.json`
  - `plugins/codestory/.claude-plugin/plugin.json`
  - `plugins/codestory/.github/plugin/plugin.json`
- Validate release changes with
  `python .github/scripts/check-codestory-release.py --version <version>` and
  `node .github/scripts/check-workflow-policy.mjs`.
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
- When Codex must observe a plugin-source change, publish the corresponding
  update to `TheGreenCedar/AgentPluginMarketplace`, refresh the marketplace,
  and verify the installed managed runtime path/version plus project-scoped
  status. CodeStory repository state alone does not update the marketplace.

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
- Retrieval design and operations: `docs/architecture/retrieval-design.md`,
  `docs/testing/retrieval-architecture.md`, and
  `docs/ops/retrieval-engine.md`
- Language claim tiers: `docs/architecture/language-support.md`
- Agent operational contract:
  `plugins/codestory/skills/codestory-grounding/SKILL.md` and its references
- Current CLI syntax: `codestory-cli --help` and subcommand help
