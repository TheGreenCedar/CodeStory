# Embedding Backend Benchmarks

This page records the local backend and model comparison from 2026-04-19. The
numbers are machine-specific and should be rerun before changing defaults, but
they are the current evidence for CodeStory's embedding backend choice.

## What Was Tested

All rows below came from the implemented CodeStory runtime path, not from a
standalone model probe. The benchmark indexed `3,729` semantic docs and then ran
the retrieval quality harness.

Local artifact paths from the run:

- speed CSV: `target/embedding-tuning-benchmarks/real-backend-20260419-084629/speed.csv`
- quality CSV: `target/embedding-tuning-benchmarks/real-backend-20260419-084629/quality/quality.csv`
- llama.cpp logs: `target/embedding-tuning-benchmarks/real-backend-20260419-084629/logs/`

The earlier llama/GGUF timing notes from before the runtime backend existed are
invalid and should not be reused.

## Backend Support

Runtime supports three embedding backends:

- `CODESTORY_EMBED_BACKEND=onnx`: in-process ONNX runtime for local model artifacts.
- `CODESTORY_EMBED_BACKEND=llamacpp`: OpenAI-compatible `llama-server --embedding`
  endpoint for GGUF models.
- `CODESTORY_EMBED_RUNTIME_MODE=hash`: deterministic local-dev semantic smoke mode.

The llama.cpp path keeps CodeStory platform-neutral. CodeStory only talks to the
embedding HTTP endpoint; acceleration is owned by the server. On the benchmark
machine, llama.cpp used Vulkan on an AMD Radeon RX 7900 XT and logged:

- `using device Vulkan0 (AMD Radeon RX 7900 XT)`
- `offloaded 25/25 layers to GPU`
- `Vulkan0 model buffer size = 307.13 MiB`
- `Vulkan0 compute buffer size = 71.03 MiB`

ONNX remains useful for fast in-process embedding, but it is not required for
Qwen, Gemma, or Nomic profiles.

## Speed Results

Sorted by semantic docs per second.

| Config | Backend | Profile | Semantic seconds | Docs/sec | Index seconds |
| --- | --- | --- | ---: | ---: | ---: |
| `onnx-minilm-b256-s2` | onnx | `minilm` | 3.544 | 1052.20 | 11.012 |
| `onnx-bge-small-b256-s2` | onnx | `bge-small-en-v1.5` | 6.691 | 557.32 | 14.921 |
| `llama-nomic-v15-b256-r2-np2` | llama.cpp | `nomic-embed-text-v1.5` | 11.588 | 321.80 | 19.073 |
| `llama-nomic-v15-b128-r2-np2` | llama.cpp | `nomic-embed-text-v1.5` | 11.611 | 321.16 | 18.899 |
| `onnx-bge-base-b64-s1` | onnx | `bge-base-en-v1.5` | 14.788 | 252.16 | 23.709 |
| `llama-nomic-v15-b64-r1-np1` | llama.cpp | `nomic-embed-text-v1.5` | 15.920 | 234.23 | 23.953 |
| `llama-nomic-v15-b128-r1-np1` | llama.cpp | `nomic-embed-text-v1.5` | 15.943 | 233.90 | 24.776 |
| `llama-nomic-v2-b128-r2-np2` | llama.cpp | `nomic-embed-text-v2-moe` | 21.010 | 177.49 | 28.483 |
| `llama-gemma-b128-r2-np2` | llama.cpp | `embeddinggemma-300m` | 21.804 | 171.02 | 29.215 |
| `llama-nomic-v2-b128-r1-np1` | llama.cpp | `nomic-embed-text-v2-moe` | 23.299 | 160.05 | 31.052 |
| `llama-gemma-b128-r1-np1` | llama.cpp | `embeddinggemma-300m` | 27.741 | 134.42 | 35.188 |
| `llama-qwen-ctx8192-b128` | llama.cpp | `qwen3-embedding-0.6b` | 47.292 | 78.85 | 54.037 |
| `llama-qwen-ctx1024-b128` | llama.cpp | `qwen3-embedding-0.6b` | 47.356 | 78.74 | 54.951 |
| `llama-qwen-ctx2048-b128` | llama.cpp | `qwen3-embedding-0.6b` | 47.393 | 78.68 | 54.139 |
| `llama-qwen-ctx512-b128` | llama.cpp | `qwen3-embedding-0.6b` | 48.199 | 77.37 | 56.817 |

