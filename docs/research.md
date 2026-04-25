# CodeStory Research Handbook

This is the human front door for CodeStory research. It keeps only the durable
decisions and points to the comparison matrix, not raw run ledgers.

## Current Decisions

| Area | Decision | Why it matters |
| --- | --- | --- |
| Real local embeddings | Use `CODESTORY_EMBED_BACKEND=llamacpp`. | This is the active real-model path. |
| Deterministic local checks | Use `CODESTORY_EMBED_RUNTIME_MODE=hash`. | Keeps local-dev and CI checks reproducible without model services. |
| Default model profile | `CODESTORY_EMBED_PROFILE=bge-base-en-v1.5`. | BGE-base remains the best quality/speed family for the active runtime. |
| Default doc shape | `CODESTORY_SEMANTIC_DOC_ALIAS_MODE=alias_variant`, durable semantic scope. | Compact aliases help retrieval without the noise of full alias text. |
| Best current local pipeline packet | BGE-base GGUF through llama.cpp/Vulkan, batch `512`, request count `6`, server batch `1024`, server microbatch `1024`, stored vectors `int8`. | This is the strongest current CodeStory-local packet after quality-gated scoring. |
| Prior cross-repo promoted profile | BGE-base GGUF, batch `768`, request count `4`, server microbatch `1024`, stored vectors `int8`. | This has external promotion evidence, but the newer local b512/r6 shape still needs its own cross-repo gate. |
| Evidence standard | Quality gates and rank profiles come before speed. | A faster row is rejected when MRR, Hit@10, rank1/rank2-10, or misses regress. |

## Research Threads

### Embedding And Pipeline Performance

Read [Embedding Pipeline Decision Matrix](testing/embedding-backend-benchmarks.md)
for the full comparison. It records the current winner, superseded candidates,
discarded lanes, and what still needs proof.

### Repo-Scale E2E Performance

Read [codestory-e2e-stats-log.md](testing/codestory-e2e-stats-log.md) for the
rolling index/search timing history. This is the release-style sanity check for
semantic indexing behavior and cache reuse.

### Product And UX Research

Read [project-delight-roadmap.md](project-delight-roadmap.md) for the product
direction around `ask`, explainable retrieval, navigation UX, MCP serving, setup
help, and implemented roadmap work.

### Architecture And Documentation Research

Read [decision-log.md](decision-log.md), [architecture overview](architecture/overview.md),
and [indexing pipeline](architecture/indexing-pipeline.md) for the current
architecture state. Historical ADR-style notes were collapsed into current
architecture docs because clear live-system explanations are more useful here.

## How To Continue Research

1. Start from the current decision table and the comparison matrix.
2. Add candidates to the existing benchmark harness instead of creating a new
   one-off script.
3. Keep query-sliced runs exploratory. Promotion needs full-query repeats,
   provider proof, and a clean rank profile.
4. Update the comparison matrix in the same change that adds or rejects a
   meaningful research lane.
5. Do not commit raw run ledgers, dashboard exports, or local artifact catalogs.
