# Implementation Plan

- [x] 1. Repair default retrieval test hygiene
  - [x] 1.1 Move `integration_query_against_fixture_manifest` behind `#[ignore]` or `CODESTORY_LIVE_SIDECAR_TESTS=1`.
  - [x] 1.2 Replace shallow live reachability skip with a full preflight or controlled live-only failure message.
  - [x] 1.3 Add a hermetic retrieval query fixture or mock test for success and unavailable sidecar behavior.
  - [x] 1.4 Remove live-sidecar `expect("index")` panics from default test paths.
  - [x] 1.5 Verify `cargo test -p codestory-retrieval` with sidecars down or absent.
  - _Requirements: 1.1, 1.2, 1.3, 1.4_

- [x] 2. Restore branch-head release proof
  - [x] 2.1 Run `cargo build --release -p codestory-cli` at branch `HEAD`.
  - [x] 2.2 Run `cargo test -p codestory-cli --test codestory_repo_e2e_stats -- --ignored --nocapture` at the same `HEAD`.
  - [x] 2.3 Append the emitted row to every relevant table in `docs/testing/codestory-e2e-stats-log.md`.
  - [x] 2.4 After final docs changes, rerun `ready` or `doctor` if sidecar input hash or readiness proof may have changed.
  - _Requirements: 2.1, 2.2, 2.3, 2.4_

- [x] 3. Make sidecar candidate-resolution failures visible
  - [x] 3.1 Replace `unwrap_or_default()` in `try_sidecar_primary_search` with an unavailable outcome that includes candidate-resolution failure.
  - [x] 3.2 Replace `unwrap_or_default()` in `search_results_sidecar_primary` with explicit `sidecar_retrieval_unavailable_error` mapping.
  - [x] 3.3 Add runtime regression tests for both sidecar primary search paths.
  - [x] 3.4 Verify packet batch behavior still maps resolution failures consistently.
  - _Requirements: 3.1, 3.2, 3.3, 3.4_

- [x] 4. Split publishable benchmark evidence from diagnostic assistance
  - [x] 4.1 Stop injecting manifest `expected_files` and expected symbols into publishable packet preludes by default.
  - [x] 4.2 Add an explicit diagnostic flag for manifest-derived extra probes and record `evidence_mode`.
  - [x] 4.3 Block oracle-assisted rows from `--publishable` summaries unless the output is explicitly diagnostic-only.
  - [x] 4.4 Require explicit `--max-source-reads-after-packet` policy for publishable agent A/B rows and label CodeStory-first versus packet-only rows.
  - [x] 4.5 Make packet-gate zero-selection exit non-zero unless `--allow-empty-packet-gate` is present.
  - [x] 4.6 Add Node tests for publishable blockers and packet-gate empty behavior.
  - _Requirements: 4.1, 4.2, 4.3, 4.4_

- [x] 5. Harden benchmark artifact reuse
  - [x] 5.1 Canonicalize reusable artifact paths under the source run directory.
  - [x] 5.2 Reject absolute, escaping, missing, or unexpected artifact names.
  - [x] 5.3 Add a copied-artifact size cap.
  - [x] 5.4 Add tests proving malicious `runs.jsonl` paths cannot copy local sensitive files.
  - _Requirements: 4.5, 5.3_

- [x] 6. Enforce CLI local file containment
  - [x] 6.1 Add a shared project-contained path helper in the CLI drill path.
  - [x] 6.2 Apply containment to endpoint files, search-hit files, and relative import candidates before metadata/read.
  - [x] 6.3 Add tests for absolute import rejection and `..` traversal rejection.
  - [x] 6.4 Keep rejected-path output content-free and diagnostically useful.
  - _Requirements: 5.1, 5.2, 5.4_

- [x] 7. Verify managed model artifacts
  - [x] 7.1 Add a pinned SHA-256 constant for the configured GGUF artifact.
  - [x] 7.2 Download to a temp path and verify checksum before final rename.
  - [x] 7.3 Delete temp files and fail clearly on checksum mismatch.
  - [x] 7.4 Make fallback mirrors explicit opt-in or prove them by the same checksum.
  - [x] 7.5 Update setup and sidecar docs with checksum and mirror behavior.
  - _Requirements: 6.1, 6.2, 6.3, 6.4_

