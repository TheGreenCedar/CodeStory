# Task 2 implementation report

## Result

Implemented the minimal exact-resolution proof overlay on schema 32. The durable
surface is limited to `proof_resolution_fact` and
`proof_resolution_publication`; publication is a full transactional
rematerialization inside the existing staged old-or-new core fence. Proof now
requires a matching authenticated exact fact after ordinary raw `CALL` edge
admission. Packet, context, search, navigation, Candidate 7, corpus, oracle,
cohort, and threshold contracts are unchanged.

## TDD evidence

### Slice 1 — closed facts and schema-32 storage

- RED command: `cargo test --locked -p codestory-store --test proof_resolution`
- RED result: failed to compile as intended with 18 errors. The first failures
  were `could not find proof_resolution in codestory_contracts`, no
  `seal_call_resolution_fact` export, and no proof-resolution Store read/write
  methods. No test reached execution.
- GREEN command: `cargo test --locked -p codestory-store --test proof_resolution`
- GREEN result on the accepted implementation: `4 passed; 0 failed; 0 ignored;
  0 measured; 0 filtered out; finished in 0.03s`.
- Covered migration without a synthetic receipt, deterministic round trip,
  evidence/hash/endpoint/callsite rejection without partial rows, stale
  publication rejection, and failed replacement preserving the prior complete
  publication.

### Slice 2 — parser-cache inputs and TypeScript/Rust rematerialization

- RED command: `cargo test --locked -p codestory-indexer --test proof_resolution`
- RED result: failed to compile as intended because
  `rematerialize_proof_resolution_projection` did not exist.
- GREEN command: `cargo test --locked -p codestory-indexer --test proof_resolution`
- GREEN result: `2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out;
  finished in 0.10s`.
- Covered TypeScript local and direct named-import calls, Rust same-file and
  inherent-method calls, full and incremental deterministic rematerialization,
  and the closed missing/ambiguous/unsupported/incomplete-domain statuses.
- Cache compatibility command:
  `cargo test --locked -p codestory-indexer parser_cache_without_resolution_inputs_decodes_as_an_empty_legacy_projection`
- Cache compatibility result: `1 passed; 0 failed; 0 ignored; 0 measured; 311
  filtered out; finished in 0.00s` in the owning unit-test binary. A decoded
  legacy cache remains navigation-compatible but cannot claim a complete proof
  projection.

### Slice 3 — authoritative runtime receipts

- RED command: `cargo test --locked -p codestory-runtime indexed_source_call_path_v1`
- RED result during the exact-callsite ordering slice: `19 passed; 1 failed`;
  `source_built_same_line_calls_use_source_order_before_adversarial_edge_ids`
  exposed raw-edge callsite columns that did not distinguish same-line calls.
- GREEN command: `cargo test --locked -p codestory-runtime indexed_source_call_path_v1`
- GREEN result: `20 passed; 0 failed; 0 ignored; 0 measured; 973 filtered out;
  finished in 0.41s` in the runtime unit-test binary; all integration binaries
  selected zero tests and passed.
- The correction binds the receipt to the exact source byte span and orders
  witnesses by native file identity, exact callsite start byte, then numeric
  edge ID. Missing facts remain `Unknown`; absent, stale, corrupt, or
  inconsistent projections are typed
  `proof_semantic_projection_unavailable`.
- Agent kernel command:
  `cargo test --locked -p codestory-agent indexed_source_call_path_v1`
- Agent kernel result: `45 passed; 0 failed; 0 ignored; 0 measured; 368 filtered
  out; finished in 0.02s` in the unit-test binary; the unrelated integration
  binary selected zero tests and passed.
- Publication-fence command:
  `cargo test --locked -p codestory-runtime publication_transitions_fail_or_cancel_atomically -- --test-threads=1`
- Publication-fence result: `2 passed; 0 failed; 0 ignored; 0 measured; 991
  filtered out; finished in 4.18s` in the runtime unit-test binary. Full and
  incremental failure/cancellation retain the old publication.

### Slice 4 — one-way boundary and frozen qualification compatibility

- Architecture-fence command:
  `cargo test --locked -p codestory-cli --test architecture_contracts exact_resolution_facts_are_a_one_way_proof_overlay`
- Architecture-fence result: `1 passed; 0 failed; 0 ignored; 0 measured; 56
  filtered out; finished in 0.07s`.
- Frozen-contract command:
  `cargo test --locked -p codestory-bench --test proof_availability_contracts`
- Frozen-contract result: `63 passed; 0 failed; 1 ignored; 0 measured; 0
  filtered out; finished in 37.03s`.
- Proof modules contain no `ResolutionCertainty` import and no
  `Edge.certainty` or `Edge.confidence` read. Retrieval, packet planning,
  packet/search/context runtime, CLI/catalog/API DTOs, and checked-in Candidate
  schemas are fenced from the private fact surface.

## Accepted focused gate

All commands ran serially.

- `cargo fmt --package codestory-contracts --package codestory-store --package codestory-indexer --package codestory-agent --package codestory-runtime --package codestory-bench -- --check`
  — passed with no output.
