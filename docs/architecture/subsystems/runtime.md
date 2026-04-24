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
- `CODESTORY_SEMANTIC_DOC_ALIAS_MODE`: default `alias_variant`; use `no_alias` for baseline research rows or `current_alias` for the older full alias text
- `CODESTORY_EMBED_BACKEND`: `llamacpp` or `hash`
- `CODESTORY_EMBED_PROFILE`: built-in profile; defaults to `bge-base-en-v1.5`; explicit profiles include `minilm`, `bge-small-en-v1.5`, `bge-base-en-v1.5`, `qwen3-embedding-0.6b`, `embeddinggemma-300m`, `nomic-embed-text-v1.5`, or `nomic-embed-text-v2-moe`
- `CODESTORY_EMBED_LLAMACPP_URL`: OpenAI-compatible llama.cpp embedding endpoint, default `http://127.0.0.1:8080/v1/embeddings`
- `CODESTORY_EMBED_LLAMACPP_REQUEST_COUNT`: number of concurrent llama.cpp embedding requests, clamped from `1` to `16`
- `CODESTORY_LLM_DOC_EMBED_BATCH_SIZE`: semantic doc embedding batch size, default `128`

llama.cpp lets CodeStory use GGUF embedding models through an HTTP server, so
GPU acceleration can be provided by the server without making the runtime
platform-specific. The `hash` backend remains for deterministic local-dev and
CI checks. Current local benchmark findings and recommendations live in
[embedding-backend-benchmarks.md](../../testing/embedding-backend-benchmarks.md).

Timing fields for this path are in `IndexingPhaseTimings`: `semantic_doc_build_ms`, `semantic_embedding_ms`, `semantic_db_upsert_ms`, `semantic_reload_ms`, `semantic_docs_reused`, `semantic_docs_embedded`, `semantic_docs_pending`, and `semantic_docs_stale`.

## Failure Signatures

- runtime regains direct persistence logic
- search engine internals become public API
- CLI formatting concerns start driving runtime behavior
- semantic docs become an implicit background side effect instead of an explicit index phase