Key speed findings:

- MiniLM ONNX is still the fastest local option.
- BGE-small ONNX is the best fast in-process quality candidate.
- Nomic v1.5 is the fastest llama.cpp/GGUF profile.
- `CODESTORY_EMBED_LLAMACPP_REQUEST_COUNT=2` with a matching llama.cpp `-np 2`
  server setting improved Nomic v1.5 from about `234` docs/sec to about `322`
  docs/sec without changing model quality.
- Qwen3 works through llama.cpp, but this local run did not justify its speed
  cost for default indexing.

## Quality Results

Quality used an 18-query retrieval harness. Higher `Hit@k` and `MRR@10` are
better. Lower mean rank is better.

| Config | Backend | Profile | Hit@1 | Hit@10 | MRR@10 | Mean rank when found |
| --- | --- | --- | ---: | ---: | ---: | ---: |
| `llama-gemma-b128-r2-np2` | llama.cpp | `embeddinggemma-300m` | 0.3333 | 0.6111 | 0.4444 | 1.73 |
| `llama-nomic-v2-b128-r2-np2` | llama.cpp | `nomic-embed-text-v2-moe` | 0.2778 | 0.6111 | 0.4046 | 2.09 |
| `llama-nomic-v15-b256-r2-np2` | llama.cpp | `nomic-embed-text-v1.5` | 0.2222 | 0.6111 | 0.3630 | 2.36 |
| `onnx-bge-small-b256-s2` | onnx | `bge-small-en-v1.5` | 0.2222 | 0.5556 | 0.3537 | 2.10 |
| `llama-qwen-ctx8192-b128` | llama.cpp | `qwen3-embedding-0.6b` | 0.2222 | 0.5556 | 0.3519 | 2.00 |
| `onnx-minilm-b256-s2` | onnx | `minilm` | 0.1667 | 0.5556 | 0.2923 | 3.30 |
| `onnx-bge-base-b64-s1` | onnx | `bge-base-en-v1.5` | 0.1667 | 0.5000 | 0.2824 | 2.56 |

Key quality findings:

- EmbeddingGemma via llama.cpp had the best local retrieval quality.
- Nomic v2 quality beat Nomic v1.5, but it was slower.
- Nomic v1.5 was the best speed/quality balance among the non-ONNX options.
- BGE-small was the best ONNX quality/speed compromise in this run.
- Qwen3 was real and working, but its measured quality did not offset its speed
  cost on this harness.

## Semantic Doc Alias Follow-Up

On 2026-04-19, a follow-up compared the pre-alias semantic document text against
the alias-enriched semantic document text. This was a regression check for the
generated doc text, not a replacement for the backend speed table above.

Method:

- Before binary: commit `6930933`.
- After binary: current alias-enriched semantic doc text implementation.
- Query suite: same 18 natural-language symbol retrieval intents.
- Corpus: current CodeStory checkout, `10,662` semantic docs for the accepted
  BGE-base DirectML run.
- Hardware evidence: llama.cpp profiles whose logs showed Vulkan on AMD Radeon
  RX 7900 XT, completed ONNX CPU profiles, plus a BGE-base ONNX rerun built with
  `codestory-runtime/onnx-directml` and run with
  `CODESTORY_EMBED_EXECUTION_PROVIDER=directml`.
- Artifact roots:
  - `target/model-quality-comparison/alias-doc-text`
  - `target/model-quality-comparison/alias-doc-text-directml`

Historical CPU rows are retained below only to preserve what was run. They are
not accepted for future benchmark decisions. New benchmark rows used for
defaults must be GPU-only: ONNX rows should use DirectML or CUDA, and llama.cpp
rows should show a GPU device with all model layers offloaded.

Alias follow-up results:

