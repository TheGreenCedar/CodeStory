# Embedding Backend Benchmarks

This page is the current decision sheet for CodeStory embedding backends,
semantic-doc aliasing, and benchmark follow-up work. It intentionally avoids the
older diary format: use this file first, then open the raw artifacts only when
you need per-query ranks or logs.

## Current Decision

| Question | Decision |
| --- | --- |
| Default self-contained backend | `CODESTORY_EMBED_BACKEND=onnx` |
| Default model/profile | `CODESTORY_EMBED_PROFILE=bge-small-en-v1.5` |
| Default semantic doc alias mode | `CODESTORY_SEMANTIC_DOC_ALIAS_MODE=alias_variant` |
| Default client batch size | `CODESTORY_LLM_DOC_EMBED_BATCH_SIZE=128` |
| Default ONNX sessions | Keep the runtime default cap of `2`; `4` was not repeat-stable enough to become default |
| Best Run 2 quality score candidate | BGE-base GGUF via llama.cpp/Vulkan, `alias_variant`, `scope=all`, batch `512`, CodeStory request count `4`, server `-np 4`, `--pooling cls` |
| Best 60/30/10 local pipeline profile | BGE-base GGUF via llama.cpp/Vulkan, `alias_variant`, `scope=durable`, batch `768`, CodeStory request count `4`, server `-np 4`, `-ub 1024`, `--pooling cls`, with `CODESTORY_STORED_VECTOR_ENCODING=int8` scaled-int8 persistence |
| Compressed BGE-base candidate | Clean-source BGE-base Q5_K_M GGUF via llama.cpp/Vulkan, `alias_variant`, `scope=all`, batch `512`, request count `4`, server `-np 4`, `--pooling cls`; three full-query repeats matched Q8 quality with lower throughput |
| Best ONNX quality fallback | BGE-base ONNX DirectML, `alias_variant`, `scope=all`, batch `128`, sessions `2` |
| Best llama.cpp throughput profile | BGE-base GGUF, `alias_variant`, batch `128`, `--pooling cls`, CodeStory request count `4`, server `-np 4` |
| Fast prior finalist profile | ONNX BGE-small with `no_alias`, batch `256` |
| Best BGE-small promotion candidate | ONNX BGE-small with `scope=all`, `no_alias`, batch `256`; three full-query repeats passed with MRR@10 0.6089 and Hit@10 0.9000 |
| Promoted compact-storage profile | Compact scaled-int8 persisted vectors under `CODESTORY_STORED_VECTOR_ENCODING=int8`; BGE-base b768/r4 with llama.cpp `-ub 1024` passed the local 150-query suite and external four-repo gate, with 777 persisted bytes per doc |
| Best Run 2 retrieval signal | `scope=all` improved BGE-small MRR most, while `no_alias` was the best durable-scope row; original retrieval speed rows are invalid because the GPU was locked |
| Quality experiment, not default | Qwen3 0.6B GGUF with `alias_variant`, context `2048`, `r1/np1` |
| Measured negative candidate | `nomic-embed-text-v2-moe` now runs with `CODESTORY_SEMANTIC_DOC_MAX_TOKENS`, but 256/768-dimensional bounded rows stayed below gate |

The shipped runtime default remains BGE-small because it is the current
self-contained baseline in code and docs. The crossed BGE-small candidate is now
promotion-grade evidence, but it still does not silently flip defaults: changing
the runtime shape requires an explicit default-change decision and the
repo-scale gate.

The 2026-04-23 60/30/10 pipeline segment promotes BGE-base b768/r4 plus
scaled-int8 persisted vectors as the best measured opt-in family, not as a
silent runtime default. Keep the default BGE-small/float32 path unchanged until
the owner explicitly accepts a default flip; set the BGE-base profile and
`CODESTORY_STORED_VECTOR_ENCODING=int8` when reproducing the promoted pipeline.
The local speed leader additionally sets llama.cpp `-ub 1024`; cross-repo run
`20260423024405` passed the four-repo gate for that knob.

The alias feature stays, but the older full alias text does not become the
default. The default is the compact alias variant: language, terminal name,
owner names, and symbol-role hints are kept; full name-alias and path-alias
lists are excluded unless `CODESTORY_SEMANTIC_DOC_ALIAS_MODE=current_alias` is
set for reproduction.

## Candidate Coverage Matrix

This table is the guardrail against forgetting models. Every model in
`scripts/embedding-gpu-fair-benchmark.mjs` should appear here, even if it is
only a baseline, a blocked candidate, or a planned quantization row.

