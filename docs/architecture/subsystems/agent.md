# Agent Subsystem

`codestory-agent` owns packet planning. It decides what evidence a task needs:
prompt terms, claim-profile coverage labels, evidence roles and carriers, citation
scoring, and the deduplicated query plan. It does not run that plan.

The crate depends on `codestory-contracts` alone. It cannot activate a
project, open or write storage, execute retrieval, retry a publication, or
move readiness. The only runtime state it may read is what the host already
pinned, through the `PinnedReader` trait implemented by runtime.

## Ownership

- packet terms, claims, obligations, and coverage labels;
- evidence roles, carriers, and citation scoring;
- probe and required-probe planning;
- the query plan handed to runtime for execution;
- `PinnedReader`, the only allowed view of pinned runtime state.

## Entry points

- `src/lib.rs`: crate contract and module map
- `src/planning.rs` and `src/packet_plan.rs`: plan construction
- `src/packet_terms.rs`, `src/packet_flow_requirements.rs`, `src/packet_obligations.rs`: prompt terms, coverage labels, and exact-probe obligations
- `src/packet_evidence_roles.rs` and `src/packet_evidence_carriers.rs`: how a citation can count
- `src/packet_scoring.rs` and `src/citation.rs`: ranking inside the plan
- `src/pinned_reader.rs`: the pin trait runtime implements

## What stays in runtime

Runtime still owns execution residuals that need `AppController`, store,
retrieval, or filesystem writes: `orchestrator`, `retrieval_primary`,
`packet_batch`, `packet_probe`, `packet_search`, traces, and budget/capping
that fold live step results. Those modules live under
`crates/codestory-runtime/src/agent/` on purpose. They are not planning.

## Extension rules

- add planning policy here; add execution, publication retry, and assembly in
  runtime;
- never import `codestory-runtime`, `codestory-store`, `codestory-retrieval`,
  or `codestory-workspace` from this crate;
- read pinned state only through `PinnedReader`.

## Failure signatures

- packet planning modules reappear under `codestory-runtime`;
- this crate starts retrieval, indexing, or a publication retry;
- a planner reads ambient process state instead of a pin;
- sufficiency *policy* is rewritten in retrieval while planning stays here.

See [runtime](runtime.md) for assembly and retry, and
[retrieval](retrieval.md) for fail-closed query execution.
