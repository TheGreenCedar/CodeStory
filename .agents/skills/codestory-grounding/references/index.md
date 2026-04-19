# `index` — Build or Refresh the Symbol Index

Discovers project files, extracts symbols and edges via tree-sitter + semantic resolution, persists graph/search state to SQLite, and synchronizes semantic docs before returning when embedding assets are available.

## Usage

```
target/release/codestory-cli(.exe) index [OPTIONS]
```

## Arguments

| Argument | Type | Default | How it works |
|----------|------|---------|--------------|
| `--project` | path | `.` | Repository root to index or query. `--path` is an alias. Runtime opens this path, discovers files from the workspace configuration, and derives the default project-cache key from the resolved root. |
| `--cache-dir` | path | *auto* | Cache directory to use exactly as passed. If omitted, `codestory-cli` uses the system cache root with a per-project hashed subdirectory. The SQLite DB and sibling persisted search directory live under this cache root. |
| `--refresh` | enum | `auto` | Refresh strategy: `auto`, `full`, `incremental`, or `none`. This decides whether graph/snapshot/semantic indexing runs before the summary is returned. |
| `--format` | enum | `markdown` | Output format: `markdown` or `json`. JSON exposes the same project stats, retrieval state, phase timings, semantic counters, and resolution counters for automation. |

## Refresh Modes

| Mode | Behavior |
|------|----------|
| `auto` | Inspect stored inventory. If the cache has no indexed files, run `full`; otherwise run `incremental`. This is the default for `index`. |
| `full` | Build a staged SQLite database from the full workspace, parse/extract/resolve every discovered source file, copy reusable semantic docs forward from the previous live DB when present, finalize snapshots, publish the staged DB, and sync semantic docs before returning. |
| `incremental` | Open the live DB, diff filesystem discovery against stored inventory, reindex changed/new/unindexed files, remove disappeared files, refresh live snapshots, and rebuild/prune semantic docs only for touched files. |
| `none` | Open the existing cache and return a summary without running graph or semantic indexing. Use only when you intentionally want to inspect a known-good cache. |

## Semantic Behavior

There is no `index --semantic off` option in the current CLI. Semantic docs are part of the default `index` contract when embedding assets are available.

Runtime environment variables control semantic retrieval and tuning:

| Variable | Behavior |
|----------|----------|
| `CODESTORY_HYBRID_RETRIEVAL_ENABLED=false` | Disable hybrid retrieval and use symbolic ranking. |
| `CODESTORY_SEMANTIC_DOC_SCOPE=all` | Include the broader all-symbol semantic doc set. The default is durable symbols only. |
| `CODESTORY_LLM_DOC_EMBED_BATCH_SIZE` | Override semantic doc embedding batch size. Default is `128`; use this only while profiling. |
| `CODESTORY_EMBED_RUNTIME_MODE=hash` | Use lightweight deterministic hash embeddings for local-dev semantic checks. |
| `CODESTORY_EMBED_MODEL_PATH` | Path to the ONNX embedding model artifact. |
| `CODESTORY_EMBED_TOKENIZER_PATH` | Path to `tokenizer.json`; defaults to a sibling of the model artifact. |
| `CODESTORY_EMBED_SESSION_COUNT` | Number of ONNX embedding sessions, clamped from `1` to `16`; default is bounded by available parallelism and capped at two. |
| `CODESTORY_EMBED_INTRA_THREADS`, `CODESTORY_EMBED_INTER_THREADS`, `CODESTORY_EMBED_PARALLEL_EXECUTION` | ONNX CPU-provider tuning; do not use CPU-provider rows for benchmark decisions. |
| `CODESTORY_EMBED_EXECUTION_PROVIDER` | `cpu`, `cuda`, or `directml`; CUDA and DirectML require the matching Cargo feature. |

## Output

Returns project stats, retrieval readiness, and phase timings. Markdown output is compact; JSON output exposes the same fields as structured data.

```
# Index
project: `codestory`
storage: `/path/to/codestory.db`
refresh: `auto(incremental)`
stats: nodes=4231 edges=8452 files=187 errors=3
retrieval: hybrid semantic_docs=3690 model=sentence-transformers/all-MiniLM-L6-v2-local
timings_ms: parse=1200 flush=300 resolve=450 cleanup=80 cache_refresh=0
semantic_ms: doc_build=115 embedding=32634 db_upsert=420 reload=18
semantic_docs: reused=0 embedded=3690 pending=3690 stale=0
resolution: calls 120->15, imports 42->3
```

Important timing fields:

| Field | Meaning |
|-------|---------|
| `timings_ms.parse` | Parse, extract, artifact-cache, and indexer preparation time. |
| `timings_ms.flush` | Projection persistence time for graph rows and derived projection rows. |
| `timings_ms.resolve` | Post-flush edge resolution time. |
| `timings_ms.cleanup` | Incremental cleanup for removed/stale files. |
| `timings_ms.cache_refresh` | Runtime cache refresh and semantic sync wrapper time. |
| `semantic_ms.doc_build` | Generated semantic text and hash construction. |
| `semantic_ms.embedding` | Embedding runtime work for pending semantic docs. |
| `semantic_ms.db_upsert` | SQLite upsert time for embedded docs. |
| `semantic_ms.reload` | Loading persisted semantic docs into the runtime search engine when needed. |
| `semantic_docs.reused` | Existing docs accepted without embedding. |
| `semantic_docs.embedded` | Docs newly embedded in this run. |
| `semantic_docs.pending` | Docs that required embedding after reuse checks. |
| `semantic_docs.stale` | Persisted docs pruned because they no longer match the refreshed symbol set. |

## Examples

```bash
# First-time index of the current repo
target/release/codestory-cli(.exe) index --project .

# Force full re-index
target/release/codestory-cli(.exe) index --project . --refresh full

# Index a different project, JSON output
target/release/codestory-cli(.exe) index --project ../other-repo --format json
```

## Refresh Troubleshooting

| Situation | Recommended refresh |
|----------|----------------------|
| First-time indexing or after cache deletion | `--refresh full` |
| Verifying a fix for prior indexing errors | `--refresh full` |
| Verifying schema/storage-version or graph/query-rule changes | `--refresh full` |
| Normal follow-up indexing after editing a few files | `--refresh incremental` or `auto` |
| Reusing a known-good index immediately after a successful fresh build + index run | `--refresh none` |

Prefer `--refresh full` when you need confidence that historical errors are gone. Incremental runs can leave stale error rows behind if the previously failing files are not reprocessed.