| Model/profile | Local artifacts and backends | Evidence so far | Current status |
| --- | --- | --- | --- |
| `minilm` / all-MiniLM-L6-v2 | ONNX and GGUF Q8 | Legacy full GPU run only. ONNX: MRR@10 0.2668, Hit@10 0.5556, 773 docs/sec. llama.cpp: MRR@10 0.2372, Hit@10 0.6111, 610 docs/sec. | Speed/sanity baseline only; not competitive on quality. Keep in smoke/full runs, not finalists. |
| `bge-small-en-v1.5` | ONNX default, GGUF Q8/Q5, planned GGUF Q6/Q4, generated ONNX int8, stored-vector int8/uint8/binary, planned ONNX int4 | Shipped default profile. Stage 1 showed `no_alias` beat aliases for BGE-small. Stage 4 no-alias finalist was fast but lower quality. Run 2 controls made `scope=all`, `no_alias`, b256 the best composite controls row. Run 2 retrieval isolated knobs: `scope=all` had best MRR, and `no_alias` was the best durable-scope row. The focused `bge-small-candidate` stage repeated the crossed row three bounded times, then three full-query times. Full-query quality was stable at MRR@10 0.6089, Hit@10 0.9000, Hit@1 0.4571, Persistent Hit@10 0.25, with average docs/sec about 786.52. Dynamic-int8 ONNX was provider-valid but measured-negative: default shape scored MRR@10 0.1435, Hit@10 0.5556; the crossed fast profile scored MRR@10 0.0992, Hit@10 0.4444. Stored-vector byte quantization on the winner-shape bounded slice preserved float32 ranking exactly: float32, int8, and uint8 all had MRR@10 0.2531, Hit@10 0.6667, and Persistent Hit@10 0.25; binary kept Hit@10 but regressed MRR to 0.2269. Clean-source GGUF Q5_K_M and same-backend Q8_0 both passed Hit@10 0.9000, but both lost Persistent Hit@10 and indexed around 467-471 docs/sec, below the ONNX finalist. | Promotion candidate only in float32 crossed ONNX form. Stored-vector int8/uint8 are promising storage candidates but need full-query reruns after DirectML throughput is healthy. Dynamic int8 model weights, binary stored vectors, and current BGE-small GGUF Q8/Q5 are not candidates. Keep the runtime default unchanged until the owner explicitly accepts the default-shape change and the repo-scale gate passes. |
| `bge-base-en-v1.5` | ONNX, GGUF Q8, clean F16-derived GGUF Q6/Q5/Q4, locally generated GGUF Q6 from Q8, generated ONNX int8, planned ONNX int4 | Best prior quality family. Stage 4 ONNX s2 had MRR@10 0.5006. Run 2 controls improved all-scope MRR@10 to 0.6379 for ONNX and 0.6362 for llama.cpp, both Hit@10 0.9143. Finalists2 then moved BGE-base to full-query decision evidence: ONNX repeated at average score 789.1745, MRR@10 0.6593, Hit@10 0.9286, and about 303.75 docs/sec; llama.cpp/Vulkan b128/r4 repeated at average score 791.5879, MRR@10 0.6601, Hit@10 0.9286, and about 435.88 docs/sec. A b256/r4 follow-up repeated at average score 791.8120, MRR@10 0.6605, Hit@10 0.9286, Persistent Hit@10 0.25, and about 424.27 docs/sec. A b512/r4 follow-up improved the score frontier again, averaging 791.9135 with the same quality shape, about 434.30 docs/sec, and 41.14s index time. Adjacent b768/b1024/b384 checks did not beat b512/r4: b768 failed repeat stability, while b1024 and b384 regressed MRR. Dynamic-int8 ONNX shrank the local artifact to about 109.7 MB, but the bounded row regressed to MRR@10 0.2593, Hit@10 0.6667, and 129.37 docs/sec. Q8-to-Q6 GGUF requantization and clean-source Q6/Q4 both lost Hit@10 or persistent-hit. Clean-source Q5_K_M recovered the persistent bucket and repeated stably under b128/r4 at average score 791.5032, then repeated under the b512/r4 leader shape at average score 791.6327 with the same MRR@10 0.6605, Hit@10 0.9286, Persistent Hit@10 0.25, and about 407.00 docs/sec. | Current score leader in Q8 llama.cpp/Vulkan b512/r4 form; b128/r4 remains the fastest stable throughput leader. ONNX remains the best non-llama fallback. Clean-source Q5_K_M b512/r4 is the repeat-stable compressed BGE-base GGUF candidate, but it trades lower throughput for smaller model footprint and still needs an owner/runtime decision before promotion. Dynamic int8, Q8-to-Q6 requantization, clean-source Q6, clean-source Q4, and adjacent batch sizes b384/b768/b1024 are measured-negative for promotion. Do not flip defaults automatically without an owner/runtime decision and repo-scale gate. |
| `nomic-embed-text-v1.5` | GGUF Q8, planned GGUF Q6/Q5/Q4, MRL dimensions 768/512/256/128/64 | Legacy full GPU run: MRR@10 0.4602, Hit@10 0.7222, 302 docs/sec. Stage 1 found `current_alias` was its best alias mode at MRR@10 0.4739. The old no-prefix prompt row crashed, but Run 2 retested it cleanly: no-prefix scored 714.5411 with MRR@10 0.6028 and Hit@10 0.8714. The matching prefixed/current-profile row repeated stably across three full-query runs at average score 753.9136, MRR@10 0.6407, Hit@10 0.8857, Hit@1 0.5143, Persistent Hit@10 0, and 308.11 docs/sec. Bounded 2026-04-21 dimension probes remained weak: 256 dim had MRR@10 0.2917 and Hit@10 0.5556; 768 dim regressed to MRR@10 0.2381 with the same Hit@10. | Repeat-stable fallback, not a default candidate. Keep prefixes. It edges the BGE-small finalist on score but is much slower and loses the persistent bucket; do not spend more dimension-only loops without a new semantic-doc/query hypothesis. |
| `nomic-embed-text-v2-moe` | GGUF Q8, MRL dimensions 768/256 under `CODESTORY_SEMANTIC_DOC_MAX_TOKENS` | Legacy full run failed; prior alias docs exceeded the model context cap. Current source confirms required prefixes, 512-token maximum input, and Matryoshka truncation such as 256 dimensions. Run 2 added an opt-in conservative semantic-doc token budget after repeated context crashes. Provider-verified bounded rows then completed but were weak: 256 dim scored MRR@10 0.3086, Hit@10 0.5556, docs/sec 163.02; 768 dim regressed to MRR@10 0.2222, Hit@10 0.4444. | No promotion candidate. The context blocker is solved for research, but Nomic v2 is measured-negative on this query slice. |
| `embeddinggemma-300m` | GGUF Q8, MRL dimensions 512/256/128 | Smaller early quality run looked promising, but legacy full GPU run was weaker at MRR@10 0.3815 and Hit@10 0.6111. Stage 1 alias test favored `no_alias`; `current_alias` regressed by about 0.0504 MRR. Current source confirms 512/256/128 Matryoshka truncation. Bounded 2026-04-21 probes were provider-valid but weak: 256 dim had MRR@10 0.3611 and Hit@10 0.5556; 768 dim regressed to MRR@10 0.2963 with the same Hit@10. | No dimension-only promotion candidate. Avoid more Gemma dimension loops unless the semantic-doc/query shape changes. |
| `qwen3-embedding-0.6b` | GGUF Q8, planned GGUF Q6/Q5/Q4, MRL dimensions 512/256/128 | Initial full b128 row failed, then bounded runs worked. Stage 1 alias variant reached MRR@10 0.5208. Stage 4 finalist averaged MRR@10 0.5060 but Hit@10 was 0.7857 and throughput was about 73 docs/sec. Current source confirms MRL/custom output dimensions from 32 to 1024 and instruction-aware retrieval. Bounded 2026-04-21 dimension probes recovered one persistent miss but still failed the gate: both 512 and 1024 dim had MRR@10 0.4389 and Hit@10 0.6667; 1024 improved Hit@1 to 0.3333 but remained slow at 76.68 docs/sec. | Strongest dimension-lane quality signal, but still too slow and below quality gate. Do not try smaller Qwen dimensions without a new quality hypothesis. |

Quantized model rows are part of the plan, but most lower-bit artifacts are not
present yet outside the BGE families. Dynamic-int8 ONNX artifacts were generated
locally for BGE-small and BGE-base and were provider-valid, but both regressed
too far to promote. Clean-source BGE-base GGUF Q6/Q5/Q4 artifacts were also
generated and measured: Q5_K_M b512/r4 is a repeat-stable compression candidate,
while Q6_K and Q4_K_M are negative quality/persistent-hit evidence. Clean-source
BGE-small GGUF Q5_K_M was close to the same-backend Q8_0 control, but both lost
Persistent Hit@10 and trailed the ONNX BGE-small finalist on speed. Missing GGUF
Q6/Q5/Q4 and ONNX int4 artifacts for other models should still produce skipped
rows in `weight-quant`; they are not evidence that quantization hurts quality.

Stored-vector quantization is a separate storage/search implementation lane.
The first BGE-small bounded rows used quantized corpus prefiltering plus
full-precision rescoring; int8/uint8 preserved ranking on that slice while
binary sign-bit storage lost MRR. The current CodeStory implementation also
supports compact persisted storage: with `CODESTORY_STORED_VECTOR_ENCODING=int8`
it writes versioned scaled-int8 embedding blobs and decodes them back to
normalized float vectors for the existing search path. The BGE-base b768/r4
scaled-int8 profile now has local full-query and external four-repo promotion
evidence; the older BGE-small byte rows are historical storage evidence, not the
current compact-storage promotion row.

## Run 2 Controls Snapshot

The first expanded Run 2 controls stage ran on 2026-04-19 with artifacts in
`target/embedding-research/controls-run2-20260419`. It used 70 queries and
three repeats per control row.

| Rank | Profile | Backend | Doc mode | Scope | Settings | MRR@10 | Hit@1 | Hit@10 | Persistent Hit@10 | Docs/sec | Index s | Decision |
| ---: | --- | --- | --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| 1 | BGE-small | ONNX DirectML | `no_alias` | `all` | b256, s2 | 0.6130 | 0.4571 | 0.9143 | 0.2500 | 740.528 | 31.642 | Best Run 2 controls score; promote to retrieval sweep, not default yet |
| 2 | BGE-base | llama.cpp Vulkan | `alias_variant` | `all` | b128, r4/np4, ctx4096, cls pool | 0.6362 | 0.4857 | 0.9143 | 0.0000 | 399.220 | 42.175 | Best footprint/speed among BGE-base quality controls |
| 3 | BGE-base | ONNX DirectML | `alias_variant` | `all` | b128, s2 | 0.6379 | 0.4857 | 0.9143 | 0.0000 | 301.715 | 50.642 | Highest MRR, but worst footprint and slower than llama.cpp |
| 4 | BGE-small | ONNX DirectML | `alias_variant` | `durable` | b128, s2 | 0.5268 | 0.3286 | 0.9143 | 0.0000 | 404.451 | 19.556 | Current default baseline; fastest cold index because it embeds fewer docs |

Controls are not enough to change the runtime default. The main signal is that
`semantic_scope=all` plus BGE-small `no_alias` deserves the next retrieval sweep:
it recovered one persistent-miss bucket while staying much faster and smaller
than BGE-base. The cost is a larger semantic corpus, so compare it against
hybrid weights and alias variants before promoting anything.

## Run 2 Retrieval Snapshot

The retrieval stage ran on 2026-04-19 with artifacts in
`target/embedding-research/retrieval-run2-20260419`. It used 70 queries, ran
seven BGE-small ONNX rows, and recorded one skipped Nomic v2 row.

The original retrieval timing data is invalid for speed/cost decisions because
the GPU was locked or unavailable during the run. The default retrieval row took
115.773s to index, while the same shape in the earlier controls run took
19.556s. Slow one-row checks reproduced the bad timing while the GPU was still
unavailable. After making DirectML/CUDA provider registration fail hard and
rerunning with the GPU available, the same default retrieval shape returned to
the expected range:

