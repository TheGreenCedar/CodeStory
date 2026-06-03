# Embedding Pipeline Decision Matrix

This is the curated decision record for CodeStory embedding, indexing, and
retrieval performance work. It keeps the evidence needed to understand what was
tried, what was rejected, and what still needs proof.

## Decision Summary

| Question | Current answer |
| --- | --- |
| Real local embedding backend | `CODESTORY_EMBED_BACKEND=llamacpp` through the mandatory local sidecar |
| Deterministic diagnostic backend | `CODESTORY_EMBED_RUNTIME_MODE=hash` |
| Default profile | `CODESTORY_EMBED_PROFILE=bge-base-en-v1.5` |
| Default doc shape | `CODESTORY_SEMANTIC_DOC_ALIAS_MODE=alias_variant`, durable semantic scope |
| Current broad-holdout incumbent candidate | Pending fresh mandatory-sidecar quality row. Historical incumbent: BGE-base Q8 GGUF through llama.cpp/Vulkan, batch `512`, request count `6`, server batch `1024`, server microbatch `1024`, stored vectors `int8`, full-text enabled |
| Cross-repo gate status | Historical q8/r6 full-text profile passed the external gate across 4 projects and 225 queries; the current sidecar contract needs the same fresh gate before benchmark promotion |
| Primary metric shape | `pipeline_score = 1000000 * (0.7 * quality + 0.2 * speed + 0.1 * memory) * quality_gate_penalty` |
| Memory component shape | Model footprint, persisted vector footprint, and cache/index footprint; peak RAM is reported separately when sampled |
| Default-change rule | Do not promote a faster row when MRR, Hit@10, rank profile, repeat behavior, or cross-repo behavior regresses |
| Perfect-score rule | Treat perfect retrieval scores as unpromoted until split isolation, leakage checks, tainted-query exclusion, cache replay blocking, adversarial buckets, and a fresh confirmation run all agree |

The corrected 2026-04-28 loop excludes historically tainted query text and
requires `promotion_eligible=true`. After broadening to 74 clean holdout queries,
the useful local scores are no longer perfect: Q8/r6 full-text holds
`MRR@10=0.9824324324324325`, `Hit@10=1`, and
`Hit@1=0.972972972972973`. Segment 2 restarted the experiment log with this
profile as the fresh baseline: first measured baseline
`pipeline_score=909369.1102743357`; repeat `909844.2157260955`. That repeat
also showed peak-RAM sampling variance, so tiny memory deltas need repeat
evidence before they matter. Treat earlier perfect-score r5 evidence and any
single-pass q5 score as historical/suspect, not current default proof.

As of the mandatory sidecar reset, older ONNX and hash-projection rows are
historical diagnostics. Add a fresh sidecar row before calling the active runtime
promoted on quality and cross-repo evidence.

## Primary Comparison Matrix

