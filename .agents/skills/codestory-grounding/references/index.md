# `index` - Build or Refresh the Symbol Index

Discovers project files, extracts symbols and edges, persists graph/search state
to SQLite, writes graph-native symbol docs and component reports, and
synchronizes selected dense anchors when embedding assets are available.

## Usage

```text
<codestory-cli> index [OPTIONS]
```

## Options

| Option | Default | Use |
|--------|---------|-----|
| `--project <path>` / `--path <path>` | `.` | Target repository root. Always pass this explicitly. |
| `--cache-dir <path>` | auto | Override the per-project cache root. |
| `--refresh <auto|full|incremental|none>` | `auto` | Choose the graph/snapshot/symbol-doc/dense-anchor refresh mode. |
| `--format <markdown|json>` | `markdown` | Use JSON for automation and timing analysis. |
| `--output-file <path>` | stdout | Write output to a file with an existing parent directory. |
| `--dry-run` | off | Show workspace discovery and planned adds/removals without writing storage. |
| `--summarize` | off | Generate cached symbol summaries; requires `CODESTORY_SUMMARY_ENDPOINT`, `local`, or `mock`. |
| `--progress` | off | Print progress to stderr while preserving stdout output. |
| `--watch` | off | Keep watching the project root and run incremental refreshes on changes. |

## Refresh Modes

| Mode | Behavior |
|------|----------|
| `auto` | Use `full` for an empty cache and `incremental` otherwise. |
| `full` | Rebuild the project graph, symbol docs, component reports, and dense anchors from the discovered workspace. |
| `incremental` | Reindex changed/new/unindexed files, remove disappeared files, and prune touched symbol docs or dense anchors. |
| `none` | Inspect the existing cache without refreshing it. Use only after a known-good same-session index. |

Use `--refresh full` for first-time indexes, cache/schema uncertainty, and fixes
for historical indexing failures. Incremental runs can leave stale error rows
when previously failing files are not touched.

## Symbol Docs And Dense Anchors

There is no `index --semantic off` flag. Graph-native `symbol_search_doc` rows
are part of the default index contract. Under `graph_first_v1`, dense vectors
are only written for selected anchors such as entrypoints, public APIs,
documented nontrivial symbols, central graph nodes, component reports, and
unstructured docs. On a fresh machine, check the setup plan first:

```text
<codestory-cli> setup embeddings --project <target-workspace> --dry-run --format json
```

Then install assets with `setup embeddings --project <target-workspace>` if the
plan is acceptable, and rerun `index --refresh full`.

High-signal environment toggles:

| Variable | Use |
|----------|-----|
| `CODESTORY_SEMANTIC_DOC_SCOPE=all` | Include the broader all-symbol symbol-doc scope for diagnostics. Accepted aliases are `all`, `full`, `all-symbols`, and `all_symbols`; omitted or other values default to durable symbols. |
| `CODESTORY_EMBED_BACKEND=llamacpp` | Use the mandatory local llama.cpp embedding sidecar. |
| `CODESTORY_EMBED_LLAMACPP_URL=http://127.0.0.1:8080/v1/embeddings` | Product embedding endpoint for bge-base sidecar vectors. |
| `CODESTORY_SUMMARY_ENDPOINT=local` | Enable deterministic local summaries with `--summarize`. |

Use other embedding, alias, batch-size, tokenizer, provider, hash, ONNX, and
summary tuning variables only for focused diagnostics or historical comparisons.
Agent packet/search readiness requires retrieval status to report
`retrieval_mode=full`. A zero dense-anchor corpus is valid only when the
manifest reports it explicitly; otherwise stale or unavailable Qdrant state
fails closed.

## Output

Markdown returns a compact index summary. JSON exposes the same data for tools:

- project and storage path
- refresh mode and discovered file/error counts
- local navigation readiness notes, symbol-doc counts, dense-anchor counts, and policy reason counts
- parse, flush, resolve, cleanup, cache, and semantic timing buckets
- resolution counters plus symbol-doc write and dense-anchor reuse/embed/skip/prune counts

Important timing fields are `timings_ms.parse`, `timings_ms.flush`,
`timings_ms.resolve`, `timings_ms.cleanup`, `cache_ms.search_index`,
`cache_ms.runtime_publish`, `semantic_ms.doc_build`,
`semantic_ms.embedding`, `semantic_ms.db_upsert`, and `semantic_ms.prune`.

## Examples

```text
<codestory-cli> index --project <target-workspace>
<codestory-cli> index --project <target-workspace> --refresh full
<codestory-cli> index --project <target-workspace> --dry-run
<codestory-cli> index --project <target-workspace> --watch --progress
CODESTORY_SUMMARY_ENDPOINT=local <codestory-cli> index --project <target-workspace> --summarize
```

## Endpoint Awareness

When OpenAPI JSON/YAML files are indexed, CodeStory emits endpoint symbols such
as `GET /api/users`. Client literals like `fetch("/api/users")` and
`axios.post("/api/users")` can create speculative call edges to matching
endpoint refs, so confirm certainty before treating frontend/backend trails as
verified.
