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

## Current Recommendation

Use these defaults until a newer benchmark disproves them:

- Default in-process embedding: `CODESTORY_EMBED_BACKEND=onnx` with
  `CODESTORY_EMBED_PROFILE=bge-small-en-v1.5`.
- Fastest explicit legacy option: `CODESTORY_EMBED_BACKEND=onnx` with
  `CODESTORY_EMBED_PROFILE=minilm`.
- Best portable GPU quality path: `CODESTORY_EMBED_BACKEND=llamacpp` with
  `CODESTORY_EMBED_PROFILE=embeddinggemma-300m`.
- Best portable GPU speed/quality balance: `CODESTORY_EMBED_BACKEND=llamacpp`
  with `CODESTORY_EMBED_PROFILE=nomic-embed-text-v1.5` and
  `CODESTORY_EMBED_LLAMACPP_REQUEST_COUNT=2`.

Before changing a default, rerun both speed and quality on the target machine.
Do not make performance claims from a backend that is not wired into
`crates/codestory-runtime/src/search/engine.rs`.
