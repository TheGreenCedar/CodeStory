# Retrieval Design

CodeStory serves agent packet/search evidence only from a current, complete
publication. The graph database, lexical shard, semantic vectors, SCIP data,
and embedding producer identity are one fail-closed contract.

## Runtime shape

The release contains one CodeStory executable. A small internal sys crate links
llama.cpp and ggml into it, and the checksum-pinned BGE-base-en-v1.5 Q8 GGUF is
embedded as release data. When mmap is required, the process atomically
materializes those bytes to a content-addressed file below the CodeStory cache.
The file is disposable and verified before use. Retrieval never downloads a
model or helper executable.

One lazily initialized model and accelerator context is shared process-wide by
every repository opened through multi-project stdio. One-shot CLI commands pay
their own initialization cost; CodeStory does not use a daemon to conceal it.

| Platform | Production backend | Policy |
| --- | --- | --- |
| macOS Apple Silicon | Metal | Required and live-verified |
| Windows | Vulkan | Required and live-verified |
| Linux | Vulkan | Claimed only with protected hardware evidence |
| Hosted CI / maintainer diagnostic | CPU | Only with `CODESTORY_EMBED_ALLOW_CPU=1` |

There is no silent GPU-to-CPU fallback. Software adapters such as llvmpipe,
lavapipe, WARP, and SwiftShader are rejected for accelerated policy.

## Embedding contract

The product contract remains BGE-base-en-v1.5 Q8 GGUF with the pinned tokenizer,
query and document prefixes, CLS pooling, normalization, dimensions, batching,
and vector persistence format. These values participate in the producer and
manifest identity. A change requires a new identity and rebuild.

Maintainer health evidence includes:

- exact model SHA-256 and linked ggml build identity;
- backend and physical adapter identity;
- `accelerated` or `cpu_explicit` policy;
- timed live embedding smoke and initialization time;
- process engine instance and model load count;
- materialized model path/reuse state;
- model and offloaded layer counts plus live accelerator verification.

Normal plugin UX reduces that detail to whether retrieval is ready.

## Publication identity

Each published retrieval generation binds:

- core graph `generation_id` and `run_id`;
- lexical version and source/input fingerprint;
- semantic generation and Qdrant collection;
- graph artifact hash, symbol-document count, and dense-anchor count;
- embedding producer identity and schema/policy versions.

Readers pin the core SQLite snapshot and semantic-generation lease together.
Candidate resolution never reopens the current database halfway through a
query. Before returning packet/search output, runtime revalidates both identities
and returns `cache_busy` for one bounded retry when publication changed.

Writers stage and validate a complete candidate, rescan source inside the
publication fence, then publish atomically. Failure or drift leaves the prior
generation live. Old semantic generations with the former producer identity
are rebuilt once; there is no legacy execution branch.

## Query path

1. Validate cheap immutable generation/schema metadata.
2. Pin one complete retrieval publication.
3. Generate the query vector with the shared in-process engine.
4. Combine lexical, semantic, SCIP, and graph candidates under runtime policy.
5. Resolve evidence through the pinned core snapshot.
6. Revalidate publication and engine identity before returning.

Deep corpus validation runs at build, promotion, readiness, or explicit health
boundaries, never before every query.

## Readiness

`retrieval_mode=full` remains the agent-facing classification. Full mode
requires a current manifest, coherent graph/lexical/semantic identities, and a
live engine satisfying its explicit policy. Missing, stale, partial, ambiguous,
or mismatched evidence blocks packet/search while local graph navigation can
remain available.

Status and doctor are observational. A product tool call may initialize the
embedded engine and build missing retrieval state automatically; it never asks
the user to approve an internal subsystem.

## Ownership and cleanup

Retrieval artifacts live below the project cache and are removed only through
the shared owned-deletion boundary using a trusted root handle and relative
generation path. Cleanup never follows a previously validated pathname and
never removes resources outside a proved CodeStory ownership token.

There are no embedding endpoints, ports, leases, PIDs, repair workers, server
logs, or process shutdown records in this architecture.

## Performance gate

Changes to the engine compare incumbent and candidate inside the same release
build on the same machine. Measure cold initialization, warm queries, bulk
indexing, RSS, GPU memory, vector parity, quality, and multi-repository reuse
separately. The accepted historical reference is roughly 368-372 embedded
documents/sec, 84.7 ms cross-repository search p95, MRR@10 0.9824, Hit@10 1.0,
Hit@1 0.973, and 829-1,020 MB peak working set. Five percent is measurement
noise, not an allowed repeatable regression.

See [retrieval engine operations](../ops/retrieval-engine.md) and the
[testing architecture](../testing/retrieval-architecture.md).