| Config | Backend | Hardware path | Hit@1 before | Hit@1 after | Hit@10 before | Hit@10 after | MRR@10 before | MRR@10 after | Mean rank before | Mean rank after | Result |
| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| `onnx-minilm-b256-s2` | ONNX | CPU | 0.1111 | 0.1667 | 0.6667 | 0.6111 | 0.2603 | 0.2853 | 4.00 | 3.73 | mixed; ranking improved, recall dipped |
| `onnx-bge-small-b256-s2` | ONNX | CPU | 0.3889 | 0.2778 | 0.8333 | 0.7778 | 0.5283 | 0.4085 | 2.60 | 3.36 | regressed |
| `onnx-bge-base-b64-s1` | ONNX | CPU | invalid | invalid | invalid | invalid | invalid | invalid | invalid | invalid | incomplete CPU run; ignore |
| `llama-nomic-v15-b256-r2-np2` | llama.cpp | Vulkan | 0.2222 | 0.2778 | 0.7222 | 0.7222 | 0.4111 | 0.4602 | 2.46 | 2.08 | ranking improved |
| `llama-nomic-v2-b128-r2-np2` | llama.cpp | Vulkan | 0.2778 | failed | 0.5556 | failed | 0.3750 | failed | 2.00 | failed | alias docs exceeded context |
| `llama-gemma-b128-r2-np2` | llama.cpp | Vulkan | 0.3333 | 0.2778 | 0.6667 | 0.6111 | 0.4468 | 0.3815 | 2.33 | 2.36 | regressed |
| `llama-qwen-ctx8192-b128` | llama.cpp | Vulkan | 0.2778 | 0.3333 | 0.7778 | 0.7778 | 0.4370 | 0.4694 | 2.57 | 2.43 | ranking improved |
| `onnx-bge-base-b64-s1` | ONNX | DirectML | 0.3333 | 0.3333 | 0.7778 | 0.7778 | 0.4681 | 0.4926 | 2.71 | 2.21 | ranking improved |

## Fair GPU-Only Batch-128 Follow-Up

The mixed-batch ranking above is superseded for default decisions. On
2026-04-19, the comparison was rerun with the implemented runtime path under a
single batch size and GPU-only rule.

Method:

- Harness: `scripts/embedding-gpu-fair-benchmark.mjs`.
- Current semantic document text: alias-enriched implementation.
- Corpus: current CodeStory checkout, about `10,682` semantic docs.
- Query suite: same 18 natural-language symbol retrieval intents.
- Shared client batch: `CODESTORY_LLM_DOC_EMBED_BATCH_SIZE=128`.
- ONNX GPU path: release CLI built with `codestory-runtime/onnx-directml`,
  `CODESTORY_EMBED_EXECUTION_PROVIDER=directml`, and two ONNX sessions.
- llama.cpp GPU path: `llama-server --embedding --device Vulkan0 -ngl 999`.
  Completed rows logged `using device Vulkan0 (AMD Radeon RX 7900 XT)` and all
  layers offloaded.
- Artifact roots:
  - `target/embedding-gpu-fair-benchmark/full-20260419-b128-gpu`
  - `target/embedding-gpu-fair-benchmark/qwen-b128-r1-np1-gpu`

Combined score:

`combined_score = 0.70 * normalized(MRR@10) + 0.30 * normalized(docs/sec)`

Higher is better. MRR@10 is weighted more heavily than throughput because a fast
retriever that returns worse symbols is not the best default.

| Rank | Config | Backend | Hardware | Batch | MRR@10 | Hit@10 | Docs/sec | Combined score | Read |
| ---: | --- | --- | --- | ---: | ---: | ---: | ---: | ---: | --- |
| 1 | `llama-bge-base-b128-r2-np2-vulkan` | llama.cpp | Vulkan | 128 | 0.4926 | 0.7778 | 290.47 | 0.7948 | best overall when a llama.cpp server is available |
| 2 | `onnx-bge-base-b128-s2-directml` | ONNX | DirectML | 128 | 0.4926 | 0.7778 | 245.53 | 0.7758 | best self-contained runtime default |
| 3 | `llama-nomic-v15-b128-r2-np2-vulkan` | llama.cpp | Vulkan | 128 | 0.4602 | 0.7222 | 302.32 | 0.7111 | best non-BGE llama.cpp balance |
| 4 | `llama-bge-small-b128-r2-np2-vulkan` | llama.cpp | Vulkan | 128 | 0.4095 | 0.7778 | 428.81 | 0.6259 | nearly tied with ONNX BGE-small |
| 5 | `onnx-bge-small-b128-s2-directml` | ONNX | DirectML | 128 | 0.4085 | 0.7778 | 433.61 | 0.6252 | faster than BGE-base, weaker ranking |
| 6 | `llama-qwen-b128-r1-np1-vulkan` | llama.cpp | Vulkan | 128 | 0.4648 | 0.7222 | 67.20 | 0.6238 | strong quality, too slow for default |
| 7 | `llama-gemma-b128-r2-np2-vulkan` | llama.cpp | Vulkan | 128 | 0.3815 | 0.6111 | 186.99 | 0.4464 | not competitive after alias docs |
| 8 | `onnx-minilm-b128-s2-directml` | ONNX | DirectML | 128 | 0.2668 | 0.5556 | 773.39 | 0.3811 | fastest accepted row, weak quality |
| 9 | `llama-minilm-b128-r2-np2-vulkan` | llama.cpp | Vulkan | 128 | 0.2372 | 0.6111 | 610.37 | 0.2307 | slower and lower quality than ONNX MiniLM |

