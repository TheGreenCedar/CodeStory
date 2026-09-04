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
- explicit semantic-projection republish from one pinned complete stored core,
  without source discovery, parsing, or source reads;
- grounding, trails, symbol workflows, target context, search, and packet
  assembly;
- one packet-probe normalization and resolution path for exact paths, stable
  symbol IDs, qualified symbols, file-scoped symbols, free queries, and
  generation-bound continuations;
- one descriptor-only admission pass across the unchanged question and all
  typed free queries before candidate source or graph hydration;
- assembly of `PacketCompilationInputV1` and projection of the pure compiler's
  bounded evidence product;
- managed retrieval preparation and user-facing gap mapping;
- generation-coherent candidate resolution and one bounded publication retry.

## Main paths

- `src/lib.rs` and `src/services.rs`: project/index services and retained state
- `src/grounding.rs` and `src/support.rs`: grounding and support assembly
- `src/search/`: runtime search state and graph-native documents
- `src/agent/`: execution residuals (orchestrator, retrieval-primary, packet
  batch/probe/search, traces). Packet *planning* lives in
  [`codestory-agent`](agent.md).
- `src/controller_bookmarks.rs`: annotation CRUD against the store's sidecar

## Publication contract

A complete core publication may retain verified malformed UTF-8 source with a
`malformed` coverage gap. It retains the file identity and content digest, not
structural units, symbol documents, or proof facts. Both full and incremental
refresh reverify those bytes before publication. A transition from valid to
malformed removes the old projection; unchanged malformed source does not
trigger a retry. Unreadable source, incomplete discovery, source drift, and
collector failures still prevent publication.

Runtime publishes the core index through store, then asks retrieval to finalize
immutable lexical/vector/SCIP state when a broad operation needs it. On reads it
requires query hits and candidate resolution to share one
`RetrievalPublicationIdentity`, holds the core read and generation leases, and
revalidates before returning. Publication drift permits one bounded retry.

Work that one publication fixes is cached against that publication's identity
rather than repeated per pin. The canonical symbol-name map is the example: it
is keyed by storage path plus the full core publication identity, and its
stored row count is re-observed on every reuse, so a canonical table that moved
under a stable publication is restreamed instead of answered from a stale map.
A public operation also arms a `codestory_workspace::SourceFreshnessScope`, so
its pre-build check, nested wrappers, and post-build check share one source
content pass over files whose recorded content hash is already known;
`AgentRetrievalTraceDto::source_freshness_telemetry` publishes the resulting
pass counters. The scope never answers the post-build "source inputs changed
while running {operation}" check from the memo: that check re-derives every
verdict from content.

Every path that replaces core projections moves user annotations into the store
sidecar first, and the ordering is enforced by the type system rather than by
convention: `index_full_for_runtime` and `index_incremental_for_runtime` demand
an `AnnotationsOwned`, which only
`ensure_annotations_owned_before_core_replacement` can mint. A future refresh
entry point that forgets the cutover does not compile.

The per-user engine authority belongs to retrieval/llama-sys and runs in the
automatically managed embedding server. Runtime may cause lazy server and
engine activation and hold publication leases, but cannot reconfigure the
engine per project or infer readiness from `retrieval_mode` alone.

Runtime accepts a bounded-source exclusion set only from a complete inventory
and verified structural collector results, publishes it with the candidate
core, and requires its bound manifest on freshness and read surfaces. `files`
exposes those paths as source inventory with observed byte/unit bounds and
explicit false graph and semantic coverage; packet and search never treat them
as indexed evidence.

Semantic document preparation normalizes the file table once and retains
display/read paths by owning file-node identity. Symbols resolve those paths
through `file_node_id`; runtime does not duplicate path strings or retain a
second owned display-name map per symbol. The current all-node load and graph
lookup remain a separate bounded-streaming concern. Index telemetry exposes
selected symbols, retained context files and path bytes, and lookup entries so
that boundary stays visible.

Projection-only republish is a writer, never an activation or observational
repair path. Runtime accepts only current stored document contracts, rebuilds
graph-derived context and dense selection, and delegates atomic old-or-new core
promotion to store. Retrieval generation construction remains retrieval-owned;
the core replacement therefore makes broad retrieval stale until the existing
retrieval index command publishes a matching generation.

## Extension rules

- put reusable product workflows here and expose typed contract DTOs;
- keep command parsing/rendering in CLI and persistence in store;
- extend packet/search through the existing retrieval-primary path rather than
  creating a second scoring or readiness system.
- keep probe resolution metadata diagnostic: a requested probe may constrain
  exact identity resolution but cannot promote rank, materiality, sufficiency,
  or an answer-stage order.

## Failure signatures

- CLI or MCP adapter composes product semantics;
- candidate IDs resolve against whatever core database is current;
- core indexing success is reported as full retrieval readiness;
- a project operation mutates per-user server or process defaults.
