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
| Best prior quality candidate | BGE-base ONNX DirectML, `alias_variant`, batch `128`, sessions `2` |
| Best llama.cpp throughput profile | BGE-base GGUF, `--pooling cls`, CodeStory request count `4`, server `-np 4` |
| Fast prior finalist profile | ONNX BGE-small with `no_alias`, batch `256` |
| Best Run 2 controls lead | ONNX BGE-small with `scope=all`, `no_alias`, batch `256`; needs retrieval sweep before promotion |
| Best Run 2 retrieval signal | `scope=all` improved BGE-small MRR most, while `no_alias` was the best durable-scope row; original retrieval speed rows are invalid because the GPU was locked |
| Quality experiment, not default | Qwen3 0.6B GGUF with `alias_variant`, context `2048`, `r1/np1` |
| Blocked candidate | `nomic-embed-text-v2-moe` until semantic docs have a hard token budget |

The shipped runtime default remains BGE-small because it is the current
self-contained baseline in code and docs. The Stage 4 table below is retained as
prior benchmark evidence, not as authority to silently flip the default without
the broader Run 2 research pass.

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
| `bge-small-en-v1.5` | ONNX default, GGUF Q8, planned GGUF Q6/Q5/Q4, planned ONNX int8/int4 | Shipped default profile. Stage 1 showed `no_alias` beat aliases for BGE-small. Stage 4 no-alias finalist was fast but lower quality. Run 2 controls made `scope=all`, `no_alias`, b256 the best composite controls row. Run 2 retrieval isolated knobs: `scope=all` had best MRR, and `no_alias` was the best durable-scope row. | Keep default unchanged. Run a crossed, repeatable row for `scope=all` + `no_alias` + b256 with fail-hard DirectML provider selection. |
| `bge-base-en-v1.5` | ONNX, GGUF Q8, planned GGUF Q6/Q5/Q4, planned ONNX int8/int4 | Best prior quality family. Stage 4 ONNX s2 had MRR@10 0.5006. Run 2 controls improved all-scope MRR@10 to 0.6379 for ONNX and 0.6362 for llama.cpp, both Hit@10 0.9143. | Best quality reference and best llama.cpp throughput reference, but model/vector footprint is larger than BGE-small. Use as quality bar, not automatic default. |
| `nomic-embed-text-v1.5` | GGUF Q8, planned GGUF Q6/Q5/Q4 | Legacy full GPU run: MRR@10 0.4602, Hit@10 0.7222, 302 docs/sec. Stage 1 found `current_alias` was its best alias mode at MRR@10 0.4739. Prompt no-prefix row failed with a Windows search-index permission error, not a quality result. | Still interesting because it documents Matryoshka dimensions. Run `dimension` before making any speed/footprint claim. |
| `nomic-embed-text-v2-moe` | GGUF Q8 | Legacy full run failed; prior alias docs exceeded the model context cap. Run 2 records it as blocked until semantic docs have a token budget. | Blocked, not rejected. Needs token-aware semantic-doc budgeting before quality comparison. |
| `embeddinggemma-300m` | GGUF Q8 | Smaller early quality run looked promising, but legacy full GPU run was weaker at MRR@10 0.3815 and Hit@10 0.6111. Stage 1 alias test favored `no_alias`; `current_alias` regressed by about 0.0504 MRR. | Keep as historical candidate; not a default or finalist until a new source or run changes the case. |
| `qwen3-embedding-0.6b` | GGUF Q8, planned GGUF Q6/Q5/Q4 | Initial full b128 row failed, then bounded runs worked. Stage 1 alias variant reached MRR@10 0.5208. Stage 4 finalist averaged MRR@10 0.5060 but Hit@10 was 0.7857 and throughput was about 73 docs/sec. | Quality experiment only. Too slow and lower Hit@10 than BGE candidates for default use. Do not run dimension truncation unless a source proves Matryoshka support. |

Quantized model rows are part of the plan, but most quantized artifacts are not
present yet. Missing quantized GGUF/ONNX artifacts should produce skipped rows
in `weight-quant`; they are not evidence that quantization hurts quality.
Stored-vector quantization is a separate storage/search implementation lane and
is intentionally manifest-only until CodeStory can index quantized corpus
vectors and rescore safely.

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
`scope=all` + `no_alias` + b256. That crossed row is the next decision-grade
default candidate to run with fail-hard DirectML provider selection.

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

That is not enough to flip defaults because the exact best controls combination
has not yet been repeated as a crossed retrieval/finalist row under verified GPU
conditions. It is enough to make `scope=all` + `no_alias` + b256 the next
BGE-small default-candidate row.

### BGE-Base

BGE-base is still the quality reference. It is the best prior ONNX quality
candidate and the best llama.cpp throughput candidate. Run 2 controls showed
both ONNX and llama.cpp BGE-base passing the quality gate with essentially the
same Hit@10 and MRR, but llama.cpp had a much smaller model artifact and better
docs/sec in that controls run. The tradeoff is still footprint: the vector bytes
per doc are double BGE-small because the embedding dimension is 768 instead of
384.

