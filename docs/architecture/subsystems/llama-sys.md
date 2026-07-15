# Llama Sys Subsystem

`codestory-llama-sys` is the internal native embedding boundary. It compiles
llama.cpp/ggml and the pinned CodeRankEmbed Q8 model contract into the
CodeStory release executable. It has no public product or network surface.

## Build identity

`build.rs` pins the model filename, size, SHA-256, llama source revision, and
ggml build identity. Release builds invoke the checksum-pinned preparation
script, which reuses or prepares the verified workspace build asset before Rust
independently verifies and embeds its bytes.
`CODESTORY_EMBED_MODEL_SOURCE` remains an optional explicit input for hermetic
or offline builds and fails closed when invalid. Development builds may omit the
model so source checks remain practical, but such a binary cannot claim full
product retrieval.

Native features select Metal on macOS and Vulkan on Windows/Linux. CPU
execution remains available only through the caller's explicit
`CODESTORY_EMBED_ALLOW_CPU=1` policy.

## Runtime contract

`src/lib.rs` owns:

- verified content-addressed model materialization for mmap;
- physical backend and adapter selection, including software-adapter rejection;
- one model worker with bounded query and bulk queues;
- owner-thread residency with a 60-second idle unload and automatic wake;
- RAII residency leases for operations that must retain one load generation;
- query priority between bulk batches;
- the CodeRank query prefix (`Represent this query for searching relevant code: `),
  no document prefix, batching, CLS pooling, and L2 normalization;
- timed smoke, initialization, offload, adapter, and model-load diagnostics.

The crate returns embeddings and engine diagnostics. It does not select a
project, publish a retrieval generation, or decide whether packet/search may
serve.

## Extension rules

A model, tokenizer, pooling, normalization, vector-dimension, backend, or ggml
change creates a new producer identity and requires retrieval rebuild and
same-run performance/quality evidence. Production must never respond to
accelerator failure by silently selecting CPU.

## Failure signatures

- the runtime downloads a model or backend;
- more than one model context is loaded in a multi-project process;
- an idle task retains its model, context, or accelerator allocation after the
  residency window;
- WARP, llvmpipe, lavapipe, or SwiftShader satisfies accelerated policy;
- model bytes are materialized without digest verification and atomic publish;
- backend details leak into normal user-facing readiness messages.

See [retrieval design](../retrieval-design.md) for publication policy and
[retrieval verification](../../testing/retrieval-architecture.md) for proof
tiers.
