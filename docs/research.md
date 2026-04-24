# CodeStory Research Handbook

This is the human front door for CodeStory research. Use it before opening the
large benchmark sheets or raw artifact trees.

The short version: active embedding support is now llama.cpp for real local
models and `hash` for deterministic local-dev checks. ONNX benchmark evidence
is preserved as historical provenance, but ONNX is no longer a runtime or
research-harness backend. The strongest measured local pipeline remains BGE-base
through llama.cpp/Vulkan with scaled int8 persisted vectors.

## Current Decisions

| Area | Decision | Why it matters |
| --- | --- | --- |
| Runtime backend | Use `CODESTORY_EMBED_BACKEND=llamacpp` for real local embeddings, or `CODESTORY_EMBED_RUNTIME_MODE=hash` for deterministic local-dev/CI checks. | ONNX had no remaining product benefit and added dependency, provider, artifact, and docs weight. |
| Default profile | `CODESTORY_EMBED_PROFILE=bge-base-en-v1.5`, `CODESTORY_SEMANTIC_DOC_ALIAS_MODE=alias_variant`, durable scope, batch `128`. | This keeps the runtime aligned with the promoted BGE-base evidence while still requiring an explicit llama.cpp endpoint for real embeddings. |
| Best quality/speed/footprint profile | BGE-base GGUF through llama.cpp/Vulkan, `alias_variant`, durable scope, batch `768`, request count `4`, server `-np 4`, `-ub 1024`, cls pooling, with `CODESTORY_STORED_VECTOR_ENCODING=int8`. | The 60/30/10 pipeline loop found the best local score here and the cross-repo gate passed with no misses. |
| BGE-small status | Keep BGE-small rows as historical evidence only. | The strongest BGE-small result was an ONNX row, and ONNX is retired; current GGUF BGE-small rows did not beat the BGE-base llama.cpp path. |
| Best compressed BGE-base artifact | Clean-source Q5_K_M GGUF under the BGE-base b512/r4 leader shape. | It preserved ranking quality with a smaller model artifact, but throughput and score stayed below Q8. It is a compression option, not the leader. |
| Negative lanes to pause | Broad hybrid-weight sweeps, dimension-only loops, retired ONNX lanes, BGE-small GGUF lower-bit rows, Nomic v2 under current doc shape. | These consumed enough evidence to stop repeating them until the semantic-doc, query, hardware, or model hypothesis changes. |
| Evidence standard | Provider-verified rows plus per-query ranks and repeated finalists decide recommendations. | A single row, CPU fallback, or missing GPU/provider proof is provenance only. |

## Research Threads

### Embedding and Retrieval Backend Research

Read [embedding-backend-benchmarks.md](testing/embedding-backend-benchmarks.md)
for the full decision sheet. It consolidates model/backend choices, alias-mode
results, Run 2 controls/retrieval/finalists, quantization lanes, the 60/30/10
pipeline loop, and the remaining backlog.

Use [embedding-research-run-2.md](testing/embedding-research-run-2.md) when you
need the harness contract: source scan, stage names, scoring, stop rules, and
how to run bounded slices without confusing exploratory evidence with promotion
evidence.

Use [research-data-catalog.md](testing/research-data-catalog.md) when you need
to find raw CSV/JSON/log artifacts or preserve the local evidence tree.

### Repo-Scale E2E Performance

Read [codestory-e2e-stats-log.md](testing/codestory-e2e-stats-log.md) for the
rolling index/search timing history. This is the release-style sanity check for
semantic indexing behavior and cache reuse, not a replacement for the raw
benchmark runs.

### Product and UX Research

Read [project-delight-roadmap.md](project-delight-roadmap.md) for the external
research-backed product direction: `ask`, explainable retrieval, navigation UX,
MCP serving, setup help, and the implemented snapshot of those ideas.

### Architecture and Documentation Research

Read [decision-log.md](decision-log.md), [architecture overview](architecture/overview.md),
and [indexing pipeline](architecture/indexing-pipeline.md) for the current
architecture state. The old ADR-style layer was intentionally collapsed into
current architecture docs because thin historical records were less useful than
clear explanations of the live pipeline.

## Data Custody

Tracked docs keep the human-readable synthesis. Raw data stays local because it
includes large generated caches, logs, search indexes, and model artifacts.

Important local evidence roots:

| Root | What it contains |
| --- | --- |
| `target/embedding-research/` | GPU fair benchmark runs, Run 2 controls/retrieval/finalists, quantization probes, per-query ranks, manifests, and per-case logs. |
| `target/autoresearch/indexer-embedder/` | The later autoresearch loop around the 60/30/10 pipeline score, compact stored vectors, cache/scoring experiments, and local promotion candidates. |
| `target/autoresearch/cross-repo-promotion/` | External promotion gates over freelancer, traderotate, the-green-cedar, Sourcetrail, and focused follow-up probes. |
| `models/` | Local GGUF model artifacts used by active benchmark rows, plus any historical artifacts that have not yet been cleaned. This directory is ignored by git. |

Those roots are not committed. If this checkout is moved or cleaned, archive
them first. The catalog records the shape and most important paths, but it is
not a substitute for the raw files.

## How To Continue Research

1. Start from the current decision table above and the benchmark backlog.
2. Extend `scripts/embedding-gpu-fair-benchmark.mjs` or the existing
   autoresearch scripts instead of creating a parallel harness.
3. Write `manifest.json`, `sources.md`, `results.csv`, `results.json`,
   `query-ranks.csv`, and logs for every run that might matter later.
4. Treat query-sliced runs as exploratory. Promotion needs full-query repeats
   and provider proof.
5. Update the tracked synthesis and the data catalog in the same change that
   introduces a new research lane or accepted decision.