### Nomic V1.5

Nomic v1.5 should not be treated as "done" just because it is missing from the
finalist table. Its main reason to stay in the plan is Matryoshka dimensionality
support. The correct next evidence is the `dimension` stage, comparing 768, 512,
256, 128, and 64 dimensions against BGE-small negative controls. Existing prompt
and alias results are not sufficient to judge that lane.

### Nomic V2 MoE

Nomic v2 MoE is blocked by input-budget correctness, not rejected for model
quality. Do not rerun it by shrinking random parameters around the failure.
First add a hard semantic-doc token budget, then rerun it as a normal candidate.

### EmbeddingGemma

EmbeddingGemma looked better in early smaller runtime-backed runs than in the
fairer full GPU run. The alias stage also showed aliases hurt it. Keep the model
documented because it did show up in prior research, but it currently has no
promotion path without a new hypothesis.

### Qwen3 0.6B

Qwen is a real quality experiment, not a default candidate. Its MRR was strong
in the alias and finalist stages, but Hit@10 and throughput make it hard to
justify as a default. Keep it in quality experiments and quantized-weight
research, but do not claim dimension-shortening support without source evidence.

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
| llama.cpp BGE-base parallelism | `r4/np4` gave the best repeat throughput while preserving Hit@10. The earlier `r1/np1` MRR bump did not repeat. |
| BGE-small | Shipped default profile remains BGE-small with compact aliases. The faster no-alias BGE-small row stays a speed tradeoff, not the default. |
| Run 2 retrieval | `scope=all` was the largest BGE-small quality lever; durable `no_alias` beat durable aliases; hybrid-weight tweaks did not improve quality. Original speed rows are invalid because the GPU was locked; fail-hard DirectML speedchecks restored the default row to about 17-21s index time. |
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
| Run 2 controls | `target/embedding-research/controls-run2-20260419` | Expanded 70-query controls for default, prior BGE-base candidates, and fast BGE-small. |
| Run 2 retrieval | `target/embedding-research/retrieval-run2-20260419` | Hybrid-weight, semantic-scope, and alias-mode isolation for BGE-small; quality usable, speed invalid because GPU was locked. |
| Run 2 retrieval speedchecks | `target/embedding-research/retrieval-run2-speedcheck-normal-20260419`, `target/embedding-research/retrieval-run2-speedcheck-failhard-20260419`, `target/embedding-research/retrieval-run2-speedcheck-confirmed-gpu-20260419` | One-row default-shape checks showing slow invalid rows, then restored GPU-fast DirectML rows after provider fail-hard and GPU availability. |
| Superseded raw run | `target/embedding-research/tuning-stage2-20260419` | Kept as provenance only; it mixed selected IDs across stages before the harness selection bug was fixed. |

Each current run writes `results.csv`, `results.json`, `query-ranks.csv`,
`alias-comparisons.csv`, `cases.json`, `queries.json`, raw per-case logs, and a
Markdown report. New finalist/repeat runs also write `repeat-summary.csv`.
Run 2 additionally writes `manifest.json` and `sources.md`; see
[embedding-research-run-2.md](embedding-research-run-2.md).

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
- Store raw logs and per-query ranks. Do not collapse the result to only the
  final score.
- Record obvious host-load anomalies, GPU availability problems, and timing
  drifts. When a same-shape row diverges materially from a prior run, rerun a
  one-row speedcheck and compare phase timings before making speed or cost
  claims.
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
| 1 | Run a verified-GPU crossed BGE-small retrieval/finalist row for `scope=all`, `no_alias`, b256. | Controls made this exact combination the best lead, but retrieval isolated scope and alias mode separately. |
| 2 | Rerun any retrieval row whose speed/cost matters under fail-hard DirectML provider selection. | The original retrieval run has usable quality deltas but invalid speed data because the GPU was locked. |
| 3 | Add a token-aware semantic-doc budget and retest `nomic-embed-text-v2-moe`. | It cannot be fairly judged while alias docs can overflow its context; Run 2 records it as skipped until then. |
| 4 | Implement quantized-vector storage/search with full-precision rescoring, then run `vector-quant`. | Source scan says vector quantization is separate from model quantization, but CodeStory cannot benchmark it honestly until the store/search path exists. |
| 5 | Generate missing GGUF/ONNX quantized artifacts and run `weight-quant`. | The harness now records missing artifacts as skipped rows instead of false failures. |
| 6 | Run `dimension` for Nomic v1.5 and compare against BGE-small negative controls. | Nomic documents Matryoshka dimensions; BGE truncation should stay a negative control. |
| 7 | Promote only repeat-stable rows into `finalists2`. | Defaults should not change from a single mixed tuning pass. |
