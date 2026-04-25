# Embedding Pipeline Decision Matrix

This is the curated decision record for CodeStory embedding, indexing, and
retrieval performance work. It replaces the old run diaries and raw packet
ledgers with the evidence a reader needs to understand what was tried, what was
kept, what was rejected, and what still needs proof.

## Decision Summary

| Question | Current answer |
| --- | --- |
| Real local embedding backend | `CODESTORY_EMBED_BACKEND=llamacpp` |
| Deterministic local-dev backend | `CODESTORY_EMBED_RUNTIME_MODE=hash` |
| Default profile | `CODESTORY_EMBED_PROFILE=bge-base-en-v1.5` |
| Default doc shape | `CODESTORY_SEMANTIC_DOC_ALIAS_MODE=alias_variant`, durable semantic scope |
| Best current local packet | BGE-base GGUF through llama.cpp/Vulkan, batch `512`, request count `6`, server batch `1024`, server microbatch `1024`, stored vectors `int8` |
| Best previous cross-repo profile | BGE-base GGUF through llama.cpp/Vulkan, batch `768`, request count `4`, server microbatch `1024`, stored vectors `int8` |
| Primary metric shape | Quality-led pipeline score: quality is the largest term, speed is secondary, footprint is tertiary, and quality-gate penalties can reduce the final score |
| Default-change rule | Do not promote a faster row when MRR, Hit@10, rank profile, or cross-repo behavior regresses |

The newest local optimization loop makes the current local choice obvious:
`b512/r6/sb1024/ub1024` is the best measured CodeStory packet. The older
`b768/r4/ub1024` compact-storage profile still has cross-repo promotion evidence,
but it has not been rerun as the newest local winner shape. Treat it as
historical promotion evidence, not a reason to override the latest local result.

## Primary Comparison Matrix

| Candidate or lane | Best relevant evidence | Quality signal | Speed signal | Decision |
| --- | --- | --- | --- | --- |
| BGE-base llama.cpp, b512/r6, server batch 1024, microbatch 1024, stored int8 | `pipeline_score=873163.999266` | MRR@10 `0.982545045`, Hit@10 `1.0` | `409.58` docs/sec, index `18.779s`, semantic `10.833s` | Keep as the current local winner. |
| BGE-base llama.cpp, b512/r6, microbatch 1024 only | `pipeline_score=872606.758299` | MRR@10 `0.982545045`, Hit@10 `1.0` | `406.50` docs/sec, semantic `10.915s` | Kept but superseded by the server-batch shape. |
| BGE-base llama.cpp, b512/r5, microbatch 1024 | `pipeline_score=870971.265321` | MRR@10 `0.982545045`, Hit@10 `1.0` | `397.47` docs/sec, semantic `11.163s` | Kept as the path that led to r6, now superseded. |
| BGE-base llama.cpp, b512/r8, server batch 1024, microbatch 1024 | `pipeline_score=872037.623150` | MRR@10 `0.982545045`, Hit@10 `1.0` | `403.36` docs/sec, semantic `11.005s` | Discard. More concurrency did not beat r6. |
| BGE-base llama.cpp, b512/r7 or b768/r6 | r7 `870663.075971`, b768/r6 `871191.174582` | Quality held, but no score win | Both slower than the r6 winner | Discard. Adjacent batch/concurrency expansion is not the bottleneck. |
| Projection-only symbol index, full-text symbol index disabled | First packet `869621.479788`; repeat `865754.081701` | MRR@10 `0.983108108`, rank1 `144`, rank2-10 `4`, misses `0` | First packet `391.93` docs/sec, repeat `378.30` docs/sec | Keep as opt-in architecture evidence. It reduces nonsemantic overhead but is not the primary winner. |
| Continuous quality and explicit quality gates | Gate packets stayed finite and penalized fragile rows instead of reporting fake wins | Exposed MRR/rank regressions such as rank1 `142` vs `144` | Measurement only | Keep. Quality is part of the primary metric, not a side note. |
| Query-rank and denominator reporting | Rank profile exposed rank1/rank2-10/miss counts; semantic doc counts stopped denominator ambiguity | Measurement only | Measurement only | Keep. This is the evidence layer future changes need. |
| BGE-base Q5 active pipeline scout | `pipeline_score=843988.193781` | Quality gate failed: MRR@10 `0.975788288`, rank1 `142`, rank2-10 `6` | `351.87` docs/sec, index `22.435s`; footprint improved | Discard for the active pipeline. Smaller artifact did not justify the quality loss. |
| BGE-small GGUF scouts | `353357.245456` for all-scope, `516361.429775` for durable | Quality gate failed despite good speed/footprint | Durable scout reached `574.33` docs/sec | Discard for this pipeline. It is fast but not good enough. |
| Nomic and EmbeddingGemma model-family scouts | Nomic `587079.276176`; Gemma `706729.502599` | Both below the quality bar | Slower or lower-ranked than BGE-base | Pause unless the semantic-doc/query hypothesis changes. |
| Token-budget narrowing and semantic-scope pruning | tok320 `863128.088009`; no-macros `838267.207245`; no-test-macros `862424.110651` | Lost quality or rank profile | Some rows reduced semantic time but not enough | Discard. Removing documents is not free. |
| Direct hydration, file-path cache, and reload skipping | Direct hydration `843840.934147`; stable-order hydration `859167.697841`; file-path cache `862471.707786` | Quality or speed regressed | Did not remove enough cache/semantic time | Discard for now. |
| Background semantic embedding | Buggy packet `949630.846470`; corrected packet `857789.768509` | Corrected packet had lower quality | Corrected speed did not beat the winner | Discard. The apparent win was a measurement bug. |
| ONNX rows | Older ONNX rows are historical only | Some older rows were competitive | Active runtime no longer carries ONNX | Do not reopen unless a new backend decision deliberately reintroduces it. |

## What Was Tried

The measured work covered these families:

- llama.cpp request geometry: batch size, request count, server batch, and server microbatch.
- Stored-vector footprint: compact scaled int8 persisted vectors.
- Quality metric repair: explicit gates, continuous penalties, denominator metrics, and query-rank reporting.
- Semantic document shape: token budgets, macro/test-macro pruning, callable/type scopes, and durable/all scope variants.
- Streaming and overlap scouts: semantic-doc streaming batches, sort windows, background embedding, and search-index overlap.
- Cache and reload reduction: direct hydration, stable ordering, file-path caching, semantic reload skipping, and cache refresh split reporting.
- Model family scouts: BGE-base, BGE-small, Nomic, EmbeddingGemma, Qwen-derived dimension work, and GGUF weight quantization.
- Cross-repo promotion: the previous compact BGE-base b768/r4 microbatch profile passed external repositories, but the newest b512/r6 local winner still needs its own cross-repo gate before becoming a broader recommendation.

## What Was Not Proven

- True producer-consumer streaming from parsed/indexed symbols into the embedder
  has not been proven. Several scouts reduced or rearranged semantic work, but
  none established a safe end-to-end streaming architecture that beats the
  current b512/r6 winner.
- The newest local winner has not yet been promoted through the external
  cross-repo gate.
- Q5 remains a compression option only if a future quality-preserving recipe
  exists. The active Q5 packet failed the quality gate.
- ONNX is not an active path. Older ONNX evidence should not drive new choices
  after the backend was removed.

## How To Use This Matrix

Use this file as the first stop for embedding and pipeline decisions. If a new
candidate is added, update the matrix with the candidate shape, the best
decision-grade metric row, the quality/rank signal, the speed signal, and the
decision. Do not add raw run ledgers or diary files to the repo.