Failed GPU-only rows:

- `llama-nomic-v2-b128-r2-np2-vulkan` still failed because the model context was
  capped to `512` tokens and one alias-enriched document reached `519` tokens.
- `llama-qwen-b128-r2-np2-vulkan` failed with the connection closed by the
  server. The accepted Qwen row uses the same batch size with `r1/np1`, which is
  the viable GPU server shape on this machine.

Alias follow-up findings:

- Alias-enriched docs are not a universal quality win. They improved ranking for
  MiniLM, Nomic v1.5, Qwen3, and BGE-base DirectML, but regressed BGE-small and
  EmbeddingGemma on this query suite.
- Nomic v2 failed after the alias change because llama.cpp rejected one document:
  `input (519 tokens) is larger than the max context size (512 tokens)`.
- Completed CPU ONNX rows are historical only. Do not use them for future
  ranking or default decisions.
- The accepted BGE-base DirectML run held Hit@10 steady and improved MRR@10 from
  `0.4681` to `0.4926`; this is a ranking improvement, not a recall improvement.
- The persistent misses across the accepted BGE-base DirectML run were
  `trail-neighborhood`, `semantic-sync`, `semantic-doc-text`, and
  `resolve-target`.
- Before adding `nomic-embed-text-v2-moe` back to the recommendation set, add or
  verify a hard semantic-doc token budget; the model context is capped at `512`
  tokens in llama.cpp even when a larger server context is requested.

## Current Recommendation

Use these defaults until a newer fair GPU-only benchmark disproves them:

- Default self-contained runtime: `CODESTORY_EMBED_BACKEND=onnx` with
  `CODESTORY_EMBED_PROFILE=bge-base-en-v1.5`.
- Default semantic doc embedding batch size: `CODESTORY_LLM_DOC_EMBED_BATCH_SIZE=128`.
- Best overall GPU profile when a llama.cpp server is available:
  `CODESTORY_EMBED_BACKEND=llamacpp` with
  `CODESTORY_EMBED_PROFILE=bge-base-en-v1.5`,
  `CODESTORY_EMBED_LLAMACPP_REQUEST_COUNT=2`, and a Vulkan/CUDA server using the
  BGE-base Q8 GGUF with `--pooling cls`.
- Best non-BGE llama.cpp fallback: `CODESTORY_EMBED_BACKEND=llamacpp` with
  `CODESTORY_EMBED_PROFILE=nomic-embed-text-v1.5` and
  `CODESTORY_EMBED_LLAMACPP_REQUEST_COUNT=2`.
- Fastest explicit lower-quality option: `CODESTORY_EMBED_BACKEND=onnx` with
  `CODESTORY_EMBED_PROFILE=minilm`.
- Do not use `nomic-embed-text-v2-moe` with the alias-enriched docs until the
  semantic doc text has a hard token budget; llama.cpp capped this model to a
  `512`-token context and the alias follow-up overflowed it.

Do not treat EmbeddingGemma as the recommended GPU quality path after the alias
follow-up. Its older baseline looked strongest, but the alias-enriched run
regressed and the fair GPU-only combined score is below BGE-base, Qwen3, Nomic
v1.5, and BGE-small. BGE-small is now an explicit faster/lower-quality alternate,
not the default.

Before changing a default, rerun both speed and quality on the target machine
with a GPU-only, same-batch matrix.
Do not make performance claims from a backend that is not wired into
`crates/codestory-runtime/src/search/engine.rs`.

For semantic document text changes, rerun quality on the target model profile.
Do not run CPU benchmark rows for default decisions.