| Candidate or lane | Best relevant evidence | Quality signal | Speed and footprint signal | Decision |
| --- | --- | --- | --- | --- |
| Managed BGE-base ONNX Runtime, CLS-pooled graph, DirectML, doc batch 2048, token budget 32768, stored int8 | Large C++ workspace fresh-cache timing on 26,010 semantic docs after batch-fast tokenizer switch and pooled-output graph derivation | Quality gate not run yet; direct CPU ORT check showed pooled graph output exactly matched source `last_hidden_state[:, 0, :]` on sampled inputs; semantic contract and search smoke passed | `semantic_embedding_ms=128.438s`; previous 32k unpooled row was `135.762s`; pooled 65k was slower at `131.807s`; prior managed ONNX 65k first pass was `152.500s`, batch-fast 65k was `138.118s`, 16k was `137.776s`; unpooled 131k was aborted as slower and memory-heavy after sampled peak working set around `3.37 GB` | Historical diagnostic lane after mandatory-sidecar reset. Do not treat as promoted product evidence without a fresh sidecar contract and quality gate. |
| BGE-base llama.cpp, b512/Q8/r6, server batch 1024, microbatch 1024, stored int8, full-text enabled | Segment 2 baseline `909369.110274`; repeat `909844.215726`; earlier fixed-wrapper holdout `910504.353332`; cross-repo `851670.370370` | Local MRR@10 `0.982432`, Hit@10 `1.0`, Hit@1 `0.972973`; cross-repo Hit@10 `1.0`, adversarial Hit@10 `1.0`, MRR@10 `0.826831` across 225 queries | Segment 2 baseline `368.01` docs/sec, cache `74.40 MB`, sampled peak descendant working set `828.73 MB`; repeat `371.89` docs/sec, sampled peak `1019.79 MB`; cross-repo search p95 `84.7 ms` | Historical externally validated baseline and closest prior evidence to the current mandatory sidecar backend. Re-run the same gates under the generation-bound sidecar contract before promotion. |
| BGE-base llama.cpp, b512/r5, microbatch 1024, stored int8 | Earlier local `pipeline_score=918957.022351`; confirmation `918697.617312`; corrected segment-2 scout `901789.644032` | Earlier perfect local scores triggered the overfit review; corrected segment-2 quality matched q8/r6, but did not improve it | Corrected segment-2 r5 slowed to `327.58` docs/sec and sampled peak rose to `1074.43 MB` | Historical/discarded. Do not treat r5 as the current promoted answer after the corrected broad-holdout pass. |
| BGE-base llama.cpp, b768/r4 vs b512/Q8/r6 on the 74-query broad holdout | Packet 18 selected Q8/r6 with `pipeline_score=910173.164803` | r4 matched Q8/r6 quality | r4 was slower than Q8/r6 (`361.85` vs `389.22` docs/sec) | Do not promote. Useful comparison only. |
| BGE-base Q5 against the broad holdout incumbent | Segment 2 first pass `910704.594864`; repeat `901823.678946` | First pass matched q8/r6 quality; repeat failed quality with MRR@10 `0.975676` and Hit@1 `0.959459` | Model footprint shrank to `78.21 MB`, but speed regressed and repeat stayed below baseline | Discard as a default/promotion candidate. Reopen only with a quality-preserving and speed-neutral compression recipe. |
| Full-text symbol index disabled | Segment 2 first pass `909873.095714`; repeat `908555.984747` | Quality matched q8/r6 on both passes | Cache and sampled RAM improved, but the first-pass score advantage did not repeat and the repeat fell below baseline | Discard as a default/promotion candidate for now. Keep only as an architecture scout. |
| Semantic-doc alias modes against q8/r6 | Segment 2 isolated `current_alias=890267.797771`; isolated `no_alias=843630.908137` | Both failed the quality gate; `current_alias` MRR@10 `0.973423`, `no_alias` MRR@10 `0.936937` and Hit@10 `0.986486` | `no_alias` was faster, but quality loss dominated; `current_alias` was slower and larger | Keep `alias_variant` as the default semantic-doc shape. Stop broad alias-mode toggles unless miss analysis identifies a specific doc feature. |
| Continuous quality and explicit quality gates | Gate packets stayed finite and penalized fragile rows instead of reporting fake wins | Exposed MRR/rank regressions and removed fake-perfect conclusions | Measurement only | Keep. Quality is part of the primary metric, not a side note. |
| Leakage guard, tainted-query quarantine, and cache replay block | `semantic-doc-leakage-check` passed before the current benchmark packets | Prevents production semantic docs from copying benchmark query text | Measurement guard only | Required before promotion evidence. |
| Query-rank, denominator, cache footprint, and peak-RAM reporting | Rank profile, semantic doc counts, `best_cache_dir_size_mb`, and sampled peak working set are now reported | Measurement only | Measurement only | Keep. This is the evidence layer future changes need. |
| Parallel semantic score computation for large semantic indexes | Run 26 `pipeline_score=898343.384426` | Quality held: MRR@10 `0.982432`, Hit@10 `1.0`, Hit@1 `0.972973` | Speed and RAM regressed: `362.61` docs/sec, peak descendant working set `1072.92 MB` | Discard. Broad Rayon parallelism hurt the user-weighted metric; future search-latency work needs a more targeted search-only hypothesis. |
| Top-k semantic score collection | First pass `912943.380576`; repeat `897131.467693` | Repeat failed quality: MRR@10 `0.970608`, Hit@1 `0.959459`, quality gate failed | Repeat stayed below the incumbent and kept high sampled RAM (`1058.27 MB`) | Discard and revert. Do not revisit without an exact-equivalence regression test against the old full-score selection path. |
| BGE-small GGUF scouts | Segment 2 q8 distant scout `790489.007403`; older `353357.245456` for all-scope, `516361.429775` for durable | Segment 2 MRR@10 `0.901351`, Hit@1 `0.824324`, quality gate failed despite Hit@10 `1.0` | Smaller model footprint did not overcome lower quality and end-to-end shape changes | Discard for this pipeline. It is fast-ish in spots but not good enough. |
| Nomic and EmbeddingGemma model-family scouts | Nomic `587079.276176`; Gemma `706729.502599` | Both below the quality bar | Slower or lower-ranked than BGE-base | Pause unless the semantic-doc/query hypothesis changes. |
| Token-budget narrowing and semantic-scope pruning | tok320 `863128.088009`; no-macros `838267.207245`; no-test-macros `862424.110651` | Lost quality or rank profile | Some rows reduced semantic time but not enough | Discard. Removing documents is not free. |
| Direct hydration, file-path cache, and reload skipping | Direct hydration `843840.934147`; stable-order hydration `859167.697841`; file-path cache `862471.707786` | Quality or speed regressed | Did not remove enough cache/semantic time | Discard for now. |
| Background semantic embedding | Buggy packet `949630.846470`; corrected packet `857789.768509` | Corrected packet had lower quality | Corrected speed did not beat the incumbent candidate | Discard. The apparent win was a measurement bug. |
| Older ONNX rows | Historical only | Older ONNX rows predate the active replacement | Active runtime now carries ONNX directly | Do not use old ONNX evidence for new default decisions. Re-run speed, quality, and cross-repo gates on the active implementation before declaring a benchmark promotion. |

