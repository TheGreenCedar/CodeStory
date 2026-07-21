# CodeStory Research Handbook

This page summarizes CodeStory research decisions. It keeps only the durable
decisions and points to the comparison matrix, not raw run output.

## Current Decisions

| Area | Decision | Why it matters |
| --- | --- | --- |
| Real local embeddings | Use the linked CodeRankEmbed Q8 engine through one private per-user server. | Product packet/search evidence requires exact server and embedded-model producer identity plus `retrieval_mode=full`; CPU is explicit-only. |
| Default doc shape | Graph-native `symbol_search_doc` for durable symbols plus `CODESTORY_SEMANTIC_DOC_ALIAS_MODE=alias_variant` for selected dense anchors. | Code recall is AST-first; compact aliases help the dense-anchor subset without returning to an all-code vector corpus. |
| Dense policy | `graph_first_v2` with reasons `public_api`, `entrypoint`, `documented_nontrivial`, `central_graph_node`, `component_report`, and `unstructured_doc`. | Dense vectors are reserved for structurally justified anchors; centrality uses complete bounded relationship counts while private trivial code stays discoverable through symbol docs and graph/lexical recall. |
| Current model decision | CodeRankEmbed Q8 replaces BGE after the #1164 same-machine Metal study: +36% dense-only MRR@10 and +55% Hit@1, with an accepted throughput and memory cost. | Quality is the primary product gate. Release evidence must now establish the CodeRank producer identity and new machine baseline. |
| Peak memory evidence | Segment-2 q8/r6 baseline measured peak descendant working set `828.726562 MB`; repeat sampled `1019.789062 MB`; `peak_vram_mb` was unavailable on this host. | Memory is now measured explicitly, but sampled peak RAM is noisy enough that tiny memory wins need repeats. |
| Evidence standard | Quality gates and rank profiles come before speed. | Performance and memory remain explicit tradeoffs, but they do not erase a material, repeatable retrieval-quality gain. |

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

Read [User guides](users/README.md) and [architecture overview](architecture/overview.md)
for current operator workflows and navigation surfaces. Treat roadmap notes as
direction, not benchmark proof or a changelog.

### Architecture And Documentation Research

Read [architecture overview](architecture/overview.md) and
[indexing pipeline](architecture/indexing-pipeline.md) for the current
architecture state.

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
