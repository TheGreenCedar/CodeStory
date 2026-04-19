# Runtime Subsystem

`codestory-runtime` is the only orchestration layer.

## Ownership

- project open and summary flows
- full and incremental indexing orchestration
- runtime-owned search engine state and ranking
- semantic doc synchronization, embedding reuse, and retrieval readiness reporting
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

## Search And Semantic Sync

Runtime owns the default semantic-sync path after graph indexing completes. The store owns persisted rows, but runtime decides when to build semantic docs, when to reuse or embed them, when to reload them into the search engine, and how to report readiness to CLI callers.

Important tuning surfaces:

- `CODESTORY_SEMANTIC_DOC_SCOPE`: default durable symbols; use `all` for the older broad symbol set
- `CODESTORY_LLM_DOC_EMBED_BATCH_SIZE`: semantic doc embedding batch size, default `64`
- `CODESTORY_EMBED_SESSION_COUNT`: ONNX embedding worker count, clamped from `1` to `16`
- `CODESTORY_EMBED_INTRA_THREADS`, `CODESTORY_EMBED_INTER_THREADS`, `CODESTORY_EMBED_PARALLEL_EXECUTION`: ONNX CPU execution tuning
- `CODESTORY_EMBED_EXECUTION_PROVIDER`: `cpu`, `cuda`, or `directml`; CUDA and DirectML require the matching Cargo feature

Timing fields for this path are in `IndexingPhaseTimings`: `semantic_doc_build_ms`, `semantic_embedding_ms`, `semantic_db_upsert_ms`, `semantic_reload_ms`, `semantic_docs_reused`, `semantic_docs_embedded`, `semantic_docs_pending`, and `semantic_docs_stale`.

## Failure Signatures

- runtime regains direct persistence logic
- search engine internals become public API
- CLI formatting concerns start driving runtime behavior
- semantic docs become an implicit background side effect instead of an explicit index phase
