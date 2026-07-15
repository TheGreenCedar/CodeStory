# Retrieval Verification Architecture

This page defines evidence tiers and acceptance assertions. The system design
lives in [retrieval design](../architecture/retrieval-design.md); current
measurements live in
[embedding backend benchmarks](embedding-backend-benchmarks.md).

## Evidence tiers

| Tier | Required evidence | Supported claim |
| --- | --- | --- |
| Source | locked checks and focused crate tests | source compiles and contracts hold |
| Hosted package | packaged executable, offline isolated cache, explicit CPU policy | package is self-contained; no acceleration claim |
| Protected hardware | same package, CPU disallowed, physical backend/adapter, timed smoke and full offload | Metal or Vulkan works on that machine |
| Product runtime | plugin launcher, full retrieval, packet/search, two projects sharing one engine | installed agent path is coherent |
| Restart | new process reuses verified materialized model content | content-addressed cache reuse works |
| Performance/quality | same-run measurements and holdout gates | an engine change is promotion-eligible |

A lower tier cannot support a higher-tier claim.

## Required engine assertions

After an engine-initializing operation, packaged proof reads
`codestory://diagnostics/retrieval-engine` and verifies:

- exact model digest and ggml build identity;
- backend, physical adapter, and `accelerated` or `cpu_explicit` policy;
- engine instance and model-load count;
- initialization and live-smoke timing;
- materialized path, digest, and reuse state;
- model/offloaded layer counts and live accelerator execution.

Accelerated proof rejects software adapters and requires the expected model
offload. Hosted proof requires explicit CPU permission; absent GPU hardware
does not imply permission.

## Packaged product assertions

`.github/scripts/check-packaged-agent-proof.py` verifies the supported subset
for its environment:

1. archive checksum, safe extraction, one native executable, version, and help;
2. clean offline cache with no model, backend, or helper download;
3. core indexing and retrieval publication to `retrieval_mode=full`;
4. exact engine identity and policy;
5. packet and search through the plugin launcher;
6. two repositories using one engine instance and one model load;
7. process restart and content-addressed model reuse;
8. absence of embedding-server, port, lease, and consent state.

## Workflow ownership

| Workflow | Environment | Claim boundary |
| --- | --- | --- |
| `retrieval-engine-smoke.yml` | hosted Linux/Windows | explicit CPU source/protocol behavior |
| `packaged-platform-proof.yml` | hosted package matrix | offline packaged behavior; CPU explicit where required |
| `macos-metal-proof.yml` | protected Apple Silicon | packaged Metal, physical adapter, smoke, offload |
| `windows-vulkan-proof.yml` | protected Windows GPU | packaged Vulkan, physical adapter, smoke, offload |

Linux acceleration remains unclaimed until equivalent protected Vulkan hardware
evidence exists.

## Performance and quality acceptance

Measure cold initialization, warm query percentiles, bulk documents/second,
peak RSS, GPU memory, vector parity, retrieval quality, multi-repository reuse,
and restart reuse separately. Compare engine candidates in the same release
build and machine. No retrieval-quality loss is accepted; a repeatable
throughput, latency, or memory regression blocks promotion. The benchmark page,
not this architecture contract, owns time-specific baselines.

## Focused failure boundaries

Tests cover exact model/build identity, corrupt materialization, explicit CPU
permission, prohibited fallback, software-adapter rejection, process-wide reuse,
producer migration, generation-coherent reads, publication drift with one
bounded retry, and owned cleanup below its trusted root. Tests that only
supervised the removed embedding process do not belong in the suite.
