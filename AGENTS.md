# Repository Guidelines

**Audience:** Contributors — workspace layout, verification lanes, and merge bar.

## Project Structure & Module Organization
- Rust workspace is defined in `Cargo.toml`; crates live under `crates/`.
- Primary runtime surface is `crates/codestory-cli`; the canonical agent skill ships with the plugin under `plugins/codestory/skills/codestory-grounding`.
- Workspace crates: `codestory-contracts`, `codestory-workspace`, `codestory-store`, `codestory-indexer`, `codestory-retrieval`, `codestory-runtime`, `codestory-cli`, `codestory-bench`.
- Runtime artifacts: user-cache SQLite grounding indexes keyed by repo path; build outputs in `target/`.

## Architecture Overview
- `codestory-runtime` is the headless orchestrator used by the CLI and skill scripts.
- `codestory-contracts` holds the shared graph model, DTOs, grounding/trail types, and shared events.
- Indexing pipeline: `codestory-workspace` discovers files and refresh plans, `codestory-indexer` extracts symbols/edges via tree-sitter + semantic resolution, and `codestory-store` persists graph/search/snapshot state to SQLite.
- `codestory-bench` contains Criterion benches for indexing, grounding, resolution, and cleanup work.

## Build, Test, and Development Commands
- Backend build/test: `cargo build`, `cargo test`, `cargo check`, `cargo fmt`, `cargo clippy`.
- CLI runtime: `cargo run --release -p codestory-cli -- index --project .`.
- Skill-first grounding: `cargo run --release -p codestory-cli -- ground --project .`.
- Codex worktree setup runs `scripts/codex-worktree-setup.ps1` before the thread
  starts: it resolves a ready CLI from `CODESTORY_CLI`, PATH, this worktree, or a
  sibling worktree before building with `sccache`; then it tries `cache
  rehydrate`, refreshes the SQLite index, and best-effort refreshes retrieval
  sidecars/status.
- Release version checks: `python .github/scripts/check-codestory-release.py --version <version>` and `node .github/scripts/check-workflow-policy.mjs`.
- On Windows, the Codex npm shim should be invoked as `codex.cmd` (typically under `%APPDATA%\\npm`); using the extensionless `codex` shim can fail with `os error 193`.
- In this PowerShell environment, large parallel file reads can truncate output; when investigating a single large file, prefer one direct read command (for example `Get-Content` or `cmd /c type`) before parallelizing.
- Public GitHub status comments should go through `node scripts/github-status-comment.mjs --issue <n> --body-file <file>` or stdin; the helper rejects literal `\\n` text before posting.

## Coding Style & Naming Conventions
- Rust edition is `2024` across workspace crates.
- Rust naming: `snake_case` for functions/modules, `PascalCase` for types, `SCREAMING_SNAKE_CASE` for constants.

## Testing Guidelines
- Tests live in `#[cfg(test)]` blocks or `*_tests.rs`; name them `test_*`.
- Before committing, run the repo-scale CLI e2e stats test and append the emitted stats to `docs/testing/codestory-e2e-stats-log.md`:
  - `cargo build --release -p codestory-cli`
  - `cargo test -p codestory-cli --test codestory_repo_e2e_stats -- --ignored --nocapture`
- To exercise indexing fidelity and coverage, you must explicitly run full test binaries, not just filters:
  - `cargo test -p codestory-indexer --test fidelity_regression`
  - `cargo test -p codestory-indexer --test tictactoe_language_coverage`
  - Note: Using `cargo test -p codestory-index fidelity_regression` will just filter tests instead of running the targeted suites.
- Cargo verifications (build, check, test) should be serialized when working in this repo because parallel `cargo` commands will contend on the shared package and build locks.
- For graph perf and fidelity checks, use Criterion benches in `crates/codestory-bench`.
- Operator documentation lives in `docs/users/`. Docs-only proof: `git diff --check` and `node .github/scripts/check-doc-links.mjs`. Do not add unit tests that assert documentation copy or required phrases.

