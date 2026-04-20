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
| [Nomic v2 MoE model card](https://huggingface.co/nomic-ai/nomic-embed-text-v2-moe) | Remains blocked until semantic docs have a hard token budget compatible with its context limits. |
| [Qwen3 0.6B model card](https://huggingface.co/Qwen/Qwen3-Embedding-0.6B) | Keep as a quality candidate, but do not assume Matryoshka support without model-card or implementation evidence. |

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
| `vector-quant` | Stored-vector quantization lane. Manifest-only until CodeStory has quantized-vector storage/search support. | `$env:CODESTORY_EMBED_RESEARCH_STAGE='vector-quant'; node scripts/embedding-gpu-fair-benchmark.mjs` |
| `dimension` | Nomic Matryoshka dimensions and BGE-small negative controls. | `$env:CODESTORY_EMBED_RESEARCH_STAGE='dimension'; node scripts/embedding-gpu-fair-benchmark.mjs` |
| `retrieval` | Hybrid weight, semantic scope, and alias-mode sweeps. | `$env:CODESTORY_EMBED_RESEARCH_STAGE='retrieval'; node scripts/embedding-gpu-fair-benchmark.mjs` |
| `bge-small-candidate` | Three-repeat current default versus crossed BGE-small `scope=all`, `no_alias`, b256. | `$env:CODESTORY_EMBED_RESEARCH_STAGE='bge-small-candidate'; node scripts/embedding-gpu-fair-benchmark.mjs` |
| `finalists2` | Three-repeat comparison of selected candidates after earlier stages identify them. | `$env:CODESTORY_EMBED_RESEARCH_STAGE='finalists2'; node scripts/embedding-gpu-fair-benchmark.mjs` |

Use `CODESTORY_EMBED_RESEARCH_LIST=1` to list case IDs before running a stage.
Use `CODESTORY_EMBED_RESEARCH_CASES=<case-id,...>` for bounded reruns.

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
- Do not run Qwen dimension truncation as a Matryoshka claim unless a current
  source proves dimension-flexible behavior.
- Treat vector quantization as blocked until the store/search layer can preserve
  quantized corpus vectors and rescore against original vectors.
- Rerun `controls` after any harness ranking change before comparing new lanes.
