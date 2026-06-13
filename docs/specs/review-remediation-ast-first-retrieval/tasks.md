# Implementation Plan

## Phase 1: Remove Product Overfit

- [ ] 1. Remove default-on exact-family packet steering from production
  - [ ] 1.1 Delete the production call block that appends Chinook, MDN, Okio, Monolog, and Alamofire static citations.
  - [ ] 1.2 Delete unused static family citation helpers and exact-family env/default code from `orchestrator.rs`, or move required eval-only helpers behind `EvaluationProbeBoundary`.
  - [ ] 1.3 Update packet sufficiency tests so production behavior passes with steering absent.
  - _Requirements: 1.1, 1.2, 1.4_

- [ ] 2. Extend the retrieval generalization lint
  - [ ] 2.1 Add `chinook`, `mdn`, `okio`, `monolog`, `alamofire`, and the most specific hardcoded path fragments to `scripts/lint-retrieval-generalization.mjs`.
  - [ ] 2.2 Confirm eval-only files and tests remain allowed through explicit boundaries, not broad production exemptions.
  - [ ] 2.3 Run the lint and fix any production hits it reports.
  - _Requirements: 1.3, 6.1_

## Phase 2: Unify Language Support Claims

- [ ] 3. Add the shared language support registry
  - [ ] 3.1 Create `crates/codestory-contracts/src/language_support.rs` with support profile structs, enums, extension lookup, language-name lookup, and path lookup.
  - [ ] 3.2 Export the registry from `codestory-contracts`.
  - [ ] 3.3 Move claim labels and extension ownership out of drift-prone runtime/indexer tables where possible.
  - _Requirements: 2.1_

- [ ] 4. Wire registry consumers
  - [ ] 4.1 Update `codestory-workspace` discovery to consume registry extension metadata where dependency direction allows.
  - [ ] 4.2 Update `codestory-indexer` support profile APIs to delegate to the shared registry while keeping parser construction local.
  - [ ] 4.3 Update `semantic_doc_text.rs` to derive language labels from the registry.
  - [ ] 4.4 Update CLI `files` language summary claim labels to use the same registry path.
  - _Requirements: 2.1, 2.2, 2.3_

- [ ] 5. Add registry drift tests and docs updates
  - [ ] 5.1 Add tests that compare registry-supported extensions against workspace discovery, indexer profiles, semantic doc labels, and CLI/API files summaries.
  - [ ] 5.2 Update `docs/architecture/language-support.md` to name the shared registry as the source of truth.
  - [ ] 5.3 Update or supersede `docs/review-action-plan.md` so completed claims do not hide the newly confirmed gaps.
  - _Requirements: 2.3, 2.4, 5.1, 5.3_

## Phase 3: Make Retrieval Gaps Visible

- [ ] 6. Add packet sidecar diagnostics
  - [ ] 6.1 Add per-query packet sidecar diagnostic data for candidate count, resolved hit count, unresolved candidate count, mode, and optional diagnostic text.
  - [ ] 6.2 Preserve the single-search unresolved-only rejection behavior.
  - [ ] 6.3 Teach packet sufficiency to treat unresolved-only sidecar evidence as a gap.
  - _Requirements: 3.1, 3.2, 3.3_

- [ ] 7. Cover sidecar packet states in tests
  - [ ] 7.1 Update `retrieval_primary.rs` tests for empty full-mode packet subqueries.
  - [ ] 7.2 Add unresolved-only packet subquery tests that assert diagnostic visibility.
  - [ ] 7.3 Add mixed resolved/unresolved packet subquery tests.
  - _Requirements: 3.4, 6.1_

## Phase 4: Fix `files` Count Semantics

- [ ] 8. Add explicit filtered and visible counts
  - [ ] 8.1 Extend `IndexedFilesSummaryDto` with filtered and visible count fields while preserving existing whole-index fields where feasible.
  - [ ] 8.2 Compute filtered count before truncation and visible count after truncation in `AppController::indexed_files`.
  - [ ] 8.3 Update CLI markdown labels to distinguish whole-index totals, filtered totals, visible rows, and truncation.
  - _Requirements: 4.1, 4.2, 4.3_

- [ ] 9. Add `files` filter tests
  - [ ] 9.1 Cover path filters.
  - [ ] 9.2 Cover language filters.
  - [ ] 9.3 Cover role filters.
  - [ ] 9.4 Cover truncation with filtered counts.
  - _Requirements: 4.4, 6.1_

## Phase 5: Make Receiver Resolution Limits Explicit

- [ ] 10. Pin current receiver resolution claims
  - [ ] 10.1 Update language support docs to limit typed receiver-call claims to tested same-file/simple cases unless cross-file fixtures pass.
  - [ ] 10.2 Add or mark follow-up fixtures for cross-file typed receiver calls in representative languages.
  - [ ] 10.3 Document manual string-based parameter extraction as transitional debt.
  - _Requirements: 5.1, 5.2, 5.3_

- [ ] 11. Plan the later cross-file receiver implementation
  - [ ] 11.1 Create a follow-up issue or task note for routing receiver target lookup through global resolution support.
  - [ ] 11.2 Define replacement criteria for declarative AST/query parameter extraction before removing the manual string splitter.
  - _Requirements: 5.4_

## Phase 6: Verification and Closeout

- [ ] 12. Run the required remediation gate
  - [ ] 12.1 Run `cargo fmt --check`.
  - [ ] 12.2 Run `cargo check --all-targets`.
  - [ ] 12.3 Run `node scripts/lint-retrieval-generalization.mjs`.
  - [ ] 12.4 Run touched-surface runtime, indexer, CLI, and Node tests.
  - _Requirements: 6.1_

- [ ] 13. Run language and repo-scale gates before commit/merge
  - [ ] 13.1 Run `cargo test -p codestory-indexer --test fidelity_regression`.
  - [ ] 13.2 Run `cargo test -p codestory-indexer --test tictactoe_language_coverage`.
  - [ ] 13.3 Run `cargo build --release -p codestory-cli`.
  - [ ] 13.4 Run `cargo test -p codestory-cli --test codestory_repo_e2e_stats -- --ignored --nocapture`.
  - [ ] 13.5 Append the fresh stats row to `docs/testing/codestory-e2e-stats-log.md`.
  - _Requirements: 6.2, 6.3_

- [ ] 14. Report final evidence
  - [ ] 14.1 Summarize what changed by component.
  - [ ] 14.2 List exact commands run and outcomes.
  - [ ] 14.3 List any unverified risks or explicitly deferred architecture work.
  - _Requirements: 6.4_
