# AST-First Retrieval Remediation Implementation Plan

> Implementation route: use `superpowers:subagent-driven-development` or
> `superpowers:executing-plans`. Execute task-by-task, review each task before
> advancing, and keep commits small enough to revert independently.

## Goal

Remove production benchmark overfit, unify language-support claims, expose
unresolved sidecar evidence, clarify `files` count semantics, and record the
verification gates for the AST-first retrieval branch.

## Scope

This plan covers one remediation slice:

1. Product packet overfit removal.
2. Shared language-support registry.
3. Registry consumer wiring and drift tests.
4. Sidecar packet diagnostics.
5. `files` count semantics.
6. Receiver-resolution boundary docs.
7. Final verification and stats logging.

Do not start dynamic parser loading, large module decomposition, or broad
receiver-call architecture work in this slice.

## File Ownership

Create:

- `crates/codestory-contracts/src/language_support.rs`
- `docs/superpowers/plans/2026-06-13-ast-first-retrieval-remediation.md`

Modify as needed:

- `crates/codestory-contracts/src/lib.rs`
- `crates/codestory-contracts/src/api.rs`
- `crates/codestory-contracts/src/api/dto.rs`
- `crates/codestory-indexer/src/lib.rs`
- `crates/codestory-workspace/src/lib.rs`
- `crates/codestory-runtime/src/lib.rs`
- `crates/codestory-runtime/src/semantic_doc_text.rs`
- `crates/codestory-runtime/src/agent/orchestrator.rs`
- `crates/codestory-runtime/src/agent/retrieval_primary.rs`
- `crates/codestory-runtime/src/agent/packet_search.rs`
- `crates/codestory-runtime/src/agent/packet_batch.rs`
- `crates/codestory-runtime/src/agent/packet_trace.rs`
- `crates/codestory-runtime/src/agent/trace.rs`
- `crates/codestory-runtime/src/agent/trace_export.rs`
- `crates/codestory-cli/src/main.rs`
- `crates/codestory-cli/src/output.rs`
- `crates/codestory-cli/tests/cli_golden_path.rs`
- `crates/codestory-cli/tests/onboarding_contracts.rs`
- `scripts/lint-retrieval-generalization.mjs`
- `docs/architecture/language-support.md`
- `docs/review-action-plan.md`
- `docs/specs/review-remediation-ast-first-retrieval/validation.md`
- `docs/testing/codestory-e2e-stats-log.md`

## Task 1: Remove Production Benchmark-Family Steering

Acceptance criteria:

- Production packet retrieval does not branch on review benchmark families.
- `CODESTORY_PACKET_EXACT_FAMILY_STEERING` and exact-family steering helpers are removed from production code.
- Generic SQL schema support remains intact.
- `scripts/lint-retrieval-generalization.mjs` bans the reviewed benchmark-family literals in production retrieval/indexing slices.

Verification:

```powershell
node scripts/lint-retrieval-generalization.mjs
cargo test -p codestory-runtime packet_sufficiency -- --nocapture
rg -n "\b(chinook|mdn|okio|monolog|alamofire)\b|PACKET_EXACT_FAMILY_STEERING|packet_exact_family_steering" crates\codestory-cli\src crates\codestory-indexer\src crates\codestory-runtime\src crates\codestory-retrieval\src
```

Commit target:

```powershell
git commit -m "remove packet benchmark steering"
```

## Task 2: Create Shared Language-Support Registry

Acceptance criteria:

- `codestory-contracts` owns public language support metadata.
- Indexer support-profile functions delegate to the shared registry.
- Parser construction remains in the indexer; the registry does not imply every discovered extension has a parser.
- Tests distinguish first-class parser support from text-evidence/discovery support.

Verification:

```powershell
cargo test -p codestory-contracts language_support -- --nocapture
cargo test -p codestory-indexer test_language_support_profiles_separate_runtime_claims -- --nocapture
```

Commit target:

```powershell
git commit -m "centralize language support registry"
```

## Task 3: Wire Registry Consumers And Drift Checks

Acceptance criteria:

- Semantic document labels use the registry.
- Runtime `files` language labels use the registry.
- Workspace source extension coverage is checked against registry claims where ownership matches.
- Language support docs name the contracts registry as the source of truth.
- Public onboarding docs contract stays green.

Verification:

```powershell
cargo test -p codestory-runtime language_from_path_covers_supported_extensions -- --nocapture
cargo test -p codestory-workspace workspace_supported_source_extensions_have_registry_profiles -- --nocapture
cargo test -p codestory-cli --test onboarding_contracts -- --nocapture
```

