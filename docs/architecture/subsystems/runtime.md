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
- `src/served_lookup.rs` reads live source for `find`, `refs` and `read`. It attaches a call binding from a pinned publication only after matching the caller and callee source. Other files involved in resolution may have changed. Refresh guidance reports drift observed during the lookup.
- Implementation searches rank the path, name and definition text stored for each indexed function or method. Before showing code, the runtime checks the current file and the exact definition range. It shows verified call sites and can follow a literal reference into another function. A literal reference is labelled separately when the binding is unknown.
- When an unchanged published core lacks a usable function index, an explicit refresh can build it alongside the sealed generation. The replacement belongs to that exact core and run. The existing broad-search data and active publication stay usable throughout repair.
- For tracked edits, the workspace compares a pinned Git HEAD tree with the current worktree. The runtime maps changed line ranges to a matching indexed file on either side. It labels pre-edit declarations and verifies the caller's current source before showing a call site.
- `src/search/`: runtime search state and graph-native documents
- `src/agent/`: execution residuals (orchestrator, retrieval-primary, packet
  batch/probe/search, traces). Packet *planning* lives in
  [`codestory-agent`](agent.md).
- `src/controller_bookmarks.rs`: annotation CRUD against the store's sidecar

## Publication contract

A complete core publication may carry a structural file rejected by strict
UTF-8 decoding or format validation as a `binary` or `malformed` coverage gap.
The publication keeps its verified file identity and digest. It supplies no
structural, callable, symbol, or proof projection for that file. Full and
incremental refresh reverify the bytes before publishing. A readable file
that becomes opaque loses its old projection. An unchanged, verified opaque
file stays out of the refresh plan and counts as omitted in callable coverage.

Unreadable files, source drift, incomplete discovery, failed collectors, and
opaque claims without verified source or with surviving projections block
publication. Covered files remain searchable when a verified opaque gap exists,
and whole-project coverage reports the gap.

Runtime publishes the core index through store, then asks retrieval to finalize
immutable lexical/vector/SCIP state when a broad operation needs it. On reads it
requires query hits and candidate resolution to share one
`RetrievalPublicationIdentity`, holds the core read and generation leases, and
revalidates before returning. Publication drift permits one bounded retry.

Cache rehydrate selects its source's physical core generation after taking the
source writer guard. Schema, file count, freshness, space accounting, and the
snapshot copy use that one database; workspace-owned exclusions still use the
logical cache path. An absent target is checked for capacity through an
existing filesystem ancestor before its persistent writer lock is created,
then checked again under that lock before staging.

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

The controller rebinding pass runs after a committed core independently of
resident cache refresh. If it fails, the saved annotation evidence remains at
its last verified generation; full, incremental, and semantic-only core writers
must catch up against the current complete core before advancing again. Legacy
or interrupted standalone cores without a complete generation remain eligible
for managed recovery.

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

## Verified source inspection

Function-body snippets qualify a bounded closure against the already verified
source buffer before rendering an indexed extent. The Go lexical mode supports
named `func` declarations and methods, division, raw backtick strings, and nested
`interface`/`struct` signature types. Type/interface/variable anchors and
body-less declarations remain line-context fallbacks; the scanner never borrows
a later callable's body. A multiline result prefix ending in `interface` or
`struct` before its opening brace remains unsupported and falls back explicitly.
The existing scan/byte caps and other languages' ambiguity guards still apply.

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

## Indexed call-path verification

The hidden call-path kernel shares one request budget across strict search,
exclusion-tolerant search, reachable prefixes and final fact/state scans. Its
32,768 work units include deterministic ordering allowances; sorting and
retained prefix storage are limited to 1,024 entries. Prefixes retain complete
receipt and edge histories, with quota reserved before copying them. Exhaustion
returns `Unknown` with `kernel_search_budget_exceeded` and no connected receipts;
an incomplete search establishes neither a longest prefix nor a refutation.
Known input unavailability precedes exploration. An input exceeding the budget
just to scan fails closed without inferring an unscanned unavailable suffix.
The public `verify_indexed_direct_calls` operation uses this kernel; the
qualification driver inspects its detailed traces through an optional feature.
The bound applies to call-path verification, not packet/search latency or
runtime execution.
