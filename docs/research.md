# CodeStory Research Handbook

This page summarizes CodeStory research decisions. It keeps only the durable
decisions and points to the comparison matrix, not raw run output.

## Current Decisions

| Area | Decision | Why it matters |
| --- | --- | --- |
| Real local embeddings | Use `CODESTORY_EMBED_BACKEND=llamacpp` with the local llama.cpp sidecar. | Product packet/search evidence now requires the sidecar manifest to record the 768-d bge-base backend and `retrieval_mode=full`. |
| Deterministic diagnostics | `CODESTORY_EMBED_RUNTIME_MODE=hash` is diagnostic-only. | Keeps selected local-dev and CI checks reproducible without model services, but is not agent-facing retrieval evidence. |
| Default model profile | `CODESTORY_EMBED_PROFILE=bge-base-en-v1.5`. | BGE-base remains the best quality/speed family for the active runtime. |
| Default doc shape | `CODESTORY_SEMANTIC_DOC_ALIAS_MODE=alias_variant`, durable semantic scope. | Compact aliases help retrieval without the noise of full alias text. |
| Current benchmark baseline | Historical BGE-base Q8 GGUF through llama.cpp/Vulkan remains the last fully scored broad-holdout baseline; the active mandatory sidecar contract needs a fresh coherent benchmark row. | Do not compare new sidecar speed numbers against old mixed-vintage rows without rerunning the quality and cross-repo gates. |
| Peak memory evidence | Segment-2 q8/r6 baseline measured peak descendant working set `828.726562 MB`; repeat sampled `1019.789062 MB`; `peak_vram_mb` was unavailable on this host. | Memory is now measured explicitly, but sampled peak RAM is noisy enough that tiny memory wins need repeats. |
| Evidence standard | Quality gates and rank profiles come before speed. | A faster row is rejected when MRR, Hit@10, rank1/rank2-10, or misses regress. |

## Research Threads

### Embedding And Pipeline Performance

Read [Embedding Pipeline Decision Matrix](testing/embedding-backend-benchmarks.md)
for the full comparison. It records the current incumbent candidate, historical
rows, discarded lanes, and what still needs proof.

### Repo-Scale E2E Performance

Read [codestory-e2e-stats-log.md](testing/codestory-e2e-stats-log.md) for the
rolling index/search timing history. This is the release-style sanity check for
semantic indexing behavior and cache reuse.

### Product Direction

Read [project-delight-roadmap.md](project-delight-roadmap.md) for current product
direction around target context packets, explainable retrieval, navigation UX,
serving, and setup help. Treat it as direction, not benchmark proof or a
changelog.

### Architecture And Documentation Research

Read [decision-log.md](decision-log.md), [architecture overview](architecture/overview.md),
and [indexing pipeline](architecture/indexing-pipeline.md) for the current
architecture state. Historical ADR-style notes were collapsed into current
architecture docs because clear live-system explanations are more useful here.

## How To Continue Research

1. Start from the current decision table and the comparison matrix.
2. Add candidates to the existing benchmark harness instead of creating a new
   one-off script.
3. Keep query-sliced runs exploratory. Promotion needs clean holdout evidence,
   leakage checks, cache replay blocking, cross-repo proof when defaults might
   change, and a clean rank profile.
4. Update the comparison matrix in the same change that adds or rejects a
   meaningful research lane; do not let a single first-pass score outrank a
   failed repeat.
5. Do not commit raw run transcripts, dashboard exports, or local artifact
   catalogs.
