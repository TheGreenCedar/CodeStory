# Testing

Use [testing-matrix.md](testing-matrix.md) as the primary guide.

## Always Relevant

```powershell
cargo fmt --check
cargo check
cargo test
cargo clippy --all-targets -- -D warnings
```

## High-Risk Graph Changes

```powershell
cargo test -p codestory-indexer --test fidelity_regression
cargo test -p codestory-indexer --test tictactoe_language_coverage
cargo test -p codestory-runtime --test retrieval_eval
```