| Probe | Artifact root | Index s | Semantic s | Docs/sec | Use |
| --- | --- | ---: | ---: | ---: | --- |
| Original retrieval default | `target/embedding-research/retrieval-run2-20260419` | 115.773 | 107.089 | 35.783 | Quality only; speed invalid |
| Slow normal-load check | `target/embedding-research/retrieval-run2-speedcheck-normal-20260419` | 105.977 | 97.643 | 39.245 | Invalid; GPU still not executing normally |
| Fail-hard DirectML check | `target/embedding-research/retrieval-run2-speedcheck-failhard-20260419` | 20.713 | 8.871 | 431.969 | Valid provider/speed sanity |
| Confirmed GPU check | `target/embedding-research/retrieval-run2-speedcheck-confirmed-gpu-20260419` | 17.277 | 8.984 | 426.536 | Valid provider/speed sanity |

The speed lesson is not "DirectML is slow." It is "DirectML benchmark rows must
fail hard on provider registration and be run only when the GPU is actually
available." Treat the retrieval quality deltas below as useful, but rerun any
row before using its speed/cost numbers.

| Rank | Profile | Backend | Doc mode | Scope | Weights | MRR@10 | Hit@1 | Hit@10 | Persistent Hit@10 | Misses | Decision |
| ---: | --- | --- | --- | --- | --- | ---: | ---: | ---: | ---: | ---: | --- |
| 1 | BGE-small | ONNX DirectML | `alias_variant` | `all` | default | 0.6299 | 0.5143 | 0.9000 | 0.0000 | 7 | Highest MRR and Hit@1, but added `file-text-match-line` as a miss; rerun before using speed |
| 2 | BGE-small | ONNX DirectML | `no_alias` | `durable` | default | 0.5780 | 0.4000 | 0.9143 | 0.2500 | 6 | Best durable-scope row; recovered `resolve-target` but lost `role-struct-refresh-plan` |
| 3 | BGE-small | ONNX DirectML | `current_alias` | `durable` | default | 0.5319 | 0.3286 | 0.9143 | 0.0000 | 6 | Full aliases helped a few alias/path queries but regressed more rows |
| 4 | BGE-small | ONNX DirectML | `alias_variant` | `durable` | semantic-heavy `0.15/0.8/0.05` | 0.5268 | 0.3286 | 0.9143 | 0.0000 | 6 | Same ranks as default weights; semantic weighting alone did not move this corpus |
| 5 | BGE-small | ONNX DirectML | `alias_variant` | `durable` | default | 0.5268 | 0.3286 | 0.9143 | 0.0000 | 6 | Current runtime-shape baseline |
| 6 | BGE-small | ONNX DirectML | `alias_variant` | `durable` | balanced `0.45/0.45/0.1` | 0.5161 | 0.3143 | 0.9143 | 0.0000 | 6 | Slightly worse than default; only notable loss was `trail-to-target` |
| 7 | BGE-small | ONNX DirectML | `alias_variant` | `durable` | lexical-heavy `0.65/0.3/0.05` | 0.5011 | 0.3000 | 0.9000 | 0.0000 | 7 | Regressed trail queries; not a promotion path |

Query-level deltas against the current runtime-shape baseline:

- `scope=all` produced 23 better ranks and 11 worse ranks. It improved several
  expanded-suite and alias-sensitive rows, including `canonical-depth`,
  `refresh-request`, `cache-root-hash`, `fnv1a-cache-hash`, and
  `semantic-path-aliases`, but it newly missed `file-text-match-line`.
- `no_alias` durable produced 23 better ranks and 10 worse ranks. It newly
  found `resolve-target` at rank 9 and improved `search-rank`,
  `onnx-normalized-embeddings`, `llamacpp-endpoint`, and `index-file`; it newly
  missed `role-struct-refresh-plan`.
- `current_alias` durable produced 9 better ranks and 12 worse ranks. It helped
  some path/alias examples such as `file-text-match-line` and
  `workspace-exclude-patterns`, but not enough to justify full alias text.
- The hybrid-weight sweeps did not improve the default durable alias-variant
  ranking. Semantic-heavy was identical to default; balanced and lexical-heavy
  regressed trail rows.
- `nomic-embed-text-v2-moe` remained skipped because semantic docs still need a
  hard token budget before the model can be judged fairly.

The retrieval stage did not run the exact controls lead as a crossed candidate:
`scope=all` + `no_alias` + b256. The focused `bge-small-candidate` stage has now
covered that gap with provider-verified bounded repeats and three full-query
repeats. Treat it as the current BGE-small promotion candidate, not an automatic
default change.

## Per-Model Notes

### MiniLM

MiniLM exists to keep a cheap baseline in view. It is fast enough to be useful
as a harness sanity check, but the legacy full GPU run had much lower quality
than BGE-small, BGE-base, Qwen, Nomic v1.5, and EmbeddingGemma. It should stay in
smoke/full coverage but should not consume finalist slots unless the test goal
is explicitly "minimum viable embedding quality."

### BGE-Small

BGE-small is the shipped self-contained default because it is small, already
bundled, and operationally cheap. The evidence is mixed by doc shape:

- Stage 1 alias testing said `no_alias` was better than aliases for BGE-small.
- Stage 4 finalist testing kept it as the fast lower-quality profile.
- Run 2 controls changed the picture: `scope=all`, `no_alias`, b256 reached
  MRR@10 0.6130, Hit@10 0.9143, and the only nonzero Persistent Hit@10 in the
  controls table.
- Run 2 retrieval then isolated the knobs: `scope=all` with compact aliases had
  the highest MRR@10 at 0.6299, while durable `no_alias` had the best
  durable-scope score at MRR@10 0.5780 and Persistent Hit@10 0.2500.
- Hybrid weight tweaks were not the missing lever for BGE-small in this corpus:
  semantic-heavy tied the default ranks, while balanced and lexical-heavy
  regressed specific trail queries.

That is now enough to call `scope=all` + `no_alias` + b256 the BGE-small
promotion candidate. The bounded slice repeated stably but stayed below the
bounded Hit@10 gate; the full-query follow-up cleared the gate three times with
MRR@10 0.6089, Hit@10 0.9000, Hit@1 0.4571, Persistent Hit@10 0.25, average
docs/sec about 786.52, and average index time about 28.63s. Keep the shipped
default unchanged until the owner accepts the tradeoff and the repo-scale gate
passes.

### BGE-Base

BGE-base is still the quality reference. It is the best prior ONNX quality
candidate and the best llama.cpp throughput candidate. Run 2 controls showed
both ONNX and llama.cpp BGE-base passing the quality gate with essentially the
same Hit@10 and MRR, but llama.cpp had a much smaller model artifact and better
docs/sec in that controls run. The tradeoff is still footprint: the vector bytes
per doc are double BGE-small because the embedding dimension is 768 instead of
384.

The finalists2 full-query repeats changed the current recommendation. ONNX
BGE-base `scope=all`, `alias_variant`, b128/s2 repeated stably at average score
789.1745, MRR@10 0.6593, Hit@10 0.9286, Hit@1 0.5286, Persistent Hit@10 0.25,
303.75 docs/sec, and 53.25s index time. The llama.cpp/Vulkan BGE-base
`r4/np4`, `ctx4096`, `--pooling cls` finalist then repeated stably at average
score 791.5879, MRR@10 0.6601, Hit@10 0.9286, Hit@1 0.5286, Persistent Hit@10
0.25, 435.88 docs/sec, and 40.50s index time. A follow-up b256/r4 batch-size
frontier repeated at a higher average score 791.8120 with MRR@10 0.6605,
Hit@10 0.9286, Hit@1 0.5286, Persistent Hit@10 0.25, 424.27 docs/sec, and
42.74s index time. The b512/r4 frontier then repeated at average score 791.9135
with the same quality shape, 434.30 docs/sec, and 41.14s index time, including
the current single-run best score 792.0033. The adjacent b768, b1024, and b384
frontier checks did not displace it: b768 did not repeat above b512, and b1024
plus b384 both regressed MRR. Treat b512/r4 as the current measured score leader
and b128/r4 as the fastest stable throughput leader, with ONNX BGE-base as the best
non-llama quality fallback.

