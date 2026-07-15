# Retrieval Verification Architecture

This page defines the evidence needed for CodeStory retrieval claims. Design
ownership is in [retrieval-design.md](../architecture/retrieval-design.md).

## Evidence tiers

| Tier | Required evidence | Claim |
| --- | --- | --- |
| Source | Locked checks and focused crate tests | Source compiles and contracts hold |
| Hosted package | Packaged executable, offline isolated cache, explicit CPU policy | Package contains everything needed; no acceleration claim |
| Protected hardware | Same packaged executable, CPU disallowed, exact backend/adapter, timed smoke and full layer offload | Metal or Vulkan works on that machine |
| Product runtime | Plugin launcher, full retrieval, successful packet/search, two repositories sharing one engine | Agent flow uses the packaged engine coherently |
| Restart | New process reuses the same verified materialized model without rewriting it | Content-addressed cache reuse works |
| Performance/quality | Same-run incumbent/candidate measurements and holdout gates | Candidate is eligible to replace incumbent |

A lower tier cannot support a higher-tier claim.

## Engine identity assertions

The packaged proof consumes the explicit
`codestory://diagnostics/retrieval-engine` resource after an engine-initializing
operation and requires these fields:

- `embedding_model_sha256`
- `embedding_ggml_build_identity`
- `embedding_backend`
- `embedding_adapter`
- `embedding_policy`
- `embedding_engine_instance_id`
- `embedding_model_load_count`
- `embedding_smoke_ms`
- `embedding_initialization_ms`
- `embedding_materialized_path`
- `embedding_materialized_reused`
- `embedding_accelerator_execution_verified`
- model and offloaded layer counts

Accelerated proof rejects software adapters and requires every model layer to
be offloaded. Hosted proof requires `cpu_explicit`; absence of a GPU does not
authorize CPU use without `CODESTORY_EMBED_ALLOW_CPU=1`.

## Packaged proof

`.github/scripts/check-packaged-agent-proof.py` verifies:

1. archive checksum, safe extraction, one native executable, version, and help;
2. clean isolated cache with network access disabled;
3. graph and retrieval indexing to `retrieval_mode=full`;
4. exact engine identity and policy;
5. search and packet through the plugin launcher;
6. two repositories using one engine instance and one model load;
7. restart and content-addressed model reuse;
8. absence of helper executables and process-lifecycle state.

The proof produces JSON artifacts. It does not install dependencies, download a
model, start a service, reserve a port, or request consent.

## CI routing

| Workflow | Environment | Required behavior |
| --- | --- | --- |
| `retrieval-engine-smoke.yml` | Hosted Linux/Windows | Explicit CPU policy; source and protocol contracts |
| `packaged-platform-proof.yml` | Hosted package matrix | Offline packaged execution; CPU explicit where hardware acceleration is unavailable |
| `macos-metal-proof.yml` | Protected Apple Silicon | Packaged Metal execution, physical adapter, live smoke, full offload |
| `windows-vulkan-proof.yml` | Protected Windows GPU | Packaged Vulkan execution, physical adapter, live smoke, full offload |

Linux GPU support is not claimed until an equivalent protected Vulkan workflow
produces real hardware evidence.

## Performance and quality

The private development comparison may contain both implementations only until
the gate is decided. The merged code contains neither the switch nor the old
implementation.

Measure separately:

- one-shot CLI cold initialization;
- warm query p50/p95/p99;
- bulk embedding documents/sec;
- process peak RSS and GPU memory;
- vector numerical parity;
- MRR@10, Hit@10, and Hit@1;
- two-repository warm reuse and model load count;
- process restart/materialization reuse.

The historical BGE-base Q8 reference is 368-372 documents/sec, 84.7 ms
cross-repository search p95, MRR@10 0.9824, Hit@10 1.0, Hit@1 0.973, and
829-1,020 MB peak working set. No quality loss is allowed. A repeatable
throughput, warm-latency, or memory regression blocks the cutover; 5% is only
the measurement-noise tolerance.

## Failure boundaries

Focused tests cover:

- exact model/build identity and corrupt materialization rejection;
- explicit CPU permission and prohibited fallback;
- software-adapter rejection;
- process-wide singleton reuse across repositories;
- manifest producer migration and generation-coherent reads;
- publication drift and one bounded retry;
- owned cleanup that cannot escape its trusted root;
- plugin capability UX without lifecycle or consent surfaces.

Do not retain tests whose only purpose was supervising an external embedding
process.
