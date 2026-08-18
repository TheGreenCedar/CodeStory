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

## Vector index backend bake-off

The dense *index* is a separate question from the embedding *model*. W6.8
(#1664, prior art #1196/#1202) compares four candidates — the shipped exact
SQLite row scan, an exact resident `f32` matrix, sqlite-vec, and USearch — over
the 1,000/10,000/25,000/75,000 workload ladder from #1340.

```
cargo run -p codestory-bench --example codestory_vector_backend_bakeoff -- \
  --out benchmarks/release-evidence/vector-backend-bakeoff/<host>.json \
  --build-invocation "<the command that produced this binary>"
```

The runner is a Cargo example rather than a `src/bin` target because
`codestory-bench`'s `[dependencies]` feed the packaged qualification binary;
the `benchmark-support` feature it needs must stay in dev-dependencies.

The runner publishes each rung through the product's attested publication path
and measures the incumbent column with `codestory-retrieval`'s own dense scan
via that crate's `benchmark-support` feature, so the incumbent is never a
convenient reimplementation. Candidates are counterbalanced within each block
and scored against an exhaustive exact ranking computed outside every timed
region.

Gates, ladder, platform set, and the fail-closed adopt-or-retain rule live in
`codestory_bench::vector_backend_bakeoff`. Agreement is symmetric
(`|served ∩ exact| / |served ∪ exact|`) and its floor is 1.0: the lane abstains
relative to its own best hit, so a backend that loses — or invents — one
neighbour moves the abstention floor for the whole window.

Adoption requires a complete pass over every gate, at every rung, on every
shipped platform, over a corpus of real embeddings, with the incumbent measured
in the same run. Anything less records `retain_incumbent` with a per-candidate
reason. **This is a softened gate: a non-qualifying bake-off never blocks a
release and never changes a backend or quality claim.** Adopting a backend is an
implementation change that lands with the backend, not with a measurement.

Recorded runs live under `benchmarks/release-evidence/vector-backend-bakeoff/`
and are held to their own contract by
`crates/codestory-bench/tests/vector_backend_bakeoff_evidence.rs`, which
recomputes the recorded verdict from the recorded measurements rather than
trusting it.

### 0.17 outcome: no backend selected

`macos-arm64.json` is the only recorded run. It measured the incumbent and the
resident matrix over 600 counterbalanced samples per rung at the shipped
768-dimension width. **Neither qualifies, and no backend is adopted.**

sqlite-vec and USearch produced no numbers at all: neither is a workspace
dependency nor vendored, and the offline build contract forbids fetching a
native dependency during a proof run. They are recorded as `not_measured` with
that reason rather than estimated.

Two things in that record are worth reading, and neither is a product claim:

- Both measured backends agreed exactly with an exhaustive ranking at every
  rung, so nothing in the run suggests a quality difference between them.
- On this host the *incumbent* exceeded the 250 ms semantic stage budget on
  3.3% of queries at the 75,000-vector target, against the bake-off's 1%
  ceiling. The resident matrix stayed at 0.5% and held 232 MB resident, inside
  the 384 MiB cap.

The second point is the reason the run is worth keeping. It is not a latency
claim: the vectors are synthetic, the build carried debug assertions because
the shipping profile needs an embedded model this host lacks, and one developer
workstation is not a proof host. It is a signal that the incumbent's cost at
the target corpus size is close enough to its own budget to deserve a real
measurement.

Turning that signal into a decision needs what this run could not produce: a
representative corpus of shipped-model embeddings, and cross-platform
immutable-generation evidence. Until then the incumbent stands, and the
reconsideration trigger is the field rate of the semantic stage degradation
counters, not a new microbenchmark.

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

CPU embeddings are unsupported. Apple Silicon evidence must use the packaged
Metal executable. Windows and Linux hardware evidence must use their packaged
Vulkan executables. The Linux claim requires
`.github/workflows/linux-vulkan-proof.yml`; source-only CPU rejection tests are
not runtime evidence.

The packaged proof also requires offline clean-cache execution, the exact
embedded-model digest, ggml build identity, physical adapter identity, timed
smoke, full layer offload under accelerated policy, multi-repository reuse, and
restart reuse. See [retrieval-architecture.md](retrieval-architecture.md).

Historical retired-backend, hash-projection, external-endpoint, and
helper-process rows remain useful only in archived evidence. They are not
supported product paths.
