# Retrieval Verification Architecture

This page defines evidence tiers and acceptance assertions. The system design
lives in [retrieval design](../architecture/retrieval-design.md); current
measurements live in
[embedding backend benchmarks](embedding-backend-benchmarks.md).

## Evidence tiers

| Tier | Required evidence | Supported claim |
| --- | --- | --- |
| Source | locked checks and focused crate tests | source compiles and contracts hold |
| Hosted package | exact source/tree/archive/executable, inspected native imports, server/protocol/constant manifest, same-user IPC, explicit CPU policy, complete qualification record | package contract and CPU runtime are coherent; no acceleration claim |
| Protected hardware | same manifest-bound package and qualification record, CPU disallowed, physical backend/adapter, backend-observed post-encode telemetry | Metal or Vulkan and the server contract work on that machine |
| Product runtime | installed plugin launcher, full retrieval, packet/search, two independent hosts sharing one server/engine/load | installed agent path is coherent |
| Restart | new process reuses verified materialized model content | content-addressed cache reuse works |
| Performance/quality | same-run measurements and holdout gates | an engine change is promotion-eligible |

A lower tier cannot support a higher-tier claim.

## Required engine assertions

After an engine-initializing operation, packaged proof reads
`codestory://diagnostics/retrieval-engine` and verifies:

- endpoint authority, listener, server process, engine owner, native worker,
  load generation, and model-load identities;
- exact model digest and ggml build identity;
- backend, physical adapter, and `accelerated` or `cpu_explicit` policy;
- engine instance and model-load count;
- initialization and live-smoke timing;
- materialized path, digest, and reuse state;
- backend-observed execution device/backend, requested and observed model-layer
  counts, resident accelerator tensor count/bytes, execution-node count, and a
  successful encode counter that advances across live requests.

Accelerated proof rejects software adapters and unknown or inferred execution
evidence. Requested layer counts and process/GPU-memory deltas are observational
unless the post-encode backend callback confirms execution and residency.
Hosted proof requires explicit CPU permission; absent GPU hardware does not
imply permission.

## Packaged product assertions

`.github/scripts/check-packaged-agent-proof.py` verifies the supported subset
for its environment:

1. archive checksum, safe extraction, one native executable, version, and help;
2. one native manifest bound to the binary digest, format, architecture,
   target-specific linkage/loading mode, inspected native dependencies,
   packaged runtime artifacts, compiled backends, model, llama source, and
   producer;
3. manifest-bound server protocol, frozen constant set, measurement protocol,
   clean offline cache, and no model or backend download;
4. core indexing and retrieval publication to `retrieval_mode=full`;
5. exact manifest-matching engine/model/backend identity and policy before and
   after restart;
6. packet and search through the plugin launcher;
7. two independent plugin hosts and repositories using one authority, listener,
   server, engine owner, native worker, load generation, and model load;
8. server exit/respawn and content-addressed model reuse;
9. an encode counter that advances across real retrieval requests;
10. complete preregistered client-death, server-crash, worker-stall, queue,
    incompatible-owner, frozen-owner, and true-idle evidence;
11. absence of TCP, PID ownership, owner manifests, in-process fallback,
    project leakage, and consent state.

The package manifest proves compiled capability only. Accelerator execution is
a separate protected-hardware result, and neither package nor execution proof
is an answer-quality claim. Windows and Linux packages runtime-load their
recorded backend modules; their base executables must not require a Vulkan
loader, so help, status, local navigation, and explicit CPU execution can start
without one. Linux packages remain an explicit GPU non-claim.

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

Measure existing-owner connect, spawn convergence, first residency and product
ready, warm query/bulk IPC, bulk documents/tokens per second, useful busy retry,
true-idle exit, total CodeStory process memory, GPU memory, vector parity,
retrieval quality, multi-process reuse, and restart reuse separately. Native
model/backend candidates use the same-build private comparison described by
the embedding benchmark contract. The per-user server cutover instead imports
the exact-head `publishable-three-repeat-packet/v1` artifact, verifies its
source commit and tree plus complete row/repeat coverage, and derives a required
1.0 packet pass rate. Pre-fault and post-replacement searches are retained only
as crash-recovery consistency evidence. Freeze thresholds before the
qualification run. No retrieval-quality loss is accepted; a repeatable
throughput, latency, or memory regression blocks promotion. The checked-in
constant set and qualification protocol, not prose on this page, own the
candidate-specific values.

## Focused failure boundaries

Tests cover exact model/build identity, corrupt materialization, explicit CPU
permission, prohibited fallback, software-adapter rejection, per-user reuse,
producer migration, generation-coherent reads, publication drift with one
bounded retry, lease loss, malformed frames, queue pressure, cancellation,
same-user endpoint authority, idle exit, frozen-owner non-takeover, and owned
cleanup below its trusted root.

See the
[per-user server qualification contract](per-user-embedding-server-qualification.md)
for the exact scenario and retained-evidence boundary.
