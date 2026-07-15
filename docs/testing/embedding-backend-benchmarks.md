# Embedding Engine Benchmarks

CodeStory uses the embedded BGE-base-en-v1.5 Q8 GGUF contract through its linked
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

Quality cannot regress. A repeatable throughput, warm-latency, or memory
regression blocks the cutover. Five percent is the allowance for measurement
noise, not an acceptable sustained loss.

## Historical incumbent

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

These numbers are the incumbent reference, not proof for a new head. Attach the
same-run raw result JSON and machine identity to the PR before accepting the
cutover.

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
