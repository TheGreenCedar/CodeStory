# Runtime Subsystem

`codestory-runtime` is the only product orchestration layer. It decides which
owning service to call and assembles cited product results; it does not own
adapter syntax, SQLite mechanics, parsers, or model execution.

## Ownership

- project open, summary, and refresh orchestration;
- full and incremental indexing across workspace, indexer, and store;
- complete source-inventory classification and publication of verified
  source-policy exclusions before parser scheduling;
- graph-native symbol-document and dense-anchor synchronization;
- grounding, trails, symbol workflows, target context, search, and packet
  assembly;
- managed retrieval preparation and user-facing gap mapping;
- generation-coherent candidate resolution and one bounded publication retry.

## Main paths

- `src/lib.rs` and `src/services.rs`: project/index services and retained state
- `src/grounding.rs` and `src/support.rs`: grounding and support assembly
- `src/search/`: runtime search state and graph-native documents
- `src/agent/`: packet, retrieval-primary, planning, and evidence workflows

## Publication contract

Runtime publishes the core index through store, then asks retrieval to finalize
immutable lexical/vector/SCIP state when a broad operation needs it. On reads it
requires query hits and candidate resolution to share one
`RetrievalPublicationIdentity`, holds the core read and generation leases, and
revalidates before returning. Publication drift permits one bounded retry.

The per-user engine authority belongs to retrieval/llama-sys and runs in the
automatically managed embedding server. Runtime may cause lazy server and
engine activation and hold publication leases, but cannot reconfigure the
engine per project or infer readiness from `retrieval_mode` alone.

Runtime accepts an oversized-source exclusion set only from a complete
inventory, publishes it with the candidate core, and requires its bound
manifest on freshness and read surfaces. `files` exposes those paths as source
inventory with explicit false graph and semantic coverage; packet and search
never treat them as indexed evidence.

## Extension rules

- put reusable product workflows here and expose typed contract DTOs;
- keep command parsing/rendering in CLI and persistence in store;
- extend packet/search through the existing retrieval-primary path rather than
  creating a second scoring or readiness system.

## Failure signatures

- CLI or MCP adapter composes product semantics;
- candidate IDs resolve against whatever core database is current;
- core indexing success is reported as full retrieval readiness;
- a project operation mutates per-user server or process defaults.
