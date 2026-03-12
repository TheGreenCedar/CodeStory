# CLI Subsystem

`codestory-cli` is the thin adapter for the six grounding workflows.

## Ownership

- parse command-line arguments
- resolve project and cache paths
- call runtime services
- render text or JSON

## Entry Points

- `crates/codestory-cli/src/main.rs`
- `crates/codestory-cli/src/args.rs`
- `crates/codestory-cli/src/runtime.rs`
- `crates/codestory-cli/src/output.rs`

## Extension Points

- add commands in `args.rs` and `main.rs`
- add renderers in `output.rs`
- keep business logic in runtime, not here

## Failure Signatures

- CLI depends directly on `codestory-store` or `codestory-indexer`
- output helpers start opening files or stores on their own
- command-specific orchestration is copied instead of delegated
