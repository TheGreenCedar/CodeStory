# CI manifest-missing smoke: `retrieval-sidecar-smoke` (Windows)

**Status:** workflow checked in at [`.github/workflows/retrieval-sidecar-smoke.yml`](../../.github/workflows/retrieval-sidecar-smoke.yml).
Full index/query on the monorepo may exceed runner budgets; the job runs bootstrap with
`--skip-compose --wait-secs 0`, asserts `retrieval status` returns the clean pre-index
`retrieval_manifest_missing` shape, and runs runtime/retrieval protocol plus non-live CLI search
contract tests. This job is not a full sidecar readiness gate.

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
  - crates/codestory-cli/tests/search_json_output.rs
  - crates/codestory-cli/tests/stdio_protocol_contracts.rs
  - crates/codestory-runtime/src/**
  - crates/codestory-indexer/Cargo.toml
  - crates/codestory-indexer/src/lib.rs
  - docs/ops/retrieval-sidecars.md
```

## Job sketch (PowerShell)

```powershell
node scripts/lint-retrieval-generalization.mjs

cargo build --release -p codestory-cli
$cli = ".\target\release\codestory-cli.exe"

& $cli retrieval bootstrap --project . --skip-compose --wait-secs 0
if ($LASTEXITCODE -ne 0) {
  throw "retrieval bootstrap failed with exit code $LASTEXITCODE"
}

$statusJson = & $cli retrieval status --project .
if ($LASTEXITCODE -ne 0) {
  throw "retrieval status failed with exit code $LASTEXITCODE"
}

$statusText = ($statusJson | Out-String).Trim()
Write-Host $statusText
$status = $statusText | ConvertFrom-Json

if ($status.degraded_reason -ne "retrieval_manifest_missing") {
  throw "retrieval-sidecar-smoke expected manifest-missing shape before indexing"
}

if ($status.retrieval_mode -eq "full") {
  throw "manifest-missing shape check must not report full mode before retrieval index"
}

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
2. Release `codestory-cli` build exits 0.
3. `retrieval bootstrap --project . --skip-compose --wait-secs 0` exits 0 with logs visible for diagnostics.
4. `retrieval status --project .` emits readable JSON and reports `degraded_reason ==
   "retrieval_manifest_missing"` and non-`full` mode on the clean runner before indexing.
5. `cargo test -p codestory-runtime --lib` exits 0.
6. `cargo test -p codestory-runtime --test retrieval_generalization_guard` exits 0.
7. `cargo test -p codestory-cli --test stdio_protocol_contracts` exits 0.
8. `cargo test -p codestory-cli --test search_json_output` exits 0 for non-live fail-closed search contracts.
9. `cargo test -p codestory-retrieval` exits 0.

## Pins

Match [`docs/ops/retrieval-sidecars.md`](../ops/retrieval-sidecars.md) version table (real Zoekt,
`qdrant/qdrant:v1.12.5`, generated SCIP graph artifacts).

## Related tests (local substitute)

```powershell
node scripts/lint-retrieval-generalization.mjs
cargo test -p codestory-runtime --lib
cargo test -p codestory-runtime --test retrieval_generalization_guard
cargo test -p codestory-cli --test stdio_protocol_contracts
cargo test -p codestory-cli --test search_json_output
cargo test -p codestory-retrieval
```

The workflow runs the lint script and the listed test targets; `retrieval_generalization_guard`
invokes the same lint from Rust for cross-platform CI parity. This smoke job does not claim
stdio, CLI, or runtime full-mode success. Full readiness evidence requires a separate fixture run
that starts real sidecars, provisions `bge-base-en-v1.5.Q8_0.gguf`, runs `retrieval index`, and
verifies `retrieval_mode == "full"`. The live success contracts are intentionally outside the
normal smoke gate: set `CODESTORY_STDIO_FULL_RETRIEVAL_TESTS=1` before running the stdio full-mode
contracts with `-- --ignored --nocapture`, run
`cargo test -p codestory-cli --test search_json_output -- --ignored --nocapture search_json_emits_sidecar_primary_results_without_repo_text_fallback`
for the CLI lane, and run the ignored `retrieval_eval_*` tests with
`CODESTORY_RETRIEVAL_EVAL_FULL_TESTS=1` only after the sidecar fixture is prepared. Without those
preconditions, the live lanes are blocked/skipped by name rather than silently passing.
