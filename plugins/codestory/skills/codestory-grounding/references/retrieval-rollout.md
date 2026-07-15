# Retrieval Rollout Verification

Use this when a change touches retrieval, embedding execution, packet/search,
benchmarks, packaging, or accelerator claims. Match the proof to the claim.

| Rollout layer | Trustworthy proof | Claim boundary |
| --- | --- | --- |
| Indexer coverage | `cargo test --locked -p codestory-indexer --test fidelity_regression`; `cargo test --locked -p codestory-indexer --test tictactoe_language_coverage` | Parser-backed graph and document coverage only |
| Retrieval engine | `cargo test --locked -p codestory-retrieval`; a live `retrieval index`; status with `retrieval_mode="full"`, exact model/build identity, backend, adapter, policy, and timed smoke | In-process engine and manifest coherence only |
| Runtime | Runtime library, generalization, and retrieval-eval lanes | Packet/search admission and result behavior |
| CLI and plugin | Focused CLI protocol tests plus plugin static tests | Transport and user-facing capability state |
| Performance | Same-build incumbent/candidate rows for cold initialization, warm search, bulk indexing, RSS, GPU memory, vector parity, quality, and multi-repository reuse | Promotion only when no repeatable regression exceeds the 5% noise allowance |
| Hosted engine smoke | `.github/workflows/retrieval-engine-smoke.yml` | Retrieval, runtime, stdio, indexing, engine identity, docs, scripts, or workflow changes | Explicit CPU policy only; no Metal or Vulkan claim |
| Packaged hardware | Protected Metal or Vulkan workflow using the packaged executable offline | Only the exact backend and adapter exercised by that artifact |

Normal plugin calls prepare retrieval automatically. They expose `ready`,
`preparing`, or `unavailable`; users are not asked to approve an internal
lifecycle. Maintainer JSON owns backend details.

## Full retrieval proof

1. Build or unpack one release executable.
2. Disable network access and use an empty isolated cache.
3. Run graph indexing and `retrieval index --refresh full`.
4. Require `retrieval_mode="full"` and validate the engine identity fields.
5. Run search and packet through the plugin launcher.
6. Open a second repository in the same stdio process and require one engine
   instance and one model load.
7. Restart the process and require content-addressed model reuse without a
   rewrite.
8. Reject any helper executable, download, endpoint, port, PID, repair worker,
   or interactive setup state.

Hosted CI sets `CODESTORY_EMBED_ALLOW_CPU=1` and must report `cpu_explicit`.
Protected Apple Silicon proof must report Metal and verified accelerator
execution. Windows hardware proof must report Vulkan. Linux makes no GPU claim
without real Vulkan hardware evidence.

## Quality and performance gate

The accepted historical BGE-base Q8 baseline is about 368-372 embedded
documents/sec, 84.7 ms cross-repository search p95, MRR@10 0.9824, Hit@10 1.0,
Hit@1 0.973, and 829-1,020 MB peak working set. Compare old and new paths in the
same release build on the same machine. A result outside the 5% measurement
noise allowance blocks deletion or promotion unless the difference is proved
non-repeatable. Do not merge an A/B switch or the legacy implementation.

Run repo-scale stats once on the final merge-ready head when the testing matrix
requires it. The stats log is telemetry, not release authorization.
