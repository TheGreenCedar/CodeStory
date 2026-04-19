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
| Best llama.cpp throughput profile | BGE-base GGUF, `--pooling cls`, CodeStory request count `4`, server `-np 4` |
| Fast lower-quality profile | ONNX BGE-small with `no_alias`, batch `256` |
| Quality experiment, not default | Qwen3 0.6B GGUF with `alias_variant`, context `2048`, `r1/np1` |
| Blocked candidate | `nomic-embed-text-v2-moe` until semantic docs have a hard token budget |

The alias feature stays, but the older full alias text does not become the
default. The default is the compact alias variant: language, terminal name,
owner names, and symbol-role hints are kept; full name-alias and path-alias
lists are excluded unless `CODESTORY_SEMANTIC_DOC_ALIAS_MODE=current_alias` is
set for reproduction.

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
| 1 | BGE-base | ONNX DirectML | `alias_variant` | b128, s2 | 0.5006 | 0.3035 | 0.8214 | 2.2391 | 301.899 | 50.524 | 0.7683 | Default self-contained runtime |
| 2 | Qwen3 0.6B | llama.cpp Vulkan | `alias_variant` | b128, r1/np1, ctx2048, last pool | 0.5060 | 0.3571 | 0.7857 | 2.1364 | 73.214 | 160.122 | 0.7000 | Quality experiment only: slower and lower Hit@10 |
| 3 | BGE-base | llama.cpp Vulkan | `alias_variant` | b128, r4/np4, ctx4096, cls pool | 0.4881 | 0.2857 | 0.8214 | 2.3043 | 388.447 | 44.205 | 0.6674 | Best llama.cpp throughput profile |
| 4 | BGE-base | ONNX DirectML | `alias_variant` | b128, s4 | 0.4881 | 0.2857 | 0.8214 | 2.3043 | 303.456 | 50.978 | 0.6176 | Not enough gain over s2 to become default |
| 5 | BGE-base | llama.cpp Vulkan | `alias_variant` | b128, r1/np1, ctx4096, cls pool | 0.4881 | 0.2857 | 0.8214 | 2.3043 | 210.805 | 63.773 | 0.5634 | Slower; earlier MRR bump did not repeat |
| 6 | BGE-small | ONNX DirectML | `no_alias` | b256, s2 | 0.4483 | 0.2857 | 0.8214 | 2.9565 | 586.109 | 34.017 | 0.3000 | Fast profile with quality tradeoff |

Persistent misses in the finalist rows still include
`trail-neighborhood`, `semantic-sync`, `semantic-doc-text`, and
`resolve-target`. Qwen changes the miss pattern but does not solve enough of it
to justify the throughput cost as a default.

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
The shipped default follows the chosen default model: BGE-base with
`alias_variant`. The harness can still reproduce `no_alias` and `current_alias`
without hand-editing code.

## Tuning Summary

| Area | Finding |
| --- | --- |
| ONNX BGE-base sessions | Stage 2 showed s4 can be faster, but Stage 4 repeats did not preserve a quality or speed advantage over s2. Keep s2/default session cap for now. |
| ONNX BGE-base batch | Batch 128 remains the comparison and default value. Batch 256 did not improve quality and was not clearly faster in the clean tuning run. |
| llama.cpp BGE-base parallelism | `r4/np4` gave the best repeat throughput while preserving Hit@10. The earlier `r1/np1` MRR bump did not repeat. |
| BGE-small | Best treated as a fast no-alias profile. It is not a default because MRR lags BGE-base. |
| Qwen3 0.6B | Strong MRR but lower Hit@10 and about 4x slower than ONNX BGE-base in finalists. Keep as an experiment. |
| Prompt/pooling sweeps | BGE code-query prefix hurt MRR and Hit@10. BGE no-query-prefix hurt ONNX MRR. BGE mean pooling improved Hit@10 but reduced primary MRR. Qwen symbol instruction hurt MRR. |
| Nomic v1.5 prompt sweep | The no-prefix variant failed with a Windows search-index permission error, not a quality result. Keep the existing Nomic prompt/profile until retested. |
| Nomic v2 MoE | Blocked until semantic docs enforce a token budget; prior alias docs exceeded llama.cpp's 512-token cap. |

## Artifact Roots

| Stage | Artifact root | Purpose |
| --- | --- | --- |
| Smoke | `target/embedding-research/smoke-20260419` | One ONNX DirectML row and one llama.cpp Vulkan row; verified no CPU fallback. |
| Stage 1 alias | `target/embedding-research/alias-stage1-20260419` | Alias mode comparison across the main candidate set. |
| Stage 2 tuning | `target/embedding-research/tuning-stage2-clean-20260419` | Clean batch/session/parallelism tuning subset. |
| Stage 3 prompt | `target/embedding-research/prompt-stage3-20260419` | Prompt, pooling, context, and profile-specific toggles. |
| Stage 4 finalists | `target/embedding-research/finalists-stage4-20260419` | Two-repeat finalist run used for the accepted table. |
| Superseded raw run | `target/embedding-research/tuning-stage2-20260419` | Kept as provenance only; it mixed selected IDs across stages before the harness selection bug was fixed. |

Each current run writes `results.csv`, `results.json`, `query-ranks.csv`,
`alias-comparisons.csv`, `cases.json`, `queries.json`, raw per-case logs, and a
Markdown report. New finalist/repeat runs also write `repeat-summary.csv`.

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
- Use the CodeStory runtime indexing path, not a standalone embedding probe.
- Use isolated cache directories per row under `target/embedding-research/<run>/`.
- Keep one comparison table to one semantic doc mode and one batch-size policy,
  or explicitly mark mixed rows as tuning/provenance.
- For ONNX rows, use DirectML or CUDA and reject CPU-provider fallback.
- For llama.cpp rows, require a GPU device log plus full model-layer offload.
- Store raw logs and per-query ranks. Do not collapse the result to only the
  final score.

Useful commands:

```powershell
cargo build --release -p codestory-cli --features codestory-runtime/onnx-directml

$env:CODESTORY_EMBED_RESEARCH_STAGE = 'smoke'
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

## Remaining Research Backlog

This is the planned follow-up work, in priority order:

| Priority | Work | Why |
| ---: | --- | --- |
| 1 | Stabilize finalist repeat noise by checking deterministic ordering, tie breaks, and cache/index rebuild differences. | Stage 4 showed BGE-base MRR can move between 0.4881 and 0.5131 across repeats. |
| 2 | Add a token-aware semantic-doc budget and retest `nomic-embed-text-v2-moe`. | It cannot be fairly judged while alias docs can overflow its context. |
| 3 | Expand the query suite beyond 28 queries and weight persistent-miss buckets explicitly. | Four persistent misses still dominate the failure story. |
| 4 | Sweep hybrid weights and semantic doc scope (`durable` vs `all`) for BGE-base finalists. | Retrieval settings may move quality more than backend settings now. |
| 5 | Fill the remaining llama.cpp equivalents for smaller models under the new alias modes. | Older BGE-small/MiniLM llama rows exist, but not all were rerun under this staged GPU-only harness. |
| 6 | Sweep llama server `--batch-size`, `--ubatch-size`, flash attention, and context for BGE-base and Qwen. | Current llama results mostly vary request/server parallelism and context. |
| 7 | Try additional compact alias variants rather than restoring full aliases. | Full aliases helped some models but hurt others; compact variants are the promising path. |
