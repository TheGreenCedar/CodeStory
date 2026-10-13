# Agent Subsystem

`codestory-agent` owns the pure halves of packet compilation: syntactic seed
planning and repository-derived evidence selection. The seed plan may pass the
unchanged question to generic retrieval, but raw wording never creates an exact
selector. Exact paths, canonical IDs, and qualified symbols enter only through
typed probes. Typed free-query probes remain generic retrieval seeds. The
compiler receives no question text. It selects only from admitted identities,
hydrated source, certain directed relations, ambiguity, parser completeness,
and publication identity.

The crate depends on `codestory-contracts` alone. It cannot activate a
project, open or write storage, execute retrieval, retry a publication, or
move readiness. The only runtime state it may read is what the host already
pinned, through the `PinnedReader` trait implemented by runtime.

## Ownership

- `RetrievalSeedPlanV1`, the sole post-request object allowed to retain the
  original wording;
- deterministic query deduplication without English or domain taxonomies;
- repository-derived source containment, path diversity, relation-forest, and
  byte-bound selection over `PacketCompilationInputV1`;
- `PinnedReader`, the only allowed view of pinned runtime state.

## Entry points

- `src/lib.rs`: crate contract and module map
- `src/packet_plan.rs` and `src/planning.rs`: unchanged-question retrieval seed
  planning and literal query deduplication
- `src/evidence_compiler.rs`: pure repository-derived packet compilation
- `src/citation.rs` and `src/packet_evidence.rs`: compatibility metadata for
  non-compiler search/citation surfaces; these fields have no admission,
  ranking, protection, or sufficiency authority in packet compilation
- `src/pinned_reader.rs`: the pin trait runtime implements

## What stays in runtime

Runtime owns generic retrieval, packet-wide descriptor admission, exact-probe
resolution, hydration, publication retry, compilation-input assembly, public
projection, and budgets. Those modules live under
`crates/codestory-runtime/src/agent/` because they need `AppController`, store,
retrieval, or filesystem access.

## Extension rules

- keep the unchanged question confined to the generic query in
  `RetrievalSeedPlanV1`; copy exact selectors only from typed probes, and make
  compiler rules functions of typed repository evidence only;
- add retrieval, admission, hydration, publication retry, and assembly in
  runtime;
- never import `codestory-runtime`, `codestory-store`, `codestory-retrieval`,
  or `codestory-workspace` from this crate;
- read pinned state only through `PinnedReader`.

## Failure signatures

- the raw question, prompt tokens, task classes, obligations, roles, carriers,
  or answer stages cross into `PacketCompilationInputV1`;
- this crate starts retrieval, indexing, or a publication retry;
- a planner reads ambient process state instead of a pin;
- compiler output asserts answer sufficiency.

See [runtime](runtime.md) for assembly and retry, and
[retrieval](retrieval.md) for fail-closed query execution.


Runtime may supply an optional original callable/body extent from bounded parsing
of the same verified source bytes. Exact Core line/byte-column matching and the
source digest bind that observation; unsupported, erroneous, conflicting or
missing observations stay unknown. Selection credits direct delivered body
progress before speculative call reach, with completion credited only over the
original callable extent. A fully delivered declaration or requested subrange
is not evidence of a complete executable body. This does not assert behavior
or answer sufficiency. Public projection retains exact continuation locators
individually within its unchanged measured byte cap and routing identities.

The existing complete-first proposal family uses that same body-progress
objective while preserving exact requests and the prior order on ties. Unknown
metadata retains ordinary source and navigation credit; it does not establish
that a body is absent. Authenticated body progress nevertheless ranks ahead of
unknown ordinary context, which can bias selection across languages or legacy
publications. The bounded prefix/exchange search does not guarantee a global
optimum, and a large early body can still block a better combination. Executed
normal-producer coordinate controls and the captured full-frame replay are
required before claiming a packet-quality improvement.