The later 60/30/10 pipeline segment reweighted the decision around retrieval
quality, indexing/search speed, and persisted memory footprint. Under that
metric, BGE-base llama.cpp/Vulkan with `alias_variant`, `scope=durable`, b768,
request count 4/server `-np 4`, cls pooling, and scaled int8 persisted vectors
is the current promoted opt-in family. Run 92 adds llama.cpp `-ub 1024` and
scores `pipeline_score` 882198.781327 over 150 CodeStory queries with MRR@10
1.0, Hit@1 1.0, Hit@10 1.0, 360.15 docs/sec, and 777 persisted vector bytes per
doc after compacting the scaled-int8 header. Cross-repo promotion run
`20260423024405` passed for the `-ub 1024` compact-header b768 scaled-int8
profile over freelancer, traderotate, the-green-cedar, and Sourcetrail with
aggregate Hit@10 1.0, MRR@10 0.826936508, adversarial Hit@10 1.0, search p95
89.576 ms, and no misses. Keep this as a named profile or explicit
environment-driven path until the owner accepts a default change.

The leader-shape doc-mode variants did not beat `alias_variant`. Under the same
llama.cpp/Vulkan `scope=all`, b128, r4/np4, ctx4096, cls-pool shape,
`current_alias` scored 743.7298 with MRR@10 0.6269, Hit@10 0.9143, Hit@1
0.4714, Persistent Hit@10 0, 370.85 docs/sec, and 47.79s index time. `no_alias`
was faster at 479.50 docs/sec and 40.76s index time, but still scored only
772.1050 with MRR@10 0.6542, Hit@10 0.9143, Hit@1 0.5143, and Persistent
Hit@10 0. Keep `alias_variant` for the current BGE-base llama.cpp leader.

A local Q6_K compression probe is not promotion evidence. It was generated by
requantizing the existing Q8 GGUF with `llama-quantize --allow-requantize`, not
from an F16/F32 source artifact. A same-parallelism Q8 r2/np2 control scored
790.5192, MRR@10 0.6601, Hit@10 0.9286, Persistent Hit@10 0.25, 340.44
docs/sec, and 48.21s index time. The old r2/np2 Q6_K row fell to score
774.859, MRR@10 0.6588, Hit@10 0.9143, Persistent Hit@10 0, 310.04 docs/sec,
and 53.38s index time. The leader-aligned r4/np4 Q6_K row recovered throughput
to 400.72 docs/sec and 42.71s index time, but quality fell further to score
770.0178, MRR@10 0.6529, Hit@10 0.9143, Hit@1 0.5143, and Persistent Hit@10 0.
Both rows used the artifact that shrank from the quantizer-reported 111.78 MiB
Q8 model size to 86.75 MiB. Keep them as negative requantization baselines; use
a clean source artifact before judging BGE-base GGUF quantization generally.

