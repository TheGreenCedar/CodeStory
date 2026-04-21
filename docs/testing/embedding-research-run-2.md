# Embedding Research Run 2

This run broadens embedding work beyond model swaps and batch tuning. It keeps
the benchmark grounded in CodeStory's runtime indexing path while testing
separate optimization lanes: model-weight quantization, stored-vector
quantization, Matryoshka-style dimension shortening, semantic-doc shape, and
hybrid retrieval weights.

## Source Scan

Primary sources used to define the lanes:

| Source | Why it matters |
| --- | --- |
| [Sentence Transformers embedding quantization](https://www.sbert.net/docs/package_reference/util/quantization.html) | Separates embedding/vector quantization from model-weight quantization and documents float32, int8, uint8, binary, and ubinary encodings plus rescoring. |
| [ONNX Runtime quantization](https://onnxruntime.ai/docs/performance/model-optimizations/quantization.html) | Documents dynamic/static int8 quantization and selected int4 weight-only quantization, including accuracy-debugging caveats. |
| [ONNX Runtime DirectML provider](https://onnxruntime.ai/docs/execution-providers/DirectML-ExecutionProvider.html) | DirectML cannot run parallel calls on one session, which explains the multi-session benchmark design. |
| [llama.cpp quantize](https://github.com/ggml-org/llama.cpp/blob/master/tools/quantize/README.md) | Defines GGUF weight-quantization workflow, common quant types, and imatrix calibration. |
| [Qdrant quantization](https://qdrant.tech/documentation/manage-data/quantization/) | Confirms stored-vector quantization is its own search/storage lane. |
| [Nomic v1.5 model card](https://huggingface.co/nomic-ai/nomic-embed-text-v1.5) | Documents Matryoshka dimensionality tradeoffs at 768, 512, 256, 128, and 64 dimensions. |
| [Nomic v2 MoE model card](https://huggingface.co/nomic-ai/nomic-embed-text-v2-moe) | Documents required query/document prefixes, a 512-token maximum input length, and Matryoshka truncation such as 256 dimensions; Run 2 now uses an opt-in semantic-doc budget for local testing. |
| [Qwen3 0.6B model card](https://huggingface.co/Qwen/Qwen3-Embedding-0.6B) | Documents 32k context, instruction-aware retrieval, MRL support, and user-defined output dimensions from 32 to 1024. |
| [EmbeddingGemma model card](https://huggingface.co/google/embeddinggemma-300m) | Documents 768-dimensional output with 512, 256, and 128 dimension Matryoshka truncation followed by renormalization. |

## Harness Contract

Use `scripts/embedding-gpu-fair-benchmark.mjs`. Every run writes:

- `manifest.json`: source ledger, selected cases, artifact paths, model sizes, vector bytes per doc, and exact stage/case metadata.
- `sources.md`: source scan and blocked candidates.
- `results.csv` / `results.json`: row-level metrics, skipped rows, provider validation, quality gate, component scores, and combined score.
- `query-ranks.csv`: per-query ranks so persistent misses remain inspectable.
- `repeat-summary.csv`: repeat averages for stages with repeated cases.
- per-case logs under each case directory.

Decision-grade rows still require `provider_verified=true`: DirectML/CUDA
fail-hard validation for ONNX rows and Vulkan0 plus full layer offload for
llama.cpp rows. A row without provider validation is provenance only, even when
it has metric columns.

## Stages

| Stage | Purpose | Run command |
| --- | --- | --- |
| `source-scan` | Write manifest and source ledger without running model cases. | `$env:CODESTORY_EMBED_RESEARCH_STAGE='source-scan'; node scripts/embedding-gpu-fair-benchmark.mjs` |
| `controls` | Three-repeat baselines for the shipped default, prior BGE-base candidates, and fast BGE-small. | `$env:CODESTORY_EMBED_RESEARCH_STAGE='controls'; node scripts/embedding-gpu-fair-benchmark.mjs` |
| `weight-quant` | GGUF and ONNX quantized-weight candidates. Missing generated artifacts become skipped rows, not false failures. | `$env:CODESTORY_EMBED_RESEARCH_STAGE='weight-quant'; node scripts/embedding-gpu-fair-benchmark.mjs` |
| `vector-quant` | Stored-vector quantization lane for the BGE-small winner shape. Runs float32 control plus int8/uint8/binary/ubinary corpus encodings through quantized prefiltering and full-precision rescoring. | `$env:CODESTORY_EMBED_RESEARCH_STAGE='vector-quant'; node scripts/embedding-gpu-fair-benchmark.mjs` |
| `dimension` | Source-backed Matryoshka/dimension rows for Nomic v1.5, Nomic v2 MoE, Qwen3 0.6B, and EmbeddingGemma; BGE-small stays as a negative control. | `$env:CODESTORY_EMBED_RESEARCH_STAGE='dimension'; node scripts/embedding-gpu-fair-benchmark.mjs` |
| `retrieval` | Hybrid weight, semantic scope, and alias-mode sweeps. | `$env:CODESTORY_EMBED_RESEARCH_STAGE='retrieval'; node scripts/embedding-gpu-fair-benchmark.mjs` |
| `bge-small-candidate` | Three-repeat current default versus crossed BGE-small `scope=all`, `no_alias`, b256. | `$env:CODESTORY_EMBED_RESEARCH_STAGE='bge-small-candidate'; node scripts/embedding-gpu-fair-benchmark.mjs` |
| `finalists2` | Three-repeat comparison of selected candidates after earlier stages identify them. | `$env:CODESTORY_EMBED_RESEARCH_STAGE='finalists2'; node scripts/embedding-gpu-fair-benchmark.mjs` |

Use `CODESTORY_EMBED_RESEARCH_LIST=1` to list case IDs before running a stage.
Use `CODESTORY_EMBED_RESEARCH_CASES=<case-id,...>` for bounded reruns.
For short exploratory loops, set `CODESTORY_EMBED_RESEARCH_QUERY_LIMIT=<n>`,
`CODESTORY_EMBED_RESEARCH_QUERY_IDS=<id,...>`, or
`CODESTORY_EMBED_RESEARCH_QUERY_BUCKETS=<bucket,...>`. Query-sliced runs are
useful for two-minute autoresearch probes, but the full query suite is still
required before promoting a model, backend, or semantic-doc default.

On Windows, long case ids can exceed practical staged SQLite path limits once
the cache path is nested under an artifact root. The harness therefore writes
long cases under a shortened `artifact_case_dir` while preserving the full
`case_id` in reports and manifests.

After finalists2, BGE-base GGUF `weight-quant` rows are aligned to the measured
llama.cpp/Vulkan leader shape: CodeStory request count `4`, server `-np 4`,
`ctx4096`, and `--pooling cls`. Older r2/np2 Q8/Q6 rows remain useful as the
same-shape interpretation baseline for the Q8-to-Q6 requantization probe, but
future clean GGUF quantization should compare against the r4/np4 leader.

## Scoring

The runner now uses a gated score instead of the old MRR/speed-only score:

- quality gate: `Hit@10 >= 0.75`
- combined score: `0.50 * quality + 0.25 * speed + 0.15 * footprint + 0.10 * reliability`
- quality blends normalized MRR@10, Hit@10, Hit@1, and persistent-miss Hit@10
- footprint uses model size when the artifact exists and records vector bytes per doc for dimension/vector experiments

Do not change a default from one run. A candidate must pass the quality gate,
avoid material persistent-miss regressions, have provider-verified artifacts,
and hold up across finalist repeats.

## Stop Rules

- Stop a lane when missing artifacts, unsupported storage format, or context
  limits are the blocker; record it as skipped instead of tuning around it.
- Qwen3 0.6B and EmbeddingGemma dimension rows are now source-backed. If a row
  fails, treat it as a local runtime/backend compatibility finding rather than
  a reason to remove the source-backed lane.
- Treat vector quantization as blocked until the store/search layer can preserve
  quantized corpus vectors and rescore against original vectors.
- Rerun `controls` after any harness ranking change before comparing new lanes.

## 2026-04-21 Continuation Notes

- Nomic v2 is no longer merely blocked. Run 2 added
  `CODESTORY_SEMANTIC_DOC_MAX_TOKENS` and changed the research budget to charge
  long identifier-heavy fragments more conservatively. After that, Nomic v2
  produced provider-verified bounded rows, but 256 dim scored MRR@10 0.3086 /
  Hit@10 0.5556 and 768 dim regressed to MRR@10 0.2222 / Hit@10 0.4444.
- Dynamic-int8 ONNX artifacts were generated locally for BGE-small and BGE-base.
  BGE-small int8 collapsed in both default-shape and crossed fast-profile rows;
  BGE-base int8 preserved more Hit@10 but still scored only MRR@10 0.2593 /
  Hit@10 0.6667 while indexing slowly. Treat ONNX dynamic int8 as
  measured-negative until a different quantization recipe is introduced.
- Finalists2 promoted BGE-base llama.cpp/Vulkan above the previous ONNX quality
  lead. ONNX BGE-base `scope=all`, `alias_variant`, b128/s2 repeated at average
  score 789.1745, MRR@10 0.6593, Hit@10 0.9286, Hit@1 0.5286, Persistent
  Hit@10 0.25, 303.75 docs/sec, and 53.25s index time. The llama.cpp/Vulkan
  BGE-base `r4/np4`, `ctx4096`, `--pooling cls` candidate repeated at average
  score 791.5879, MRR@10 0.6601, Hit@10 0.9286, Hit@1 0.5286, Persistent
  Hit@10 0.25, 435.88 docs/sec, and 40.50s index time. Treat it as the current
  balanced quality/throughput leader, not as an automatic runtime default
  change.
- Long finalists2 case ids now use shortened artifact case directories when
  needed. This preserves full `case_id` values in reports while avoiding
  Windows staged SQLite cache open failures under deeply nested artifact roots.
- A local BGE-base Q6_K GGUF was generated from the existing Q8 GGUF with
  `llama-quantize --allow-requantize`. It reduced the quantizer-reported model
  size from 111.78 MiB to 86.75 MiB, but the full-query row regressed to score
  774.859, MRR@10 0.6588, Hit@10 0.9143, Persistent Hit@10 0, 310.04 docs/sec,
  and 53.38s index time. A same-parallelism Q8 r2/np2 control scored 790.5192,
  MRR@10 0.6601, Hit@10 0.9286, Persistent Hit@10 0.25, 340.44 docs/sec, and
  48.21s index time, so the Q6 loss is attributable to the requantized artifact
  rather than r2/np2 alone. The leader-aligned r4/np4 Q6 row recovered some
  throughput at 400.72 docs/sec and 42.71s index time, but quality fell further:
  score 770.0178, MRR@10 0.6529, Hit@10 0.9143, Hit@1 0.5143, Persistent
  Hit@10 0. Treat these as negative Q8-requantization baselines, not as
  evidence against clean F16/F32-derived GGUF quantization.
- Clean-source BGE-base GGUF quantization was then measured from the
  [CompendiumLabs/bge-base-en-v1.5-gguf](https://huggingface.co/CompendiumLabs/bge-base-en-v1.5-gguf)
  F16 artifact under the r4/np4 leader shape. Clean Q6_K scored 776.2653 with
  Hit@10 0.9143 and Persistent Hit@10 0. Clean Q5_K_M recovered the persistent
  bucket and repeated stably across three full-query rows: average score
  791.5032, MRR@10 0.6605, Hit@10 0.9286, Hit@1 0.5286, Persistent Hit@10 0.25,
  395.01 docs/sec, and 43.11s index time. Clean Q4_K_M fell back to score
  774.6363, MRR@10 0.6576, Hit@10 0.9143, Hit@1 0.5143, Persistent Hit@10 0,
  387.68 docs/sec, and 43.66s index time. Treat Q5_K_M as the only compressed
  BGE-base candidate from this lane; do not promote it without an owner/runtime
  decision and repo-scale gate.
- Clean-source BGE-small GGUF was then checked from the
  [CompendiumLabs/bge-small-en-v1.5-gguf](https://huggingface.co/CompendiumLabs/bge-small-en-v1.5-gguf)
  F16 artifact. Local llama.cpp b8840 quantization produced Q5_K_M at 27.96 MiB
  by quantizer report, with fallback quantization on 60 of 197 tensors. The
  full-query Q5_K_M row scored 737.9803, MRR@10 0.6214, Hit@10 0.9000,
  Hit@1 0.5000, Persistent Hit@10 0, 467.31 docs/sec, and 38.44s index time.
  The same-backend Q8_0 control scored 739.8364, MRR@10 0.6233, Hit@10 0.9000,
  Hit@1 0.5000, Persistent Hit@10 0, 470.64 docs/sec, and 40.11s index time.
  Q5 is close to Q8, so lower-bit quantization is not the primary problem; this
  BGE-small GGUF shape trails the ONNX BGE-small finalist and loses the
  persistent bucket. Pause BGE-small GGUF lower-bit rows unless a new
  pooling/doc-mode hypothesis or GGUF-only runtime need appears.
- The old Nomic v1.5 no-prefix prompt crash was cleared with the current
  short-label harness. No-prefix completed at score 714.5411, MRR@10 0.6028,
  Hit@10 0.8714, Hit@1 0.4429, Persistent Hit@10 0, 310.41 docs/sec, and
  52.93s index time. The matching prefixed/current-profile row was materially
  better and repeat-stable across three full-query runs: average score
  753.9136, MRR@10 0.6407, Hit@10 0.8857, Hit@1 0.5143, Persistent Hit@10 0,
  308.11 docs/sec, and 53.66s average index time. Keep Nomic prefixes; treat
  this as fallback evidence below BGE-base and not as a default change.
- BGE-base llama.cpp leader-shape doc-mode variants were measured and rejected.
  Under the same `scope=all`, b128, r4/np4, ctx4096, cls-pool shape,
  `current_alias` scored 743.7298 and `no_alias` scored 772.1050. Both lost
  Persistent Hit@10 and trailed the `alias_variant` leader at 791.6851, so keep
  aliases on for the current BGE-base llama.cpp leader profile.
- BGE-base llama.cpp b256/r4 repeated as a new score leader, not a clear
  throughput leader. Across three full-query rows it averaged score 791.8120,
  MRR@10 0.6605, Hit@10 0.9286, Hit@1 0.5286, Persistent Hit@10 0.25,
  424.27 docs/sec, and 42.74s index time. The previous b128/r4 row remains
  faster at about 435.88 docs/sec and 40.50s index time, so treat b256/r4 as a
  score candidate requiring owner/runtime decision, not an automatic default.
- BGE-base llama.cpp b512/r4 then repeated as the new score leader while
  preserving the same quality shape. Across three full-query rows it averaged
  score 791.9135, MRR@10 0.6605, Hit@10 0.9286, Hit@1 0.5286, Persistent
  Hit@10 0.25, 434.30 docs/sec, and 41.14s index time. Repeat 3 set the current
  single-run best at 792.0033 with checks passing. Treat b512/r4 as the current
  score candidate, still requiring owner/runtime decision and repo-scale gate.
- The clean-source BGE-base Q5_K_M artifact also repeated under the b512/r4
  leader shape. Across three full-query rows it preserved the same quality
  shape as Q8 b512/r4: average score 791.6327, MRR@10 0.6605, Hit@10 0.9286,
  Hit@1 0.5286, Persistent Hit@10 0.25, 407.00 docs/sec, and 41.54s index time.
  Treat Q5 b512/r4 as the compressed deployment candidate: smaller artifact and
  stable quality, but lower throughput and score than the Q8 b512/r4 leader.
- Adjacent batch-size frontier checks did not displace b512/r4. b768/r4 opened
  at score 791.9546 with the same quality shape, but repeat 2 fell to score
  791.8188 and 424.86 docs/sec. b1024/r4 regressed MRR@10 to 0.6589 and scored
  790.2636; b384/r4 regressed MRR@10 to 0.6593 and scored 790.6939. Stop
  batch-size probing for the current runtime/hardware.
- BGE-base llama.cpp r5 is measured-negative for this hardware/run. b256/r5
  improved docs/sec to 440.98 but dropped MRR@10 to 0.6593; b128/r5 reached
  441.89 docs/sec but dropped MRR@10 to 0.6589. Both kept Hit@10 0.9286 and
  Persistent Hit@10 0.25, but the MRR drop put them below the r4 leader bands.
- Long autoresearch output labels can still push staged SQLite paths over a
  Windows open limit even after case directories are shortened. The first Q5
  repeat label (`autoresearch-weight-quant-bge-base-q5-f16-source-r4-full-repeat2`)
  failed before inference with `Failed to open staged storage`; rerunning the
  same case as `ar-q5-r2` and `ar-q5-r3` completed normally. Keep future
  experiment labels short for full-query GGUF rows.
