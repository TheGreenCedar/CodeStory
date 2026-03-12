# Indexer Subsystem

`codestory-indexer` is the indexing engine.

## Ownership

- language and rule selection
- parse/extract flow
- graph artifact construction
- semantic resolution
- incremental/full refresh execution
- fidelity, language-coverage, and resolution regression suites

## Entry Points

- `crates/codestory-indexer/src/lib.rs`
- `crates/codestory-indexer/src/resolution/`
- `crates/codestory-indexer/src/semantic/`
- `crates/codestory-indexer/tests/`

## Call Chain

1. Runtime computes a refresh plan through `codestory-workspace`.
2. Runtime passes that plan plus a mutable store to `WorkspaceIndexer`.
3. Indexer parses files, builds nodes/edges/occurrences, resolves relationships, and flushes batches through `codestory-store`.
4. Store invalidates or refreshes derived snapshots after writes.

## Extension Points

- add a new language in the registry and semantic modules
- add a new extraction or resolution stage in the indexer crate, not runtime
- add regression fixtures in `tests/fixtures/` and wire them into the existing suites

## Failure Signatures

- indexing code starts depending on runtime search or CLI rendering
- resolution logic is implemented in runtime instead of indexer
- new indexing behavior lands without a matching regression test in `tests/`