Clean-source GGUF quantization is now measured separately from the Q8
requantization baseline. The F16 source artifact was downloaded from
[CompendiumLabs/bge-base-en-v1.5-gguf](https://huggingface.co/CompendiumLabs/bge-base-en-v1.5-gguf)
and locally quantized with llama.cpp b8840. Clean Q6_K reduced the artifact to
86.00 MiB by quantizer report but still lost the persistent bucket: score
776.2653, MRR@10 0.6593, Hit@10 0.9143, Hit@1 0.5286, Persistent Hit@10 0,
384.32 docs/sec, and 43.98s index time. Clean Q5_K_M is the only compression
candidate from this lane: it reduced the artifact to 77.49 MiB by quantizer
report and repeated at average score 791.5032, MRR@10 0.6605, Hit@10 0.9286,
Hit@1 0.5286, Persistent Hit@10 0.25, 395.01 docs/sec, and 43.11s index time
across three full-query rows. Rerunning that same Q5_K_M artifact under the
current b512/r4 leader shape preserved the exact quality shape again and
improved the compressed-candidate average to score 791.6327, 407.00 docs/sec,
and 41.54s index time across three full-query rows. Clean Q4_K_M
reduced the artifact to 69.47 MiB by quantizer report but fell back to score
774.6363, MRR@10 0.6576, Hit@10 0.9143, Hit@1 0.5143, Persistent Hit@10 0,
387.68 docs/sec, and 43.66s index time. Treat Q5_K_M as repeat-stable candidate
evidence, with b512/r4 as the compressed deployment shape; stop below Q5
without a new imatrix or calibration recipe.

For BGE-small, the F16 source artifact was downloaded from
[CompendiumLabs/bge-small-en-v1.5-gguf](https://huggingface.co/CompendiumLabs/bge-small-en-v1.5-gguf)
and quantized locally with llama.cpp b8840. Q5_K_M reduced the model to 27.96 MiB
by quantizer report, but the quantizer fell back on 60 of 197 tensors because
several 384-wide matrices do not fit K-quant block sizes. The full-query Q5_K_M
row scored 737.9803 with MRR@10 0.6214, Hit@10 0.9000, Hit@1 0.5000,
Persistent Hit@10 0, 467.31 docs/sec, and 38.44s index time. The same-backend
Q8_0 control scored 739.8364 with MRR@10 0.6233, Hit@10 0.9000, Hit@1 0.5000,
Persistent Hit@10 0, 470.64 docs/sec, and 40.11s index time. Q5 is close to Q8,
so lower-bit quantization is not the primary issue; this BGE-small GGUF shape is
below the ONNX BGE-small finalist and should pause unless a new pooling/doc-mode
hypothesis or GGUF-only runtime requirement appears.

### Nomic V1.5

Nomic v1.5 should not be treated as "done" just because it is missing from the
finalist table. Its main reason to stay in the plan is Matryoshka dimensionality
support. The first bounded 256-dimensional continuation probe was provider-valid
but weak on the persistent/alias slice, so do not promote 256. If the lane is
reopened, compare 768 and 512 before spending time on lower dimensions.

### Nomic V2 MoE

Nomic v2 MoE is blocked by input-budget correctness, not rejected for model
quality. The current model card also makes the desired rerun clearer: use
`search_query:` and `search_document:` prefixes, respect the 512-token maximum
input length, and include the 256-dimensional Matryoshka setting once the token
budget exists. Do not rerun it by shrinking random parameters around the
failure.

### EmbeddingGemma

EmbeddingGemma looked better in early smaller runtime-backed runs than in the
fairer full GPU run. The alias stage also showed aliases hurt it. The new
source-backed hypothesis is dimension shortening: the model card documents
512/256/128 Matryoshka outputs. The first 256-dimensional bounded probe did not
hold quality, so 256 should be treated as discarded unless a broader doc/query
change gives a reason to retest.

### Qwen3 0.6B

Qwen is a real quality experiment, not a default candidate. Its MRR was strong
in the alias and finalist stages, but Hit@10 and throughput make it hard to
justify as a default. The current model card now provides source evidence for
MRL/custom output dimensions, so the next fair Qwen question is whether 512 or
256 dimensions preserves enough quality while improving vector footprint and
possibly ingestion/search costs. The first 512-dimensional bounded probe was
provider-valid and better than the Gemma/Nomic 256 probes, but it still failed
Hit@10 and took 163.587s to index the sliced run. Keep it as evidence, not as a
promotion candidate.

## Final Accepted Table

These rows are averaged Stage 4 finalist repeats from
`target/embedding-research/finalists-stage4-20260419/repeat-summary.csv`.
All rows were GPU-only: ONNX used DirectML and llama.cpp rows logged Vulkan0 plus
full model-layer offload.

`score = 0.70 * normalized(MRR@10) + 0.30 * normalized(docs/sec)` across the
averaged finalist rows. The score is a ranking aid only; raw quality and speed
remain visible so the score cannot hide lower Hit@10 or persistent misses.

| Rank | Profile | Backend | Doc mode | Settings | MRR@10 | Hit@1 | Hit@10 | Mean rank | Docs/sec | Index s | Score | Use |
| ---: | --- | --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| 1 | BGE-base | ONNX DirectML | `alias_variant` | b128, s2 | 0.5006 | 0.3035 | 0.8214 | 2.2391 | 301.899 | 50.524 | 0.7683 | Best prior quality candidate |
| 2 | Qwen3 0.6B | llama.cpp Vulkan | `alias_variant` | b128, r1/np1, ctx2048, last pool | 0.5060 | 0.3571 | 0.7857 | 2.1364 | 73.214 | 160.122 | 0.7000 | Quality experiment only: slower and lower Hit@10 |
| 3 | BGE-base | llama.cpp Vulkan | `alias_variant` | b128, r4/np4, ctx4096, cls pool | 0.4881 | 0.2857 | 0.8214 | 2.3043 | 388.447 | 44.205 | 0.6674 | Best llama.cpp throughput profile |
| 4 | BGE-base | ONNX DirectML | `alias_variant` | b128, s4 | 0.4881 | 0.2857 | 0.8214 | 2.3043 | 303.456 | 50.978 | 0.6176 | Not enough gain over s2 to become default |
| 5 | BGE-base | llama.cpp Vulkan | `alias_variant` | b128, r1/np1, ctx4096, cls pool | 0.4881 | 0.2857 | 0.8214 | 2.3043 | 210.805 | 63.773 | 0.5634 | Slower; earlier MRR bump did not repeat |
| 6 | BGE-small | ONNX DirectML | `no_alias` | b256, s2 | 0.4483 | 0.2857 | 0.8214 | 2.9565 | 586.109 | 34.017 | 0.3000 | Fast profile with quality tradeoff |

Persistent misses in the finalist rows still include
`trail-neighborhood`, `semantic-sync`, `semantic-doc-text`, and
`resolve-target`. Qwen changes the miss pattern but does not solve enough of it
to justify the throughput cost as a default.

## Run 2 Finalists2 Table

These rows use the expanded 70-query suite and the gated Run 2 score. They are
the current promotion evidence and supersede the older Stage 4 ranking when
choosing between BGE-base backends.

| Rank | Profile | Backend | Doc mode | Scope | Settings | Repeats | Score avg | MRR@10 | Hit@1 | Hit@10 | Persistent Hit@10 | Docs/sec avg | Index s avg | Use |
| ---: | --- | --- | --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| 1 | BGE-base | llama.cpp Vulkan | `alias_variant` | `all` | b512, r4/np4, ctx4096, cls pool, Q8 | 3 | 791.9135 | 0.6605 | 0.5286 | 0.9286 | 0.2500 | 434.30 | 41.14 | Current score leader |
| 2 | BGE-base | llama.cpp Vulkan | `alias_variant` | `all` | b256, r4/np4, ctx4096, cls pool, Q8 | 3 | 791.8120 | 0.6605 | 0.5286 | 0.9286 | 0.2500 | 424.27 | 42.74 | Prior score leader |
| 3 | BGE-base | llama.cpp Vulkan | `alias_variant` | `all` | b512, r4/np4, ctx4096, cls pool, Q5_K_M | 3 | 791.6327 | 0.6605 | 0.5286 | 0.9286 | 0.2500 | 407.00 | 41.54 | Compressed deployment candidate |
| 4 | BGE-base | llama.cpp Vulkan | `alias_variant` | `all` | b128, r4/np4, ctx4096, cls pool, Q8 | 3 | 791.5879 | 0.6601 | 0.5286 | 0.9286 | 0.2500 | 435.88 | 40.50 | Fastest stable llama.cpp throughput profile |
| 5 | BGE-base | ONNX DirectML | `alias_variant` | `all` | b128, s2 | 3 | 789.1745 | 0.6593 | 0.5286 | 0.9286 | 0.2500 | 303.75 | 53.25 | Best ONNX/non-llama fallback |
| 6 | BGE-small | ONNX DirectML | `no_alias` | `all` | b256, s2 | 1 finalists2 row plus prior 3-run candidate stage | 752.9593 | 0.6219 | 0.4857 | 0.9000 | 0.2500 | 745.60 | 29.46 | Smaller/faster fallback; quality below BGE-base |

The llama.cpp BGE-base results are not silent default changes. They are the best
measured candidates if the runtime can depend on the llama.cpp/Vulkan sidecar
and can accept 768-dimensional vectors. BGE-small remains the default
self-contained profile until that product/runtime decision is made.

For the current 60/30/10 pipeline metric, the b768 scaled-int8 profile supersedes
the older b512/r4 finalist as the promoted opt-in pipeline family because it
adds real persisted-footprint savings while preserving local perfect retrieval
on the 150-query suite and passing the external four-repo gate. The `-ub 1024`
variant is the current gated leader inside that family.

## Alias Decision

Stage 1 compared `no_alias`, `current_alias`, and `alias_variant` at batch 128.
The result is not a universal "aliases are always better" result:

| Model/backend | Best observed alias mode | MRR@10 movement vs `no_alias` | Decision |
| --- | --- | ---: | --- |
| ONNX BGE-base | `alias_variant` | +0.0235 | Use for default |
| llama.cpp BGE-base | `alias_variant` | +0.0134 | Use for llama BGE-base profiles |
| llama.cpp Qwen3 0.6B | `alias_variant` | +0.0327 | Keep for optional quality experiments |
| llama.cpp Nomic v1.5 | `current_alias` | +0.0247 | Keep as model-specific fallback evidence |
| ONNX BGE-small | `no_alias` | `alias_variant` was -0.0274 | Do not force aliases onto BGE-small |
| llama.cpp EmbeddingGemma | `no_alias` | `current_alias` was -0.0504 | Do not use as a default candidate |

Because model responses differ, alias mode is a runtime and harness variable.
The shipped default is BGE-small with `alias_variant`, while the prior BGE-base
candidate also uses `alias_variant`. The harness can still reproduce `no_alias`
and `current_alias` without hand-editing code.

## Tuning Summary

| Area | Finding |
| --- | --- |
| ONNX BGE-base sessions | Stage 2 showed s4 can be faster, but Stage 4 repeats did not preserve a quality or speed advantage over s2. Keep s2/default session cap for now. |
| ONNX BGE-base batch | Batch 128 remains the comparison and default value. Batch 256 did not improve quality and was not clearly faster in the clean tuning run. |
| llama.cpp BGE-base batch/parallelism | `r4/np4` remains the stable parallel shape. b512/r4 is the new score leader at average 791.9135, while b128/r4 remains the fastest stable profile at about 435.88 docs/sec versus b512/r4 at 434.30. b768/r4 opened well but failed repeat stability; b1024/r4 and b384/r4 regressed MRR. r5 improved docs/sec to about 441 but regressed MRR below the r4 leader bands. The earlier `r1/np1` MRR bump did not repeat. |
| 60/30/10 compact-storage profile | With compact scaled-int8 persisted vectors, b768/r4 supersedes b896 and b640 for the current pipeline metric: run 92 added llama.cpp `-ub 1024` and improved the local score to 882198.781327, then cross-repo run `20260423024405` passed the four-repo profile gate. Keep the family opt-in until a default flip is explicitly accepted. |
| llama.cpp BGE-base doc mode | Leader-shape `current_alias` and `no_alias` both lost Persistent Hit@10 and trailed the `alias_variant` leader. Keep `alias_variant` for BGE-base llama.cpp. |
| BGE-small | Shipped default profile remains BGE-small with compact aliases. The crossed `scope=all`, `no_alias`, b256 row is now a promotion candidate after three stable full-query repeats, but not yet the runtime default. |
| Run 2 retrieval | `scope=all` was the largest BGE-small quality lever; durable `no_alias` beat durable aliases; hybrid-weight tweaks did not improve quality. Original speed rows are invalid because the GPU was locked; fail-hard DirectML speedchecks restored the default row to about 17-21s index time. |
| Qwen3 0.6B | Strong MRR but lower Hit@10 and about 4x slower than ONNX BGE-base in finalists. Keep as an experiment. |
| Prompt/pooling sweeps | BGE code-query prefix hurt MRR and Hit@10. BGE no-query-prefix hurt ONNX MRR. BGE mean pooling improved Hit@10 but reduced primary MRR. Qwen symbol instruction hurt MRR. |
| Nomic v1.5 prompt sweep | Retest resolved the old no-prefix crash. No-prefix is valid but worse; the prefixed/current-profile row repeated stably at average score 753.9136, MRR@10 0.6407, Hit@10 0.8857, Persistent Hit@10 0, and 308.11 docs/sec. Keep Nomic prefixes. |
| Nomic v2 MoE | Blocked until semantic docs enforce a token budget; prior alias docs exceeded llama.cpp's 512-token cap. |

## Artifact Roots

| Stage | Artifact root | Purpose |
| --- | --- | --- |
| Smoke | `target/embedding-research/smoke-20260419` | One ONNX DirectML row and one llama.cpp Vulkan row; verified no CPU fallback. |
| Stage 1 alias | `target/embedding-research/alias-stage1-20260419` | Alias mode comparison across the main candidate set. |
| Stage 2 tuning | `target/embedding-research/tuning-stage2-clean-20260419` | Clean batch/session/parallelism tuning subset. |
| Stage 3 prompt | `target/embedding-research/prompt-stage3-20260419` | Prompt, pooling, context, and profile-specific toggles. |
| Stage 4 finalists | `target/embedding-research/finalists-stage4-20260419` | Two-repeat finalist run used for the accepted table. |
| Run 2 controls | `target/embedding-research/controls-run2-20260419` | Expanded 70-query controls for default, prior BGE-base candidates, and fast BGE-small. |
| Run 2 retrieval | `target/embedding-research/retrieval-run2-20260419` | Hybrid-weight, semantic-scope, and alias-mode isolation for BGE-small; quality usable, speed invalid because GPU was locked. |
| Run 2 retrieval speedchecks | `target/embedding-research/retrieval-run2-speedcheck-normal-20260419`, `target/embedding-research/retrieval-run2-speedcheck-failhard-20260419`, `target/embedding-research/retrieval-run2-speedcheck-confirmed-gpu-20260419` | One-row default-shape checks showing slow invalid rows, then restored GPU-fast DirectML rows after provider fail-hard and GPU availability. |
| Run 2 dimension continuation | `target/embedding-research/autoresearch-dimension-20260421T042937Z`, `target/embedding-research/autoresearch-qwen-dim512-20260421T043705Z`, `target/embedding-research/autoresearch-nomic-v15-dim256-20260421T044024Z`, `target/embedding-research/autoresearch-qwen-dim1024-20260421T052914Z`, `target/embedding-research/autoresearch-gemma-dim768-20260421T053243Z`, `target/embedding-research/autoresearch-nomic-v15-dim768-20260421T053515Z` | Bounded persistent/alias query probes for EmbeddingGemma, Qwen3, and Nomic v1.5 dimensions. All were provider-valid but failed the Hit@10 gate. Qwen 512/1024 were strongest at MRR@10 0.4389 and Hit@10 0.6667 but slow; Gemma 768 and Nomic 768 regressed their 256-dim MRR signals. No dimension-only row is a promotion candidate. |
| Run 2 BGE-small bounded repeats | `target/embedding-research/autoresearch-bge-small-default-slice-20260421T050147Z`, `target/embedding-research/autoresearch-bge-small-crossed-slice-20260421T050249Z`, `target/embedding-research/autoresearch-bge-small-crossed-slice-repeat2-20260421T050406Z`, `target/embedding-research/autoresearch-bge-small-crossed-slice-repeat3-20260421T050521Z` | Persistent/alias slice control plus three crossed-candidate repeats. Current default: MRR@10 0.1139, Hit@10 0.5556. Crossed BGE-small `scope=all`, `no_alias`, b256 repeated stably at MRR@10 0.2420, Hit@10 0.6667, Persistent Hit@10 0.25, average docs/sec about 789.62. Best bounded BGE-small lead, but still below the Hit@10 promotion gate. |
| Run 2 BGE-small full-query repeats | `target/embedding-research/autoresearch-bge-small-crossed-full-run1-20260421T051205Z`, `target/embedding-research/autoresearch-bge-small-crossed-full-run2-20260421T051606Z`, `target/embedding-research/autoresearch-bge-small-crossed-full-run3-20260421T051948Z` | Three full 70-query repeats for crossed BGE-small `scope=all`, `no_alias`, b256. Quality was identical across repeats: MRR@10 0.6089, Hit@10 0.9000, Hit@1 0.4571, Persistent Hit@10 0.25. Average docs/sec was about 786.52 and average index time was about 28.63s. This is promotion-candidate evidence, not a default change by itself. |
| Run 2 Nomic v2 token-budget probes | `target/embedding-research/autoresearch-nomic-v2-dim256-doc320-r1-20260421T060123Z`, `target/embedding-research/autoresearch-nomic-v2-dim256-doc320-proxy-r1-20260421T060752Z`, `target/embedding-research/autoresearch-nomic-v2-dim768-doc320-proxy-r1-20260421T061030Z` | Whitespace-only budgets still overflowed the 512-token llama.cpp context, so Run 2 added a conservative identifier-aware token proxy behind `CODESTORY_SEMANTIC_DOC_MAX_TOKENS`. That unblocked Nomic v2, but 256 dim remained weak and 768 dim was worse. |
| Run 2 ONNX dynamic-int8 probes | `target/embedding-research/autoresearch-bge-small-onnx-int8-slice-20260421T061605Z`, `target/embedding-research/autoresearch-bge-small-fast-int8-slice-20260421T061831Z`, `target/embedding-research/autoresearch-bge-base-onnx-int8-slice-20260421T062039Z` | Local dynamic-int8 ONNX artifacts ran provider-verified. BGE-small default-shape and crossed-shape rows both collapsed in MRR/Hit@1, and BGE-base int8 stayed below gate while also slowing indexing. Treat ONNX dynamic int8 as measured-negative for now. |
| Run 2 BGE-small stored-vector quantization | `target/embedding-research/ar-vq-f32-win-slice-20260421T155234Z`, `target/embedding-research/ar-vq-int8-win-slice-20260421T155643Z`, `target/embedding-research/ar-vq-uint8-win-slice-20260421T160036Z`, `target/embedding-research/ar-vq-bin-win-slice-20260421T160429Z` | Bounded persistent/alias slice rows for the BGE-small winner shape after adding quantized prefilter plus full-precision rescoring. Float32, int8, and uint8 all preserved MRR@10 0.2531, Hit@10 0.6667, and Persistent Hit@10 0.25; binary regressed MRR to 0.2269. A same-shape alias-heavy b128 control showed that the earlier int8 timeout/regression was a bad profile/control choice, not isolated byte-quantization damage. Current ONNX DirectML timing was cold-slow (about 187-203s index time), so treat these as rank/footprint evidence and rerun speed/full-query rows after GPU health is restored. |
| Run 2 BGE-base Q8 r2/np2 compression baseline | `target/embedding-research/autoresearch-q8-baseline-bge-base-r2np2-full-20260421T122812Z` | Same-parallelism baseline for the Q6_K probe. Q8 scored 790.5192 with MRR@10 0.6601, Hit@10 0.9286, Persistent Hit@10 0.25, 340.44 docs/sec, and 48.21s index time. |
| Run 2 BGE-base Q6_K requantization probe | `target/embedding-research/autoresearch-weight-quant-bge-base-q6-requant-full-20260421T122316Z` | Local Q6_K GGUF was generated by Q8-to-Q6 requantization with `llama-quantize --allow-requantize`. The artifact shrank to 86.75 MiB by quantizer report, but the full-query row scored 774.859 with Hit@10 0.9143, Persistent Hit@10 0, 310.04 docs/sec, and 53.38s index time. Compared with the Q8 r2/np2 baseline, the regression is attributable to the requantized artifact. |
| Run 2 BGE-base Q6_K leader-shape requantization probe | `target/embedding-research/autoresearch-weight-quant-bge-base-q6-r4-requant-full-20260421T123319Z` | Same local Q6_K artifact under the leader-aligned r4/np4 shape. Throughput improved to 400.72 docs/sec and 42.71s index time, but quality fell to score 770.0178, MRR@10 0.6529, Hit@10 0.9143, Hit@1 0.5143, and Persistent Hit@10 0. Stop Q8-to-lower-bit requantization; use a clean source artifact or imatrix-aware recipe before reopening GGUF quantization. |
| Run 2 BGE-base clean-source GGUF quantization probes | `target/embedding-research/autoresearch-weight-quant-bge-base-q6-f16-source-r4-full-20260421T124159Z`, `target/embedding-research/autoresearch-weight-quant-bge-base-q5-f16-source-r4-full-20260421T124504Z`, `target/embedding-research/ar-q5-r2-20260421T131218Z`, `target/embedding-research/ar-q5-r3-20260421T131502Z`, `target/embedding-research/autoresearch-weight-quant-bge-base-q4-f16-source-r4-full-20260421T124935Z` | F16-derived Q6_K, Q5_K_M, and Q4_K_M under the leader-aligned r4/np4 shape. Q5_K_M is the only compression candidate and repeated stably at average score 791.5032 with MRR@10 0.6605, Hit@10 0.9286, Persistent Hit@10 0.25, 395.01 docs/sec, and 43.11s index time. Q6_K and Q4_K_M both lost Hit@10 or persistent-hit, so they are compression-boundary evidence rather than promotion rows. A first Q5 repeat using the long output label failed at staged SQLite open; short labels avoided the Windows path pressure. |
| Run 2 BGE-base Q5_K_M b512/r4 repeats | `target/embedding-research/ar-q5-b512-r1-20260421T162402Z`, `target/embedding-research/ar-q5-b512-r2-20260421T162659Z`, `target/embedding-research/ar-q5-b512-r3-20260421T162950Z` | The clean-source Q5_K_M artifact rerun under the b512/r4 leader shape. It matched Q8 b512/r4 ranking quality exactly across three full-query rows: MRR@10 0.6605, Hit@10 0.9286, Hit@1 0.5286, Persistent Hit@10 0.25, with average score 791.6327, 407.00 docs/sec, and 41.54s index time. This is the compressed deployment candidate below the Q8 speed/score leader. |
| Run 2 BGE-small GGUF Q5/Q8 controls | `target/embedding-research/ar-bges-q5-20260421T135732Z`, `target/embedding-research/ar-bges-q8-20260421T140149Z` | Clean-source BGE-small Q5_K_M from the CompendiumLabs F16 source scored 737.9803 with MRR@10 0.6214, Hit@10 0.9000, Hit@1 0.5000, Persistent Hit@10 0, and 467.31 docs/sec. The same-backend Q8_0 control scored 739.8364 with MRR@10 0.6233, Hit@10 0.9000, Hit@1 0.5000, Persistent Hit@10 0, and 470.64 docs/sec. Q5 is close to Q8, but both are below the ONNX BGE-small finalist and lose the persistent bucket, so pause BGE-small GGUF lower-bit rows. |
| Run 2 Nomic v1.5 prompt retest | `target/embedding-research/ar-nomic-nop-20260421T141226Z`, `target/embedding-research/ar-nomic-pref-20260421T141556Z`, `target/embedding-research/ar-nomic-pref-r2-20260421T141905Z`, `target/embedding-research/ar-nomic-pref-r3-20260421T142225Z` | The old Nomic no-prefix crash is now resolved. No-prefix scored 714.5411 with MRR@10 0.6028, Hit@10 0.8714, Hit@1 0.4429, Persistent Hit@10 0, and 310.41 docs/sec. The prefixed/current-profile row repeated stably across three full-query runs: average score 753.9136, MRR@10 0.6407, Hit@10 0.8857, Hit@1 0.5143, Persistent Hit@10 0, and 308.11 docs/sec. Keep prefixes; Nomic v1.5 is a repeat-stable fallback, not a leader. |
| Run 2 BGE-base llama.cpp doc-mode variants | `target/embedding-research/ar-bgeb-current-alias-20260421T142925Z`, `target/embedding-research/ar-bgeb-no-alias-20260421T143220Z` | Full-query leader-shape checks for BGE-base llama.cpp/Vulkan. `current_alias` scored 743.7298 and `no_alias` scored 772.1050; both lost Persistent Hit@10 and trailed the repeat-stable `alias_variant` leader. Keep `alias_variant` for the BGE-base llama.cpp leader. |
| Run 2 BGE-base llama.cpp b256/r4 repeats | `target/embedding-research/ar-bgeb-b256-r4-20260421T143819Z`, `target/embedding-research/ar-bgeb-b256-r4-r2-20260421T144120Z`, `target/embedding-research/ar-bgeb-b256-r4-r3-20260421T144414Z` | Three full-query repeats for BGE-base llama.cpp/Vulkan `scope=all`, `alias_variant`, b256, r4/np4, ctx4096, cls pool. Quality was stable at MRR@10 0.6605, Hit@10 0.9286, Hit@1 0.5286, Persistent Hit@10 0.25, with average score 791.8120, average docs/sec about 424.27, and average index time about 42.74s. This was the prior score leader before b512/r4 repeated higher. |
| Run 2 BGE-base llama.cpp b512/r4 repeats | `target/embedding-research/ar-bgeb-b512-r4-20260421T145844Z`, `target/embedding-research/ar-bgeb-b512-r4-r2-20260421T150215Z`, `target/embedding-research/ar-bgeb-b512-r4-r3-20260421T150640Z` | Three full-query repeats for BGE-base llama.cpp/Vulkan `scope=all`, `alias_variant`, b512, r4/np4, ctx4096, cls pool. Quality was stable at MRR@10 0.6605, Hit@10 0.9286, Hit@1 0.5286, Persistent Hit@10 0.25, with average score 791.9135, average docs/sec about 434.30, and average index time about 41.14s. This is the current score leader; b128/r4 remains the fastest stable throughput profile. |
| Run 2 BGE-base llama.cpp adjacent batch checks | `target/embedding-research/ar-bgeb-b768-r4-20260421T151228Z`, `target/embedding-research/ar-bgeb-b768-r4-r2-20260421T151518Z`, `target/embedding-research/ar-bgeb-b1024-r4-20260421T151841Z`, `target/embedding-research/ar-bgeb-b384-r4-20260421T152213Z` | Boundary/interpolation checks around the b512/r4 leader. b768/r4 first scored 791.9546 with the leader quality shape, but repeat 2 fell to score 791.8188 and 424.86 docs/sec. b1024/r4 scored 790.2636 with MRR@10 0.6589, and b384/r4 scored 790.6939 with MRR@10 0.6593. Do not continue batch-size probing unless runtime conditions change. |
| Run 2 BGE-base llama.cpp r5 frontier checks | `target/embedding-research/ar-bgeb-b256-r5-20260421T145150Z`, `target/embedding-research/ar-bgeb-b128-r5-20260421T145501Z` | Single full-query checks for the adjacent r5/np5 edge. b256/r5 reached 440.98 docs/sec and b128/r5 reached 441.89 docs/sec, but both regressed MRR below the r4 leaders while keeping Hit@10 0.9286 and Persistent Hit@10 0.25. Treat r5 as measured-negative unless the runtime or llama.cpp version changes. |
| Run 2 finalists2 BGE-base ONNX repeats | `target/embedding-research/autoresearch-finalists2-bge-base-quality-full-run1-short-20260421T115554Z`, `target/embedding-research/autoresearch-finalists2-bge-base-quality-full-run2-20260421T120049Z`, `target/embedding-research/autoresearch-finalists2-bge-base-quality-full-run3-20260421T120546Z` | Three full-query repeats for BGE-base ONNX `scope=all`, `alias_variant`, b128/s2. Quality was stable at MRR@10 0.6593, Hit@10 0.9286, Hit@1 0.5286, Persistent Hit@10 0.25, with average docs/sec about 303.75 and average index time about 53.25s. The first unshortened run hit a Windows staged SQLite path-length/open failure and is tracked as crash provenance, not a repeat row. |
| Run 2 finalists2 BGE-base llama.cpp repeats | `target/embedding-research/autoresearch-finalists2-llama-bge-base-throughput-full-run1-20260421T121058Z`, `target/embedding-research/autoresearch-finalists2-llama-bge-base-throughput-full-run2-20260421T121358Z`, `target/embedding-research/autoresearch-finalists2-llama-bge-base-throughput-full-run3-20260421T121654Z` | Three full-query repeats for BGE-base llama.cpp/Vulkan `scope=all`, `alias_variant`, b128, r4/np4, ctx4096, cls pool. Quality was stable at MRR@10 0.6601, Hit@10 0.9286, Hit@1 0.5286, Persistent Hit@10 0.25, with average docs/sec about 435.88 and average index time about 40.50s. This is the current balanced quality/throughput leader. |
| 60/30/10 compact-storage local promotion | `target/autoresearch/indexer-embedder/20260423T015924`, `target/autoresearch/indexer-embedder/20260423T020818`, `target/autoresearch/indexer-embedder/20260423T022445`, `target/autoresearch/indexer-embedder/20260423T024103` | Versioned scaled-int8 persisted embeddings recovered perfect local quality and kept the compact footprint. The latest b768 `-ub 1024` run scores `pipeline_score` 882198.781327 with MRR@10 1.0, Hit@1 1.0, Hit@10 1.0, speed_component 0.670088925, footprint_component 0.811721039, 360.15 docs/sec, and 777 vector bytes per doc after compacting the scaled header. b896, b640, Q5_K_M, and the 384-token doc-budget scout were lower. |
| 60/30/10 compact-storage cross-repo gate | `target/autoresearch/cross-repo-promotion/20260423022731`, `target/autoresearch/cross-repo-promotion/20260423024405` | BGE-base b768/r4 with `CODESTORY_STORED_VECTOR_ENCODING=int8` passed freelancer, traderotate, the-green-cedar, and Sourcetrail with 225 total queries and 100 adversarial queries. The latest `-ub 1024` gate scored aggregate Hit@10 1.0, MRR@10 0.826936508, Hit@1 0.724444444, adversarial Hit@10 1.0, adversarial MRR@10 0.834511905, p95 89.576 ms, and no misses. Sourcetrail covered 150 queries over the large mixed C++/Java/Python repository. |
| Superseded raw run | `target/embedding-research/tuning-stage2-20260419` | Kept as provenance only; it mixed selected IDs across stages before the harness selection bug was fixed. |

Each current run writes `results.csv`, `results.json`, `query-ranks.csv`,
`alias-comparisons.csv`, `cases.json`, `queries.json`, raw per-case logs, and a
Markdown report. New finalist/repeat runs also write `repeat-summary.csv`.
Run 2 additionally writes `manifest.json` and `sources.md`; see
[embedding-research-run-2.md](embedding-research-run-2.md). Current result rows
include `provider_requested`, `provider_verified`, and `provider_evidence`;
unverified rows are not decision-grade even if they contain metric columns.

## Recovered Evidence

Historical rows are useful provenance, but they do not decide defaults:

- Smaller early runtime-backed quality runs favored `embeddinggemma-300m` by
  MRR@10, then `nomic-embed-text-v2-moe`, then `nomic-embed-text-v1.5`; BGE-base
  was weak in that mixed setting.
- Earlier alias comparisons were inconclusive: BGE-base, Qwen, Nomic v1.5, and
  MiniLM improved in some metrics, while BGE-small and EmbeddingGemma regressed.
- Earlier mixed-batch rows were useful for discovering candidates but are not
  ranked against the fair GPU-only rows.
- CPU rows remain provenance only. CPU and GPU quality should normally match for
  the same model and math, but CPU rows are not accepted for CodeStory default
  decisions.

## Rerun Contract

Decision-grade rows must follow this contract:

- Use `scripts/embedding-gpu-fair-benchmark.mjs` or its successor.
- Start with the source-led stage contract in
  [embedding-research-run-2.md](embedding-research-run-2.md) before adding new
  tuning cases.
- Update the Candidate Coverage Matrix whenever a model profile, backend,
  quantized artifact family, or blocked candidate is added to the harness.
- Use the CodeStory runtime indexing path, not a standalone embedding probe.
- Use isolated cache directories per row under `target/embedding-research/<run>/`.
- Keep one comparison table to one semantic doc mode and one batch-size policy,
  or explicitly mark mixed rows as tuning/provenance.
- For ONNX rows, use DirectML or CUDA and reject CPU-provider fallback.
- For llama.cpp rows, require a GPU device log plus full model-layer offload.
- Treat only rows with `provider_verified=true` as ranked decision evidence.
- Keep future BGE-base GGUF `weight-quant` rows aligned with the finalists2
  llama.cpp/Vulkan leader shape: b128, r4/np4, ctx4096, and cls pooling.
- Store raw logs and per-query ranks. Do not collapse the result to only the
  final score.
- Record obvious host-load anomalies, GPU availability problems, and timing
  drifts. When a same-shape row diverges materially from a prior run, rerun a
  one-row speedcheck and compare phase timings before making speed or cost
  claims.
- Preserve shortened `artifact_case_dir` names for long case ids on Windows so
  staged SQLite cache paths stay openable, while keeping full `case_id` values
  in CSV/JSON outputs.
- Treat skipped rows as real research findings when the blocker is missing
  artifacts, absent vector-quant storage support, or an incompatible model
  context window.

Useful commands:

```powershell
cargo build --release -p codestory-cli --features codestory-runtime/onnx-directml

$env:CODESTORY_EMBED_RESEARCH_STAGE = 'smoke'
node scripts/embedding-gpu-fair-benchmark.mjs

$env:CODESTORY_EMBED_RESEARCH_STAGE = 'source-scan'
node scripts/embedding-gpu-fair-benchmark.mjs

$env:CODESTORY_EMBED_RESEARCH_STAGE = 'controls'
node scripts/embedding-gpu-fair-benchmark.mjs

$env:CODESTORY_EMBED_RESEARCH_STAGE = 'retrieval'
$env:CODESTORY_EMBED_RESEARCH_LIST = '1'
node scripts/embedding-gpu-fair-benchmark.mjs

$env:CODESTORY_EMBED_RESEARCH_STAGE = 'bge-small-candidate'
node scripts/embedding-gpu-fair-benchmark.mjs

$env:CODESTORY_EMBED_RESEARCH_STAGE = 'alias'
node scripts/embedding-gpu-fair-benchmark.mjs

$env:CODESTORY_EMBED_RESEARCH_STAGE = 'tuning'
$env:CODESTORY_EMBED_RESEARCH_LIST = '1'
node scripts/embedding-gpu-fair-benchmark.mjs
```

For bounded tuning, set `CODESTORY_EMBED_RESEARCH_CASES` to a comma-separated
case-id list from the list command. The runner scopes selected case IDs to the
requested stage before execution, which avoids mixing alias-stage and
tuning-stage rows with identical settings.
For quick autoresearch probes, also set `CODESTORY_EMBED_RESEARCH_QUERY_LIMIT`,
`CODESTORY_EMBED_RESEARCH_QUERY_IDS`, or `CODESTORY_EMBED_RESEARCH_QUERY_BUCKETS`
to keep a run inside a small wall-clock cap. Treat those rows as exploratory;
promotion decisions still require the full query suite.

## Remaining Research Backlog

This is the planned follow-up work, in priority order:

| Priority | Work | Why |
| ---: | --- | --- |
| 1 | Decide whether to flip the runtime default to the promoted BGE-base b768/r4 scaled-int8 profile or keep it as an explicit opt-in profile. | The 60/30/10 pipeline segment passed local and four-repo gates with perfect local retrieval, real persisted-footprint savings, and no external misses, but it still adds a llama.cpp sidecar and 768-dimensional vectors. |
| 2 | Decide whether to change the BGE-small runtime default, then run the repo-scale gate if accepted. | Crossed `scope=all`, `no_alias`, b256 passed three full-query repeats, but changing defaults should be an explicit product/runtime decision rather than an autoresearch side effect. |
| 3 | Decide whether the runtime should expose clean-source BGE-base Q5_K_M b512/r4 as a compressed-quality option, then run the repo-scale gate if accepted. | Clean F16-derived Q5_K_M repeated stably under the b512/r4 leader shape and matched Q8 ranking quality while reducing the artifact to 77.49 MiB by quantizer report, but throughput and score stayed lower than Q8. Q6_K and Q4_K_M lost Hit@10 or persistent-hit, so do not push below Q5 without a new imatrix or calibration recipe. |
| 4 | Rerun any retrieval row whose speed/cost matters under fail-hard DirectML provider selection. | The original retrieval run has usable quality deltas but invalid speed data because the GPU was locked. |
| 5 | Reopen BGE-small or ONNX vector-quant rows only after DirectML speed is stable. | The promoted compact-storage row is BGE-base llama.cpp/Vulkan scaled int8; the older same-shape BGE-small vector rows preserved byte-quantization quality but ran in the old GPU-locked/cold-slow timing band. |
| 6 | Revisit Nomic v2 only if semantic-doc/query shape changes materially. | The token-budget blocker is resolved for research, but 256/768-dimensional bounded rows were provider-valid and below gate. |
| 7 | Pause dimension-only probes unless semantic docs or query shape changes first. | Provider-valid 256/512/768/1024 bounded probes for Nomic v1.5, Qwen3, and EmbeddingGemma did not pass the Hit@10 gate; larger dimensions did not rescue Gemma or Nomic. |
| 8 | Promote only repeat-stable, provider-verified rows into `finalists2`. | Defaults should not change from a single mixed tuning pass or from artifacts that lack provider provenance. |