## What Was Tried

The measured work covered these families:

- ONNX Runtime provider and batch geometry, plus external llama.cpp request geometry for historical comparison.
- Stored-vector footprint: compact scaled int8 persisted vectors.
- Quality metric repair: explicit gates, continuous penalties, denominator metrics, and query-rank reporting.
- Benchmark isolation: leakage guard, tainted-query quarantine, and cache replay blocking.
- Semantic document shape: token budgets, macro/test-macro pruning, callable/type scopes, durable/all scope variants, and alias variants.
- Streaming and overlap scouts: semantic-doc streaming batches, sort windows, background embedding, and search-index overlap.
- Cache and reload reduction: direct hydration, stable ordering, file-path caching, semantic reload skipping, cache refresh split reporting, and full-text disablement.
- Model family scouts: BGE-base, BGE-small, Nomic, EmbeddingGemma, Qwen-derived dimension work, and GGUF weight quantization.
- Cross-repo promotion: the q8/r6 full-text profile passed the external gate across 4 projects and 225 queries with Hit@10 `1.0`, adversarial Hit@10 `1.0`, and search p95 `84.7 ms`.
- Memory measurement: `scripts/measure-peak-memory.ps1` samples descendant process working sets and reports optional `peak_vram_mb` when an available telemetry tool returns it.
- Search-latency implementation scout: parallel semantic score computation was tested and discarded because it slowed the measured holdout and increased sampled RAM.
- Search-latency top-k scout: top-k semantic score collection was tested and discarded because the repeat failed the quality gate.
- Segment 2 restart: q8/r6 full-text was rebaselined, q5/r6 and no-fulltext/r6 were repeated before promotion, q8/r5 and BGE-small were rejected, and isolated `current_alias`/`no_alias` semantic-doc probes confirmed `alias_variant`.

## What Was Not Proven

- True producer-consumer streaming from parsed/indexed symbols into the embedder
  has not been proven. Several scouts reduced or rearranged semantic work, but
  none established a safe end-to-end streaming architecture that beats the
  historical q8/r6 full-text baseline or a fresh mandatory-sidecar row.
- The historical external gate covers four useful repository families, but the
  current sidecar contract and any broader default should still be checked on
  representative repos before treating the profile as universal.
- Q5 remains a compression option only if a future quality-preserving recipe
  exists. The current scout saved model footprint but did not beat the q8/r6
  incumbent under the user-weighted metric.
- Disabling full-text remains an architecture idea, not a default. Its corrected
  repeat did not justify promotion.
- Broad semantic-doc alias toggles are not a proven improvement. Segment 2
  isolated both `current_alias` and `no_alias`; both failed the corrected quality
  gate, so future doc-shape work needs miss-level evidence instead of another
  whole-mode flip.
- VRAM was not measured on this Windows/Vulkan host because `nvidia-smi` did not
  return memory usage. Peak RAM is sampled working set evidence, not exact max
  RSS.
- ONNX is now the active managed path. Older ONNX evidence should not drive new
  choices until the current implementation has fresh speed, quality, and
  cross-repo rows.

## How To Use This Matrix

Use this file as the primary reference for embedding and pipeline decisions. If
a new candidate is added, update the matrix with the candidate shape, the best
decision-grade metric row, the quality/rank signal, the speed and footprint
signal, and the decision. Do not add raw run transcripts or local artifact
catalogs to the repo.
