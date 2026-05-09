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
- `CODESTORY_SEMANTIC_DOC_MAX_TOKENS`: generated semantic-doc token budget; managed ONNX setup seeds `512` unless explicitly set
- `CODESTORY_EMBED_BACKEND`: `onnx`, `llamacpp`, or `hash`
- `CODESTORY_EMBED_PROFILE`: built-in profile; defaults to `bge-base-en-v1.5`; explicit profiles include `minilm`, `bge-small-en-v1.5`, `bge-base-en-v1.5`, `qwen3-embedding-0.6b`, `embeddinggemma-300m`, `nomic-embed-text-v1.5`, or `nomic-embed-text-v2-moe`
- `CODESTORY_EMBED_ONNX_MODEL`: path to the ONNX embedding graph
- `CODESTORY_EMBED_ONNX_TOKENIZER`: path to the matching Hugging Face `tokenizer.json`
- `CODESTORY_EMBED_ONNX_PROVIDER`: `directml`, `cpu`, or `auto`
- `CODESTORY_EMBED_ONNX_BATCH_TOKENS`: max padded tokens per ORT call; managed ONNX setup seeds `32768`
- `CODESTORY_EMBED_ONNX_THREADS`: optional ORT intra-op thread count for CPU-oriented runs
- `CODESTORY_EMBED_LLAMACPP_URL`: legacy OpenAI-compatible llama.cpp embedding endpoint for `CODESTORY_EMBED_BACKEND=llamacpp`
- `CODESTORY_EMBED_LLAMACPP_REQUEST_COUNT`: legacy llama.cpp request concurrency, clamped from `1` to `16`
- `CODESTORY_LLM_DOC_EMBED_BATCH_SIZE`: semantic doc embedding batch size, default `128`; managed ONNX setup seeds `2048` unless explicitly set
- `CODESTORY_STORED_VECTOR_ENCODING`: in-memory search-vector encoding; managed ONNX setup seeds `int8` unless explicitly set

ONNX Runtime is the managed real-model path. Runtime loads the tokenizer and
graph in-process, feeds `input_ids`, `attention_mask`, and `token_type_ids`,
and accepts either a pooled rank-2 `sentence_embedding` output or a legacy
rank-3 `last_hidden_state` output. Managed setup derives a CLS-pooled runtime
graph so normal runs avoid transferring the full token hidden state, then reuse
the existing normalization and stored-vector path. The `llamacpp` backend remains
an explicit legacy option for callers that manage their own OpenAI-compatible
embedding server, and the `hash` backend remains for deterministic local-dev
and CI checks. Current benchmark findings live in
[embedding-backend-benchmarks.md](../../testing/embedding-backend-benchmarks.md).

The CLI owns managed embedding setup. `codestory-cli setup embeddings` installs
pinned Qdrant BGE-base ONNX assets under the user cache and derives
`model_optimized_cls_pool.onnx` from the downloaded graph. It does not start a
server, write server logs, or leave a model process behind. When assets are
present, CLI runtime preparation sets the pooled ONNX model and tokenizer paths plus
the managed provider and throughput defaults unless the user already set
explicit environment values.

Timing fields for this path are in `IndexingPhaseTimings`: `search_projection_rebuild_ms`, `search_symbol_index_ms`, `runtime_cache_publish_ms`, `semantic_doc_build_ms`, `semantic_embedding_ms`, `semantic_db_upsert_ms`, `semantic_reload_ms`, `semantic_prune_ms`, `semantic_docs_reused`, `semantic_docs_embedded`, `semantic_docs_pending`, and `semantic_docs_stale`.

## Failure Signatures

- runtime regains direct persistence logic
- search engine internals become public API
- CLI formatting concerns start driving runtime behavior
- semantic docs become an implicit background side effect instead of an explicit index phase