- [x] 8. Stabilize packet sufficiency JSON
  - [x] 8.1 Change `avoid_opening` from prose strings to typed raw path plus reason entries, or add a parallel raw-path field with compatibility handling.
  - [x] 8.2 Sort deduped avoid-opening paths before truncation.
  - [x] 8.3 Update benchmark composition scoring to consume raw paths only.
  - [x] 8.4 Move fallback summary claims out of proof-bearing `covered_claims`, or compute all proof claims before status.
  - [x] 8.5 Add Rust and Node golden/schema tests for packet sufficiency shape.
  - _Requirements: 7.1, 7.2, 7.3, 7.4_

- [x] 9. Complete readiness degraded-state coverage
  - [x] 9.1 Add `ready` tests for missing, unchecked, and stale indexes.
  - [x] 9.2 Add `ready --goal agent` tests for unavailable and non-full sidecar retrieval.
  - [x] 9.3 Decide whether `cache_busy` is a real structured readiness status; wire it or remove it.
  - [x] 9.4 Align readiness docs and command examples with tested output.
  - _Requirements: 8.1, 8.2, 8.3, 8.4_

- [x] 10. Add performance budgets and stress protection
  - [x] 10.1 Split compact/default packet budgets from explicit standard/deep quality budgets.
  - [x] 10.2 Add or update packet runtime smoke gating so SLA misses fail or are listed as explicit exceptions.
  - [x] 10.3 Stream or cache sidecar input fingerprinting instead of collecting full lexical entries and symbol docs for ordinary status paths.
  - [x] 10.4 Build per-file lookup maps for manual parser resolution passes or add a targeted stress benchmark before further expansion.
  - [x] 10.5 Record benchmark evidence for packet latency, strict status, and large single-file parser behavior.
  - _Requirements: 9.1, 9.2, 9.3, 9.4_

- [x] 11. Consolidate language-support ownership
  - [x] 11.1 Add an alignment test that walks registry profiles and verifies parser routing and workspace source-group behavior.
  - [x] 11.2 Extract language-specific parser/ruleset construction from `crates/codestory-indexer/src/lib.rs` into per-language modules.
  - [x] 11.3 Update language-support docs to keep parser-backed, structural, semantic, route/framework, and packet-quality claims separate.
  - [x] 11.4 Label OSS corpus evidence as raw-file-list indexer evidence unless a CLI/runtime smoke is added.
  - _Requirements: 10.1, 10.2, 10.3, 10.4_

- [x] 12. Rebalance production packet semantics and eval docs
  - [x] 12.1 Audit `packet_claim_profiles.rs` for library-specific benchmark-shaped claims.
  - [x] 12.2 Move exact row-specific claims/probes to manifests, eval-only tests, or explicit diagnostic extra probes.
  - [x] 12.3 Keep production profiles source-pattern-derived and phrased as general evidence roles or cautious claim candidates.
  - [x] 12.4 Fix `CODESTORY_EVAL_PROBES` docs so the documented command actually exercises eval behavior, or document the supported diagnostic route instead.
  - [x] 12.5 Run and preserve production generalization lint.
  - _Requirements: 11.1, 11.2, 11.3, 11.4_

- [x] 13. Clear quality-gate and docs drift
  - [x] 13.1 Fix clippy warnings in `crates/codestory-workspace/src/lib.rs` and `crates/codestory-store/src/storage_impl/mod.rs` without broad allows.
  - [x] 13.2 Align `docs/ops/retrieval-sidecars.md` with `.github/workflows/retrieval-sidecar-smoke.yml`, or update the workflow to match the runbook.
  - [x] 13.3 Update nearest durable docs or `.agents/skills/codestory-grounding` references for every changed command or behavior.
  - _Requirements: 12.1, 12.2, 12.3_

- [x] 14. Run final verification bundle
  - [x] 14.1 Run `cargo fmt --check --verbose`.
  - [x] 14.2 Run `cargo clippy --workspace --all-targets -- -D warnings`.
  - [x] 14.3 Run `cargo check --workspace`.
  - [x] 14.4 Run focused Rust tests for retrieval, runtime sidecar primary behavior, CLI readiness, indexer language coverage, and packet sufficiency.
  - [x] 14.5 Run focused Node tests and benchmark harness self-tests.
  - [x] 14.6 Run the release e2e proof and append the branch-head stats row.
  - [x] 14.7 Record skipped live gates, intentional diagnostic-only evidence, and remaining non-blocking follow-ups.
  - _Requirements: 12.4, 2.1, 2.2, 2.3, 2.4_
