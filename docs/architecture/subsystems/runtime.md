# Runtime Subsystem

`codestory-runtime` is the only orchestration layer.

## Ownership

- project open and summary flows
- full and incremental indexing orchestration
- runtime-owned search engine state and ranking
- symbol-doc synchronization, dense-anchor reuse, and retrieval readiness reporting
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

Runtime owns the default semantic-sync path after graph indexing completes. The store owns persisted rows, but runtime decides when to build graph-native symbol docs, when to build component reports, when to classify dense anchors under `graph_first_v1`, when to reuse or embed selected dense anchors, when to reload them into the search engine, and how to report readiness to CLI callers.

Important tuning surfaces:

- `CODESTORY_SEMANTIC_DOC_SCOPE`: default durable symbol-doc scope; use `all` only for diagnostics that need the older broad symbol set
- `CODESTORY_SEMANTIC_DOC_ALIAS_MODE`: default `alias_variant`; use `no_alias` for baseline research rows or `current_alias` for the older full alias text
- `CODESTORY_SEMANTIC_DOC_MAX_TOKENS`: generated symbol-doc and dense-anchor text token budget.
- `CODESTORY_EMBED_BACKEND`: product sidecar indexing requires `llamacpp`.
- `CODESTORY_EMBED_LLAMACPP_URL`: local OpenAI-compatible llama.cpp embedding endpoint for `CODESTORY_EMBED_BACKEND=llamacpp`.
- `CODESTORY_EMBED_LLAMACPP_REQUEST_COUNT`: local llama.cpp request concurrency, clamped from `1` to `16`.
- `CODESTORY_LLM_DOC_EMBED_BATCH_SIZE`: semantic doc embedding batch size, default `128`.

Product packet/search evidence is served through mandatory sidecars. The live
query sidecar must use `llamacpp:bge-base-en-v1.5`, and health must report
`retrieval_mode=full`. Stored legacy ONNX, hash, or other diagnostic producer metadata
must fail closed when it does not match the current llama.cpp product manifest;
hash and other in-process embedding paths remain diagnostic. Current benchmark findings live in
[embedding-backend-benchmarks.md](../../testing/embedding-backend-benchmarks.md).

The CLI owns managed embedding setup. `codestory-cli retrieval bootstrap` starts
the local llama.cpp sidecar when Docker Compose is available; `retrieval index`
then writes generation-bound sidecar artifacts and manifest metadata. Missing
or non-product embedding state fails closed for agent-facing retrieval.

Timing fields for this path are in `IndexingPhaseTimings`: `search_projection_rebuild_ms`, `search_symbol_index_ms`, `runtime_cache_publish_ms`, `semantic_doc_build_ms`, `semantic_embedding_ms`, `semantic_db_upsert_ms`, `semantic_reload_ms`, `semantic_prune_ms`, `symbol_search_docs_written`, `semantic_dense_docs_skipped`, dense reason counters, `semantic_docs_reused`, `semantic_docs_embedded`, `semantic_docs_pending`, and `semantic_docs_stale`.

## Failure Signatures

- runtime regains direct persistence logic
- search engine internals become public API
- CLI formatting concerns start driving runtime behavior
- symbol docs or dense anchors become an implicit background side effect instead of an explicit index phase
