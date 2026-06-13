# Third-Pass Eval Boundary Plan

> **For:** Final merge-readiness review after second-pass agents found stale proof and remaining holdout-shaped production claim synthesis.
> **Status:** In progress
> **Owner:** Codex

## Goal

Make production packet/search behavior honest by removing remaining holdout-family source-claim and exact-probe steering from `orchestrator.rs`, while preserving benchmark diagnostics behind the env-gated eval probe boundary.

## Tasks

- [x] **Task 1: Map remaining production holdout-shaped paths**
  - Found exact Requests and Express query/source-claim paths in production packet planning.
  - Found row-shaped source-claim generators for Jekyll/site build, Monolog-style log records, AutoMapper, Okio-style buffered IO, Alamofire-style request validation, and custom form validation.

- [x] **Task 2: Move exact family behavior behind eval probes**
  - Removed exact Requests/Express production query and source-claim branches.
  - Removed row-shaped family source-claim generators from production orchestration.
  - Added eval-only replacements in `crates/codestory-runtime/src/agent/eval_probes.rs`.
  - Added eval manifest rules for Requests and Express exact probes.

- [x] **Task 3: Harden production lint and regression tests**
  - Extended `scripts/lint-retrieval-generalization.mjs` to ban the newly identified exact family anchors in production Rust.
  - Added production-mode regression coverage for Requests/Express exact probes and broadened source-claim boundary coverage.
  - Updated exact family tests to opt into `CODESTORY_EVAL_PROBES` through the test override guard.

- [x] **Task 4: Focused verification**
  - `node scripts\lint-retrieval-generalization.mjs`
  - `node -e "JSON.parse(require('node:fs').readFileSync('benchmarks/tasks/eval-probes.json','utf8')); console.log('eval-probes json ok')"`
  - `git diff --check`
  - `cargo test -p codestory-runtime --test retrieval_generalization_guard -- --nocapture`
  - `cargo test -p codestory-runtime packet_plan_keeps_requests_and_express_exact_probes_eval_only -- --nocapture`
  - `cargo test -p codestory-runtime exact_family_source_claims_require_eval_probes -- --nocapture`
  - `cargo test -p codestory-runtime packet_plan_adds_prepared_session_adapter_exact_probes -- --nocapture`
  - `cargo test -p codestory-runtime route_tracing_packet_plan_seeds_express_app_route_probes_when_prompt_names_express -- --nocapture`
  - `cargo test -p codestory-runtime site_build_claims_survive_with_generic_claims -- --nocapture`
  - `cargo test -p codestory-runtime express_route_flow_source_claims_name_app_router_response_flow -- --nocapture`
  - `cargo test -p codestory-runtime python_requests_source_claims_name_method_flow -- --nocapture`
  - `cargo test -p codestory-runtime packet_supported_claims_generic_source_claims_are_domain_neutral_without_eval_probes -- --nocapture`
  - `cargo check --workspace`
  - `node scripts\codestory-language-holdout-integrity.mjs`

- [ ] **Task 5: Final proof at current tree**
  - Rebuild release CLI.
  - Repair/rebuild retrieval sidecars for the final tree.
  - Rerun `ready` and `doctor`; both must report full/fresh retrieval.
  - Rerun ignored `codestory_repo_e2e_stats` and append the fresh stats row.
  - Run a final independent review on the resulting tree.