Commit target:

```powershell
git commit -m "wire language support registry"
```

## Task 4: Surface Packet Sidecar Diagnostics

Acceptance criteria:

- Packet sidecar queries expose structured diagnostics when candidates exist but cannot be resolved.
- Trace/export/CLI surfaces preserve the diagnostics.
- Unresolved-only sidecar candidate sets count as packet sufficiency gaps.
- Diagnostics count only attempted candidate resolutions, not capped-away candidates.

Verification:

```powershell
cargo test -p codestory-runtime packet_sidecar_query_diagnostic -- --nocapture
cargo test -p codestory-runtime packet_sufficiency_treats_unresolved_sidecar_candidates_as_gap -- --nocapture
cargo check -p codestory-runtime -p codestory-cli
cargo fmt --check
git diff --check
```

Commit target:

```powershell
git commit -m "surface packet sidecar gaps"
```

## Task 5: Clarify `files` Count Semantics

Acceptance criteria:

- API DTOs expose whole-index, filtered, and visible/truncated counts.
- Runtime computes filtered counts before truncation and visible counts after truncation.
- CLI markdown labels cannot be read as filtered counts when they are whole-index counts.
- Golden path tests cover JSON and markdown labels.

Verification:

```powershell
cargo test -p codestory-cli --test cli_golden_path tiny_workspace_browser_loop_works_from_existing_cache -- --nocapture
git diff --check
```

Commit targets:

```powershell
git commit -m "clarify files count semantics"
git commit -m "test files summary truncation label"
```

## Task 6: Document Receiver-Resolution Boundaries

Acceptance criteria:

- Language support docs state that receiver-call support is fixture-backed only.
- Cross-package receiver lookup, polymorphic dispatch, inheritance-heavy selection, framework-handler resolution, and declarative parameter extraction remain explicitly out of scope.
- The old review action plan points to the active remediation spec and execution plan.
- Validation notes no longer claim implementation readiness after implementation has begun.
- Public docs avoid private local paths and blocked onboarding terms.

Verification:

```powershell
cargo test -p codestory-cli --test onboarding_contracts -- --nocapture
git diff --check
```

Commit targets:

```powershell
git commit -m "document retrieval remediation boundaries"
git commit -m "add remediation planning artifacts"
git commit -m "scrub local review evidence paths"
```

## Task 7: Final Verification And E2E Stats

Run these gates serially:

```powershell
cargo fmt --check
cargo check --all-targets
node scripts/lint-retrieval-generalization.mjs
cargo test -p codestory-indexer --test fidelity_regression
cargo test -p codestory-indexer --test tictactoe_language_coverage
cargo test -p codestory-runtime packet_sufficiency -- --nocapture
cargo test -p codestory-cli --test cli_golden_path -- --nocapture
cargo test -p codestory-cli --test onboarding_contracts -- --nocapture
cargo build --release -p codestory-cli
$env:CODESTORY_ALLOW_SKIP_REAL_REPO_DRILL_CASES='1'
cargo test -p codestory-cli --test codestory_repo_e2e_stats -- --ignored --nocapture
git diff --check
```

Append the emitted stats to `docs/testing/codestory-e2e-stats-log.md`.
If `CODESTORY_REAL_REPO_DRILL_CASES` is unavailable, explicitly label the row
as a stats-only run with the real drill intentionally skipped.

Commit target:

```powershell
git commit -m "log remediation e2e stats"
```

## Task 8: Self-Review Before Handoff

Acceptance criteria:

- Requirement coverage is 100% against the remediation spec.
- Production benchmark-family literals are absent from production retrieval/indexing slices.
- `git status --short` shows only intentional changes before the final stats commit, and a clean tree after it.
- Final response lists changed areas, verification, and any remaining risk.

Traceability validator command shape:

```powershell
$validator = $env:SPECIFICATION_ARCHITECT_TRACEABILITY_VALIDATOR
python "$validator" --path docs/specs/review-remediation-ast-first-retrieval --requirements requirements.md --tasks tasks.md --research research.md
```

Production literal check:

```powershell
rg -n "\b(chinook|mdn|okio|monolog|alamofire)\b|PACKET_EXACT_FAMILY_STEERING|packet_exact_family_steering" crates\codestory-cli\src crates\codestory-indexer\src crates\codestory-runtime\src crates\codestory-retrieval\src
```

## Execution Notes

- Cargo build and test commands must stay serialized in this repo.
- Do not claim real drill evidence unless a manifest is provided and the drill test runs without the skip flag.
- Prefer follow-up commits over amending already reviewed task commits.
