# `index` - Build or Refresh the Symbol Index

Discovers project files, extracts symbols and edges, persists graph/search state
to SQLite, writes graph-native symbol docs and component reports, and
synchronizes selected dense anchors when embedding assets are available.

## Syntax

See [generated CLI syntax](generated-cli-syntax.md) for the current command usage.
Use `<codestory-cli> <command> --help` for the complete option set.

## Refresh Modes

| Mode | Behavior |
|------|----------|
| `auto` | Build the cache when it is empty, then use incremental refresh for existing caches. |
| `full` | Rebuild the project graph, symbol docs, component reports, and dense anchors from the discovered workspace. |
| `incremental` | Reindex changed/new/unindexed files, remove disappeared files, and prune touched symbol docs or dense anchors. |
| `none` | Inspect the existing cache without refreshing it. Use only after a known-good same-session index. |

Keep the default `auto` refresh for ordinary agent setup. It performs the needed
first build for an empty repository cache and incremental refreshes after that.
Use explicit `--refresh full` only for diagnosed cache/schema uncertainty,
historical indexing failures, moved roots, or user-requested rebuild evidence.
Incremental runs can leave stale error rows when previously failing files are
not touched.

## Symbol Docs And Dense Anchors

There is no `index --semantic off` flag. Graph-native `symbol_search_doc` rows
are part of the default index contract. Under `graph_first_v1`, dense vectors
are only written for selected anchors such as entrypoints, public APIs,
documented nontrivial symbols, central graph nodes, component reports, and
unstructured docs. Product packet/search readiness uses the embedded
CodeRankEmbed engine through its private per-user server.

High-signal environment toggles:

| Variable | Use |
|----------|-----|
| `CODESTORY_SEMANTIC_DOC_SCOPE=all` | Include the broader all-symbol symbol-doc scope for diagnostics. Accepted aliases are `all`, `full`, `all-symbols`, and `all_symbols`; omitted or other values default to durable symbols. |
| `CODESTORY_EMBED_ALLOW_CPU=1` | Explicitly allow CPU embeddings for hosted CI or a maintainer diagnostic. Production never falls back to CPU silently. |
| `CODESTORY_SUMMARY_ENDPOINT=local` | Enable deterministic local summaries with `--summarize`. |

The model, tokenizer, pooling, normalization, and batching contract is compiled
into the executable. There is no embedding endpoint or backend download to
configure.
Agent packet/search readiness requires `retrieval_mode=full`; see
[status-contract.md](status-contract.md) and [retrieval-rollout.md](retrieval-rollout.md).

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
<codestory-cli> index --project <target-workspace> --refresh incremental
<codestory-cli> index --project <target-workspace> --refresh full # explicit rebuild
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
