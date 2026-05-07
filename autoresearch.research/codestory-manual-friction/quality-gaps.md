# Quality Gaps: Eliminate user and AI friction in CodeStory skill-first repo explanation across Sourcetrail, rootandruntime, and CodeStory.

- [x] Semantic doc inputs are bounded by default so Sourcetrail full semantic indexing cannot send oversized llama.cpp embedding inputs.
- [x] `doctor` clearly distinguishes `semantic ok`, `semantic partial`, `semantic stale`, and `semantic failed`, with warnings surfaced before the full check list.
- [x] Broad repo explanation uses an explicit investigation path and does not drift into bundled/sample files when semantics are partial.
- [x] `trail --query` and query DSL `trail(...)` return consistent default graph width and target resolution.
- [x] `symbol --query` and query DSL `symbol(...)` resolve exact names consistently, with type/file relevance ahead of high-scoring incidental hits.
- [x] `snippet --context` output reports the requested context and effective truncation cap so larger context requests are not misread.
- [x] Non-trail commands reject `--format dot` before doing work, and output/docs do not imply DOT is broadly valid.
- [x] `.agents/skills/codestory-grounding` tells agents to start embeddings for semantic E2E, require complete semantic health before broad `ask`, and use lexical/repo-text fallback when semantics are partial.
- [x] Manual benchmark emits `METRIC quality_gap=<count>` and preserves command transcripts for Sourcetrail, rootandruntime, and CodeStory.
- [x] Final stop condition is proven by `quality_gap=0`, passing checks, complete explanation flow on all three repos, and two fresh consecutive clean rounds with no new P0/P1/P2 friction.
