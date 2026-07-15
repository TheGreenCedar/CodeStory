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
| Hosted engine smoke | `.github/workflows/retrieval-engine-smoke.yml` with explicit CPU policy | Source and protocol behavior only; no Metal or Vulkan claim |
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

CodeRankEmbed Q8 is the current product model. It was selected over BGE by the
#1164 same-machine Metal study because the frozen dense-retrieval slice improved
MRR@10 by 36% and Hit@1 by 55%. For a future model change, compare the current
path and candidate in the same release build and machine. Treat quality as the
primary gate; report throughput, warm latency, RSS, and GPU memory separately
so an accepted tradeoff remains explicit. Do not merge a temporary A/B selector
or displaced implementation.

Run repo-scale stats once on the final merge-ready head when the testing matrix
requires it. The stats log is telemetry, not release authorization.
