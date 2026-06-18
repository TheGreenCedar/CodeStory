# Implementation Plan

## Phase 1: Fix Promotion Telemetry First

- [x] 1. Repair packet-runtime cache provenance
  - [x] 1.1 Pass `cache_prepared` and `cache_preparation` into `runColdPacketRuntime`.
  - [x] 1.2 Pass `cache_prepared` and `cache_preparation` into `runWarmPacketRuntimeGroup`.
  - [x] 1.3 Add a script self-test where cache preparation is an array but packet-runtime rows still report `prepared-sidecar-cache-read-only`.
  - [x] 1.4 Split publishable blockers into product and harness categories.
  - _Requirements: 5.1, 5.2, 5.3_

## Phase 2: Make Sufficiency Consume Flow Requirements

- [x] 2. Replace duplicate sufficiency flow-role logic
  - [x] 2.1 Extend `FlowRequirement` with a stable `role_id` and human label if needed.
  - [x] 2.2 Make `packet_sufficiency.rs` consume `packet_flow_requirements_for_terms` directly.
  - [x] 2.3 Delete the local `PacketFlowRole` triad once equivalent tests pass.
  - [x] 2.4 Treat missing probe queries as follow-up hints unless their required role is still missing.
  - _Requirements: 1.1, 1.3, 6.3_

- [x] 3. Demote generic navigation claims
  - [x] 3.1 Mark `source evidence` and adjacent-ownership claims ineligible unless they carry a required coverage role.
  - [x] 3.2 Populate `coverage_report.ineligible` with claim id or text, role, tier, and reason.
  - [x] 3.3 Add regressions for the HTML false-sufficient row and SQL false-partial row.
  - _Requirements: 1.2, 2.1, 2.2, 3.2_

## Phase 3: Add Structural Language Policy

- [x] 4. Add structural proof rules
  - [x] 4.1 Add a narrow `StructuralLanguagePolicy` helper.
  - [x] 4.2 Allow SQL source-scan table and foreign-key roles to satisfy schema coverage.
  - [x] 4.3 Require HTML native constraint, custom validation, and submit guard roles for form-validation prompts.
  - [x] 4.4 Split CSS animation roles from HTML app-shell roles.
  - _Requirements: 2.1, 2.2, 2.3_

## Phase 4: Improve Source-Backed Claims Without Fixture Text

- [x] 5. Improve dynamic symbol labels
  - [x] 5.1 Preserve JavaScript receiver-method aliases from source ranges where graph display names are weak.
  - [x] 5.2 Prefer application/router/response source anchors over examples and schema-reference component reports for request-dispatch roles.
  - [x] 5.3 Add a regression that Express names `createApplication`, `app.handle`, `app.use`, and `res.send` without embedding expected claim strings in production.
  - _Requirements: 3.1, 3.2, 3.3, 6.1_

## Phase 5: Make Compact Budget Proof-Aware

- [x] 6. Retain required proof before verbose packet sections
  - [x] 6.1 Change budget omission blocking to check missing required roles, not section names alone.
  - [x] 6.2 Prefer dropping diagrams, repeated snippets, or avoid-opening lists before proof citations and covered claims.
  - [x] 6.3 Add fmt and Swift compact regressions where quality-equivalent role coverage is sufficient despite truncation.
  - _Requirements: 4.1, 4.2, 4.3_

## Phase 6: Anti-Overfit And Promotion Gates

- [ ] 7. Strengthen production/eval boundary checks
  - [x] 7.1 Expand `scripts/lint-retrieval-generalization.mjs` to scan production packet modules for manifest-derived repo slugs, paths, symbols, and exact expected claims.
  - [x] 7.2 Add a fixture that proves a generic role rule can satisfy a synthetic non-benchmark repo prompt.
  - [x] 7.3 Run the 7-task subset before the full gate.
  - [ ] 7.4 Run the full `language-expansion-holdout` packet-runtime publishable gate with `--jobs 4` after subset pass.
  - _Requirements: 6.1, 6.2, 6.3_

## Verification Gates

- `node scripts/lint-retrieval-generalization.mjs`
- `node --test scripts/tests/codestory-agent-ab-analyzer.test.mjs`
- `node --test scripts/tests/codestory-benchmark-contract.test.mjs`
- `cargo test -p codestory-runtime --test retrieval_generalization_guard`
- `cargo test -p codestory-runtime --lib`
- `cargo test -p codestory-retrieval`
- targeted 7-task packet-runtime subset from the research note
- full `language-expansion-holdout` packet-runtime publishable gate with `--jobs 4 --prepare-codestory-jobs 1`
