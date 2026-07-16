# Llama Sys Subsystem

`codestory-llama-sys` is the internal native embedding boundary. It compiles
llama.cpp/ggml and the pinned CodeRankEmbed Q8 model contract for the CodeStory
release package. It has no public product or network surface.

## Build identity

`model-contract.json` is the checked-in compatibility descriptor for acquisition,
embedding semantics, vector format, tokenizer/config identity, producer
identity, license provenance, and the llama source revision. Retrieval owns
how those declared semantics are applied. The explicit preparation script
consumes the descriptor and publishes a verified workspace build asset. `build.rs`
consumes the same contract, never starts a process or performs network access,
and requires `CODESTORY_EMBED_MODEL_SOURCE` for release builds. It accepts only
an explicit regular file, copies into a create-new temporary file, closes and
re-verifies the bytes, then atomically publishes without replacing an existing
path. Development builds may omit the model so source checks remain practical,
but such a binary cannot claim full product retrieval.

The producer name and version from that same contract are embedded in the
product runtime ID. Retrieval combines that identity with its own authoritative
vector-policy constants when it writes persisted producer evidence. Changing
either implementation identity or version makes an older vector generation
ineligible for reuse and requires a complete rebuild.

The build also embeds a parseable `codestory-native-engine-v1` marker with the
target triple, native binary architecture, linkage and backend-loading mode,
compiled backend set, llama crate/source identity, exact model digest, a stable
digest of the model/vector/tokenizer contract, model presence, and producer
version. macOS keeps Metal built in. Windows and Linux load packaged core, CPU,
and Vulkan modules at runtime so the base executable does not acquire a
mandatory Vulkan-loader dependency. Release packaging inspects PE imports, ELF
`DT_NEEDED`, or Mach-O load commands, verifies the complete target-specific
runtime set, and records every artifact and digest in
`codestory-native-manifest.json`.

## Runtime contract

`src/lib.rs` owns:

- verified content-addressed model materialization for mmap;
- compiled and runtime backend capability reporting;
- exact execution of the caller's backend/device-class request, including
  optional software-adapter rejection and no implicit fallback;
- runtime loading of only the native backend modules shipped beside the
  executable on Windows and Linux;
- one model worker with bounded query and bulk queues;
- owner-thread residency with a 60-second idle unload and automatic wake;
- RAII residency leases for operations that must retain one load generation;
- query priority between bulk batches;
- timed smoke, initialization, adapter, and model-load diagnostics;
- post-encode backend observations for execution backend/device, model-layer
  placement, resident tensor count/bytes, execution nodes, and successful
  encode count.

The caller supplies the exact model ID/digest, dimension, pooling, token and
batch limits, smoke input, backend, and device class. The crate checks only the
compiled model compatibility facts it must execute, currently dimension and
pooling, and returns raw vectors plus engine diagnostics. Model selection,
prefixes, normalization, vector schema, batching policy, persisted evidence,
and CPU/accelerator policy live in `codestory-retrieval`. The binding does not
select a project, publish a retrieval generation, or decide whether
packet/search may serve.

## Extension rules

A model, tokenizer, pooling, normalization, vector-dimension, backend, or ggml
change creates a new producer identity and requires retrieval rebuild and
same-run performance/quality evidence. Add capability reporting here; add
product selection and fallback policy in retrieval. Production must never
respond to accelerator failure by silently selecting CPU.

## Failure signatures

- the runtime downloads a model or backend;
- more than one model context is loaded in a multi-project process;
- an idle task retains its model, context, or accelerator allocation after the
  residency window;
- WARP, llvmpipe, lavapipe, or SwiftShader satisfies accelerated policy;
- the base Windows or Linux executable imports the Vulkan loader, or a packaged
  backend module is missing or has an unrecorded native dependency;
- requested layer counts or process-memory deltas are presented as observed
  accelerator execution;
- model bytes are materialized without digest verification and atomic publish;
- backend details leak into normal user-facing readiness messages.

See [retrieval design](../retrieval-design.md) for publication policy and
[retrieval verification](../../testing/retrieval-architecture.md) for proof
tiers.
