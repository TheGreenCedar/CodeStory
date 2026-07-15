# Embedding Engine Benchmarks

CodeStory uses the embedded CodeRankEmbed Q8 GGUF contract through its linked
llama.cpp/ggml engine. The model, tokenizer, prefixes, CLS pooling,
normalization, batching, dimensions, and stored-vector format are fixed product
inputs rather than runtime-selectable backends.

## Cutover gate

Before deleting an incumbent implementation, compare incumbent and candidate
inside the same release build on the same machine. The private selector exists
only for the measurement run and must not merge.

| Dimension | Required comparison |
| --- | --- |
| Cold | One-shot CLI process initialization and first embedding |
| Warm | Repeated search latency, separated from initialization |
| Bulk | Documents/sec and batch distribution on the same corpus |
| Memory | Peak process RSS and backend GPU memory |
| Parity | Vector dimensions, norm, and numerical similarity on pinned inputs |
| Quality | MRR@10, Hit@10, Hit@1, exact-symbol and adversarial cases |
| Reuse | Two repositories in one process, one engine instance/model load |
| Restart | Content-addressed model reuse without rewriting materialization |

Quality is the primary product gate. Throughput, warm latency, process memory,
and GPU memory are separate decision inputs: a small quality move does not
justify a large operational cost, but a material, repeatable retrieval gain may
justify an explicit performance tradeoff. Five percent is the threshold below
which a measured difference is treated as noise.

## 0.16 model decision

Issue #1164 compared the incumbent BGE model with CodeRankEmbed and GTE
ModernBERT using one release-mode executable, 988 CodeStory symbol documents,
32 frozen pre-existing queries, three bulk passes, Apple M5 Metal, and a second
repository to prove one shared model load.

| Model | MRR@10 | Hit@10 | Hit@1 | Docs/s | Warm p95 | Peak RSS | Metal memory |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| BGE-base-en-v1.5 Q8 | 0.4597 | 0.7188 | 0.3438 | 256.6 | 5.74 ms | 280.9 MB | 140.3 MB |
| CodeRankEmbed Q8 | 0.6253 | 0.8125 | 0.5313 | 204.7-221.8 | 5.96-6.80 ms | 302.6-314.7 MB | 166.7-191.7 MB |
| GTE ModernBERT Q8 | 0.4711 | 0.8125 | 0.3438 | 172.8 | 6.12 ms | 381.4 MB | 186.7 MB |

CodeRankEmbed won the product decision: MRR@10 improved 36% and Hit@1 improved
55% on the same dense-only slice. The accepted tradeoff is lower bulk
throughput and higher process/GPU memory. GTE did not offer a comparable quality
gain. Upstream-to-GGUF vector parity passed at cosine 0.9986 or better, and all
models fully offloaded to Metal.

These are relative dense-model measurements, not the historical full-product
hybrid score. The raw results and exact artifact digests remain attached to
[issue #1164](https://github.com/TheGreenCedar/CodeStory/issues/1164).

## Historical full-product reference

The closest accepted BGE-base Q8 row used batch 512, request count 6, server
batch/microbatch 1024, stored int8 vectors, and full-text enabled.

| Metric | Historical result |
| --- | ---: |
| Embedded documents/sec | 368.01 baseline; 371.89 repeat |
| Cross-repository search p95 | 84.7 ms |
| MRR@10 | 0.982432 |
| Hit@10 | 1.0 |
| Hit@1 | 0.972973 |
| Peak working set | 828.73 MB baseline; 1,019.79 MB repeat |

These numbers describe the former full-product BGE path. They are context, not
proof for CodeRank or a new head. Attach same-run raw result JSON and machine
identity to the PR before accepting a future cutover.

## Product proof

Hosted jobs may set `CODESTORY_EMBED_ALLOW_CPU=1` and must label the policy
`cpu_explicit`. Apple Silicon evidence must use the packaged Metal executable.
Windows hardware evidence must use the packaged Vulkan executable. Linux makes
no GPU claim without protected Vulkan hardware.

The packaged proof also requires offline clean-cache execution, the exact
embedded-model digest, ggml build identity, physical adapter identity, timed
smoke, full layer offload under accelerated policy, multi-repository reuse, and
restart reuse. See [retrieval-architecture.md](retrieval-architecture.md).

Historical ONNX, hash projection, external endpoint, and helper-process rows
remain useful only in archived evidence. They are not supported product paths.
