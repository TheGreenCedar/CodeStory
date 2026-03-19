# Testing Matrix

```mermaid
flowchart TD
    change["What changed?"] --> always["Always run whole-workspace checks"]
    change --> indexer["Indexer, graph, or language work"]
    change --> store["Store, snapshot, trail, or search-doc work"]
    change --> runtime["Runtime, search, grounding, or orchestration work"]
    change --> cli["CLI args or output boundary work"]
    change --> bench["Bench or perf-surface work"]
    always --> workspace["fmt, check, test, clippy"]
    indexer --> fidelity["fidelity_regression, tictactoe_language_coverage, integration"]
    store --> store_tests["cargo test -p codestory-store"]
    runtime --> runtime_tests["cargo test -p codestory-runtime and retrieval_eval"]
    cli --> cli_tests["cargo test -p codestory-cli"]
    bench --> bench_checks["cargo check -p codestory-bench --benches"]
```

## Whole Workspace

```powershell
cargo fmt --check
cargo check
cargo test
cargo clippy --all-targets -- -D warnings
```

## Indexer And Graph Fidelity

```powershell
cargo test -p codestory-indexer --test fidelity_regression
cargo test -p codestory-indexer --test tictactoe_language_coverage
cargo test -p codestory-indexer --test integration
```

## Store Changes

```powershell
cargo test -p codestory-store
```

## Runtime Changes

```powershell
cargo test -p codestory-runtime
cargo test -p codestory-runtime --test retrieval_eval
```

## CLI Boundary And Output Changes

```powershell
cargo test -p codestory-cli
```

## Bench Surface Checks

```powershell
cargo check -p codestory-bench --benches
```
