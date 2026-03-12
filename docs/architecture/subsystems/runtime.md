# Runtime Subsystem

`codestory-runtime` is the only orchestration layer.

## Ownership

- project open and summary flows
- full and incremental indexing orchestration
- runtime-owned search engine state and ranking
- grounding, trail, symbol, and snippet assembly
- agent-oriented retrieval and answer flows

## Entry Points

- `crates/codestory-runtime/src/lib.rs`
- `crates/codestory-runtime/src/services.rs`
- `crates/codestory-runtime/src/search/`
- `crates/codestory-runtime/src/grounding.rs`
- `crates/codestory-runtime/src/support.rs`

## Call Chain

1. CLI builds a runtime context.
2. Runtime opens the workspace and store.
3. Runtime calls into indexer and store as needed.
4. Runtime maps persisted data into contract DTOs.
5. CLI renders results.

## Extension Points

- add new public services in `services.rs`
- add new retrieval logic under `src/search/`
- add new grounding or agent flows under runtime modules, not CLI

## Failure Signatures

- runtime regains direct persistence logic
- search engine internals become public API
- CLI formatting concerns start driving runtime behavior
