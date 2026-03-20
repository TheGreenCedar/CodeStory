# Indexer Subsystem

`codestory-indexer` is the workspace indexing engine. It turns a refresh plan into graph rows, occurrences, callable projection state, and post-flush resolution updates in `codestory-store`.

## Ownership

- language detection and parser or query selection
- file-level parse and extract work
- artifact-cache lookup and reuse
- batching and projection flush timing
- post-flush call, import, and override resolution
- incremental cleanup for touched and removed files
- indexing regression, fidelity, and language-coverage suites

## Main Modules

- `crates/codestory-indexer/src/lib.rs`: `WorkspaceIndexer`, feature flags, indexing phases, flush timing, and incremental cleanup
- `crates/codestory-indexer/src/intermediate_storage.rs`: in-memory batch shape before a store flush
- `crates/codestory-indexer/src/cache.rs`: serialized artifact-cache format and cache-key construction
- `crates/codestory-indexer/src/compilation_database.rs`: `compile_commands.json` discovery and parsed compilation metadata
- `crates/codestory-indexer/src/resolution/`: post-flush `ResolutionPass`, candidate selection, scoped resolution, and semantic fallback
- `crates/codestory-indexer/src/semantic/`: language-aware semantic helpers used by resolution

## Runtime Contract

The runtime owns orchestration and chooses the store shape for a run:

- full refresh: runtime opens a staged store, passes a full refresh plan to `WorkspaceIndexer`, then asks the store to finalize and publish the staged snapshot
- incremental refresh: runtime opens the live store, passes a diff-based refresh plan to `WorkspaceIndexer`, then asks the store to refresh live summary and detail snapshots

The indexer does not choose staged versus live storage. It only consumes a refresh plan plus a mutable store.

## Refresh Modes

`WorkspaceIndexer::run` handles both full and incremental work, but the plan changes the behavior:

- full refresh indexes every discovered source file, does not remove file rows, and runs unscoped resolution
- incremental refresh only touches files whose mtime increased or that were previously not indexed, tracks removed file IDs, seeds the symbol table from existing rows, and scopes resolution to touched files

Incremental work also does more cleanup:

- before merging new results for a touched file, the indexer may delete stale callable projection state for that file
- after the resolution pass, the indexer removes files that disappeared from the workspace

## Pipeline

The core path inside `WorkspaceIndexer::run` is:

1. Seed symbol state for incremental runs from existing stored node kinds.
2. Walk `files_to_index` in chunks using the configured batch sizes.
3. For each file, normalize the path, load compilation metadata if available, and skip unsupported files before parsing.
4. Try the artifact cache first. A cache hit can reuse stored nodes, edges, occurrences, component access, and callable projection state without reparsing the file.
5. Parse cache misses in parallel and turn each file into `IntermediateStorage`.
6. Merge per-file results into a batched in-memory projection and flush once file, node, edge, or occurrence thresholds are reached.
7. Flush any remaining batched data.
8. Run `ResolutionPass` after all projection writes are visible in the store.
9. Flush collected indexing errors.
10. For incremental runs, delete removed files from the store.

## What Gets Flushed

Projection flushes are broader than just graph rows. `IntermediateStorage` carries:

- file metadata
- nodes
- edges
- occurrences
- component access tuples
- callable projection state
- impl anchor node IDs
- indexing errors

`flush_projection_batch` writes the projection payload through `codestory-store`. Resolution changes happen later, after those rows already exist.

## Resolution and Semantic Fallback

Resolution is a post-flush pass because it depends on the stored graph state:

- the indexer first persists unresolved edges and callable projection state
- `ResolutionPass` then loads unresolved call, import, and override edges from the store
- candidate selection prefers structural matches first and can use semantic candidate indexes as a fallback for supported languages
- incremental runs scope the resolution pass to touched files; full refresh resolves across the full workspace

This keeps parse and extract logic in the indexer while leaving persistence and snapshot ownership in the store.

## Extension Points

- add language detection or parser support in the indexer crate, not in runtime
- add new extraction or projection data in `IntermediateStorage` and the store projection flush path together
- add new resolution strategies inside `src/resolution/`
- add compilation-metadata-aware behavior through `compilation_database.rs`
- add regression fixtures or suites in `crates/codestory-indexer/tests/`

## Failure Signatures

- indexing behavior moves into runtime or CLI instead of staying in `codestory-indexer`
- unsupported files are treated as runtime concerns instead of being skipped by the indexer
- resolution logic is applied before graph data is flushed
- projection flushes change without matching updates to cleanup or snapshot invalidation expectations
- new indexing behavior lands without a fidelity or regression test in `crates/codestory-indexer/tests/`

## Read Next

- [Indexing pipeline](../indexing-pipeline.md)
- [Runtime execution path](../runtime-execution-path.md)
- [Store subsystem](store.md)