- `cargo check --locked -p codestory-contracts` — passed; finished in 4.99s.
- `cargo check --locked -p codestory-store` — passed; finished in 5.61s.
- `cargo check --locked -p codestory-indexer` — passed; finished in 9.71s.
- `cargo check --locked -p codestory-agent` — passed; finished in 4.19s.
- `cargo check --locked -p codestory-runtime` — passed; finished in 15.64s.
  The existing development-build warning that no embedding model is embedded
  was emitted; it is unrelated to this proof overlay.
- `cargo test --locked -p codestory-store --test proof_resolution` — 4 passed.
- `cargo test --locked -p codestory-indexer --test proof_resolution` — 2 passed.
- `cargo test --locked -p codestory-agent indexed_source_call_path_v1` — 45
  passed in the selected unit-test binary.
- `cargo test --locked -p codestory-runtime indexed_source_call_path_v1` — 20
  passed in the selected unit-test binary.
- `cargo test --locked -p codestory-bench --test proof_availability_contracts`
  — 63 passed, 1 ignored.
- `cargo test --locked -p codestory-indexer --test fidelity_regression` — 8
  passed; finished in 0.87s.
- `cargo test --locked -p codestory-indexer --test tictactoe_language_coverage`
  — 13 passed; finished in 0.64s.
- `git diff --check` — passed with no output before the implementation commit.

No release build, full-workspace proof, fresh qualification, public route,
push, PR, issue mutation, merge, or other-worktree action was performed.

## Changed-file accounting

- `crates/codestory-contracts/src/lib.rs` and
  `crates/codestory-contracts/src/proof_resolution.rs` — closed internal fact,
  callsite, status/reason, evidence, provenance, publication, adapter, and
  funnel contracts.
- `crates/codestory-store/src/lib.rs`,
  `crates/codestory-store/src/storage_impl/mod.rs`,
  `crates/codestory-store/src/storage_impl/schema.rs`,
  `crates/codestory-store/src/storage_impl/proof_resolution.rs`, and
  `crates/codestory-store/src/storage_impl/tests/mod.rs` — schema 32, the two
  proof tables, sealed fact/read/full-replacement/validation APIs, opaque parser
  cache enumeration, and schema-version expectation.
- `crates/codestory-store/tests/proof_resolution.rs` — migration, integrity,
  determinism, stale-publication, and failure-atomicity fixtures.
- `crates/codestory-indexer/src/cache.rs`,
  `crates/codestory-indexer/src/lib.rs`, and
  `crates/codestory-indexer/src/proof_resolution.rs` — schema-v1 compact parser
  inputs, exact callsite identity correction, conservative TypeScript/TSX and
  Rust reference adapters, complete cached-input resolution, deterministic
  full rematerialization, and funnel assembly.
- `crates/codestory-indexer/tests/proof_resolution.rs` — required TypeScript,
  Rust, non-exact, full/incremental, and deterministic-digest fixtures.
- `crates/codestory-agent/src/indexed_source_call_path_v1.rs` — proof kernel
  removes certainty/confidence admission and binds receipts to exact fact ID,
  evidence digest, and exact source byte.
- `crates/codestory-runtime/src/index_commit.rs`,
  `crates/codestory-runtime/src/indexed_source_call_path_v1.rs`, and
  `crates/codestory-runtime/src/proof_qualification_support.rs` — staged
  projection publication, typed availability, raw-edge-plus-fact admission,
  source-span authentication, traces, and internal compatibility mapping.
- `crates/codestory-bench/src/bin/codestory_proof_availability/contracts.rs`,
  `inventory.rs`, `report.rs`, and
  `crates/codestory-bench/tests/proof_availability_contracts.rs` — adapt the
  internal qualification seam to authenticated fact receipts while preserving
  checked-in schemas, Candidate 7, corpus, oracle, cohort, and thresholds.
- `crates/codestory-cli/tests/architecture_contracts.rs` — compile-time/source
  boundary fence for the one-way private overlay.
- `.superpowers/sdd/2026-08-23-minimal-exact-resolution-roadmap/task-2-minimal-exact-resolution-receipts-report.md`
  — this local task evidence and accounting artifact.

## Commit and tree

- Accepted implementation commit:
  `180a1a9b262eb679048e82cac79b426cbc3ff531`
- Accepted implementation tree: `41208bdfa58ce12b4e23d0168706c704579459be`
- Evidence-report commit: the commit that adds this report after recording the
  accepted implementation identity.

## Remaining concerns

- Schema-v0 parser cache artifacts decode for compatibility but intentionally
  cannot authorize a complete proof projection. A full indexing pass rebuilds
  them as schema-v1 inputs; until then proof fails closed and the prior
  navigation publication remains usable.
- The first adapters are intentionally conservative TypeScript/TSX and Rust
  reference verticals. Unsupported or incomplete evidence produces a closed
  non-exact fact instead of guessing. Full rematerialization is deliberately
  simple and may need measurement before broadening coverage.
- The repository CodeStory grounding tool did not converge past managed
  publication preparation in this worktree, and status reported a missing
  retrieval manifest. Source inspection and the focused test gates above were
  used instead; no live packaged-plugin claim is made.
