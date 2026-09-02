# Agent Subsystem

`codestory-agent` owns Horizon A's prompt-blind packet seed planning and pure
evidence-policy helpers. It passes the unchanged question to generic retrieval
and records typed free-query probes as additional generic queries. It does not
infer paths, symbols, relations, answer stages, material roles, or sufficiency
from prompt wording. Repository-derived evidence selection lands separately
under #2106.

The crate depends on `codestory-contracts` alone. It cannot activate a
project, open or write storage, execute retrieval, retry a publication, or
move readiness. The only runtime state it may read is what the host already
pinned, through the `PinnedReader` trait implemented by runtime.

## Ownership

- the unchanged-question generic retrieval plan;
- deterministic query deduplication without English or domain taxonomies;
- `PinnedReader`, the only allowed view of pinned runtime state.

## Entry points

- `src/lib.rs`: crate contract and module map
- `src/packet_plan.rs` and `src/planning.rs`: unchanged-question retrieval
  planning and literal query deduplication
- `src/citation.rs` and `src/packet_evidence.rs`: compatibility metadata for
  non-compiler search/citation surfaces; these fields have no admission,
  ranking, protection, or sufficiency authority in packet compilation
- `src/pinned_reader.rs`: the pin trait runtime implements

## What stays in runtime

Runtime owns generic retrieval, packet-wide descriptor admission, exact-probe
resolution, hydration, publication retry, interim packet finalization, public
projection, and budgets. Those modules live under
`crates/codestory-runtime/src/agent/` because they need `AppController`, store,
retrieval, or filesystem access.

## Extension rules

- keep question handling to unchanged generic retrieval and caller-supplied
  typed probes;
- add retrieval, admission, hydration, publication retry, and assembly in
  runtime;
- never import `codestory-runtime`, `codestory-store`, `codestory-retrieval`,
  or `codestory-workspace` from this crate;
- read pinned state only through `PinnedReader`.

## Failure signatures

- prompt tokens, task classes, obligations, roles, carriers, or answer stages
  steer planning, admission, hydration, finalization, or capping;
- this crate starts retrieval, indexing, or a publication retry;
- a planner reads ambient process state instead of a pin;
- packet output asserts answer sufficiency.

See [runtime](runtime.md) for assembly and retry, and
[retrieval](retrieval.md) for fail-closed query execution.
