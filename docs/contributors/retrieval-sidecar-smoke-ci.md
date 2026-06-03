# CI manifest-missing smoke: `retrieval-sidecar-smoke` (Windows)

**Status:** workflow checked in at [`.github/workflows/retrieval-sidecar-smoke.yml`](../../.github/workflows/retrieval-sidecar-smoke.yml).
Full index/query on the monorepo may exceed runner budgets; the job runs bootstrap with
`--skip-compose --wait-secs 0`, asserts `retrieval status` returns the clean pre-index
`retrieval_manifest_missing` shape through the CLI integration test suite, and runs
runtime/retrieval protocol plus non-live CLI search contract tests. This job is not a full sidecar
readiness gate. The workflow restores a Rust build cache before the Cargo steps; a new cache key may
still pay one cold compile, but later pushes should reuse the warmed target and Cargo dependency
state.

**Preflight reference:** [`docs/ops/retrieval-sidecars.md`](../ops/retrieval-sidecars.md#preflight-smoke-contract)

---

## Purpose

Fail PRs that touch retrieval/runtime/stdio/search wiring when the manifest-missing status shape
or associated Rust contracts drift on a clean Windows runner.

## Trigger paths (suggested)

```yaml
paths:
  - crates/codestory-retrieval/**
  - crates/codestory-cli/src/**/retrieval*
  - crates/codestory-cli/src/stdio_*.rs
  - crates/codestory-cli/tests/retrieval_bootstrap_contracts.rs
  - crates/codestory-cli/tests/search_json_output.rs
  - crates/codestory-cli/tests/stdio_protocol_contracts.rs
  - crates/codestory-runtime/src/**
  - crates/codestory-indexer/Cargo.toml
  - crates/codestory-indexer/src/lib.rs
  - docs/ops/retrieval-sidecars.md
```

## Job sketch (PowerShell)

```powershell
# After checkout, Node setup, Rust toolchain setup, and Rust cache restore:
node scripts/lint-retrieval-generalization.mjs
cargo test -p codestory-cli --test retrieval_bootstrap_contracts
cargo test -p codestory-runtime --lib
cargo test -p codestory-runtime --test retrieval_generalization_guard
cargo test -p codestory-cli --test stdio_protocol_contracts
cargo test -p codestory-cli --test search_json_output
cargo test -p codestory-retrieval
```

Use a tiny fixture repo if this workflow later grows to include indexed full-mode smoke coverage;
bootstrap with `--skip-compose` does not start sidecars, fetch the GGUF model, or create the
retrieval manifest required for `retrieval_mode == "full"`.

## Pass criteria

1. Generalization lint exits 0.
2. Rust cache restore/save completes or gracefully misses without masking later failures.
3. `cargo test -p codestory-cli --test retrieval_bootstrap_contracts` exits 0, including the
   bootstrap/status assertion that reports `degraded_reason == "retrieval_manifest_missing"` and
   non-`full` mode on a clean temp project before indexing.
4. `cargo test -p codestory-runtime --lib` exits 0.
5. `cargo test -p codestory-runtime --test retrieval_generalization_guard` exits 0.
6. `cargo test -p codestory-cli --test stdio_protocol_contracts` exits 0.
7. `cargo test -p codestory-cli --test search_json_output` exits 0 for non-live fail-closed search contracts.
8. `cargo test -p codestory-retrieval` exits 0.

## Pins

Match [`docs/ops/retrieval-sidecars.md`](../ops/retrieval-sidecars.md) version table (real Zoekt,
`qdrant/qdrant:v1.12.5`, generated SCIP graph artifacts).

## Related tests (local substitute)

```powershell
node scripts/lint-retrieval-generalization.mjs
cargo test -p codestory-cli --test retrieval_bootstrap_contracts
cargo test -p codestory-runtime --lib
cargo test -p codestory-runtime --test retrieval_generalization_guard
cargo test -p codestory-cli --test stdio_protocol_contracts
cargo test -p codestory-cli --test search_json_output
cargo test -p codestory-retrieval
```

The workflow runs the lint script and focused test targets. The manifest-missing smoke lives in
`retrieval_bootstrap_contracts` so Cargo builds the CLI through the integration-test path instead of
paying for a standalone build step before the tests. The Rust cache is configured to save even on
failure, which keeps failed follow-up pushes from repeatedly paying the full Windows cold-compile
cost. `retrieval_generalization_guard` invokes the same lint from Rust for cross-platform CI parity.
This smoke job does not claim stdio, CLI, or runtime full-mode success. Full readiness evidence
requires a separate fixture run that starts real sidecars, provisions `bge-base-en-v1.5.Q8_0.gguf`,
runs `retrieval index`, and verifies `retrieval_mode == "full"`. The live success contracts are
intentionally outside the normal smoke gate: set `CODESTORY_STDIO_FULL_RETRIEVAL_TESTS=1` before
running the stdio full-mode
contracts with `-- --ignored --nocapture`, run
`cargo test -p codestory-cli --test search_json_output -- --ignored --nocapture search_json_emits_sidecar_primary_results_without_repo_text_fallback`
for the CLI lane, and run the ignored `retrieval_eval_*` tests with
`CODESTORY_RETRIEVAL_EVAL_FULL_TESTS=1` only after the sidecar fixture is prepared. Without those
preconditions, the live lanes are blocked/skipped by name rather than silently passing.