## Commit & Pull Request Guidelines
- Commit messages are short, lowercase, imperative (e.g., `fix minimap`, `refactor graph style`).
- Before staging or committing, update `CHANGELOG.md` whenever the diff changes
  shipped behavior, operator guidance, release automation, packaging, or version
  metadata. If the work modifies the current latest unreleased version or
  creates the next latest version heading, keep the entry under `Unreleased` or
  that new release heading in the same commit.
- PRs should include a summary, tests run, linked issues, and relevant artifacts for behavior changes.
- Routine implementation PRs branch from and target `dev/codestory-next`. Do not target `main` directly unless the lane is an explicit release, hotfix, final comparison/review artifact, or promotion.
- Agent branches should use the `codex/` prefix by default. Saga comparison/review branches may use `review/codestory-saga-*` when the branch exists only to support a reviewer comparison.
- The saga issue-link guard applies to `codex/*` heads, `review/codestory-saga-*` heads, `[codex]` PR titles, and PRs labeled `saga:codestory-intelligence`. Guarded PRs must include a closing issue reference (`Closes #123`, `Fixes #123`, `Resolves #123`, or the full GitHub issue URL) for the PR-sized issue. Use `Refs #...` only for broader parents or related context. For PRs targeting `dev/codestory-next`, add both the issue and PR to the Project because GitHub's computed Linked pull requests field may not populate until default-branch promotion.
- If a slice is partial under a larger saga, create or use a PR-sized child issue and close that issue in the PR body; do not close the parent until its acceptance criteria are actually met.
- Keep release comparisons, ledgers, and generated pre-release evidence in PR bodies, issue comments, project updates, or external artifacts. Do not commit generated comparison docs/CSVs/SVGs into the repo unless they are intended to become durable product documentation.

## Release Guidelines
- `crates/codestory-cli/Cargo.toml` is the release version source.
- Update every `codestory-*` workspace crate version and `Cargo.lock` together.
- Before committing a change that modifies the current unreleased version or
  creates the next latest version, update `CHANGELOG.md` under `Unreleased` or
  the new release heading so release contents are tracked while the work is
  still reviewable.
- Do not create or push `v*` release tags manually. A synchronized version bump on `main` triggers GitHub Actions to create the tag, GitHub release, cross-platform `codestory-cli` binary assets, and `SHA256SUMS.txt`.
- After merging a `dev/codestory-next` promotion PR into `main`, verify
  `dev/codestory-next` still exists and matches `main` with
  `git ls-remote --heads origin main dev/codestory-next` and
  `git rev-list --left-right --count origin/main...origin/dev/codestory-next`.
  If GitHub deletes the head branch, restore it from the promoted `main` commit
  before treating the release as complete.
- After a plugin release or plugin-source update that Codex must detect through
  the marketplace, push a corresponding change to
  `TheGreenCedar/AgentPluginMarketplace`; Codex observes the marketplace repo
  state, not only the CodeStory repo release.
- CI binary assets prove build/package smoke only. Packet/search readiness still requires the sidecar evidence tiers in `docs/contributors/testing-matrix.md`.

## Retrieval documentation
- Canonical sidecar retrieval docs are `docs/architecture/retrieval-design.md`, `docs/testing/retrieval-architecture.md`, and `docs/ops/retrieval-sidecars.md`. Parser compatibility records live in `docs/architecture/language-support.md`.

## Coding & Design Constraints

Current merge bar for production changes:

1. No holdout literals in production paths — packet/search code must not depend on benchmark holdout repo names, fixture paths, or expected-answer shapes.
2. Eval probes stay test-only — benchmark-shaped probe catalogs remain behind the test-only eval-probe boundary.
3. Language support claims match claim tier definitions — distinguish parser-backed graph coverage, structural collectors, and agent-facing packet quality.
4. Benchmark assertions reference living stats, not hard-coded baselines — repo-scale timing belongs in `docs/testing/codestory-e2e-stats-log.md`.
5. Retrieval mode changes require sidecar evidence — agent packet/search readiness must report full sidecar retrieval, not semantic-only fallback.

## Security & Configuration Tips
- Keep secrets out of the repo; pass credentials via environment variables.
