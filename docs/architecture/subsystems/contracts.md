# Contracts Subsystem

`codestory-contracts` owns the shared model used by the final V2 crates.

## Ownership

- graph/domain primitives such as nodes, edges, occurrences, trail config, and bookmarks
- adapter-facing DTOs for CLI and runtime services
- grounding and trail contract groupings
- shared event-bus and domain event types

## Entry Points

- `crates/codestory-contracts/src/graph.rs`
- `crates/codestory-contracts/src/api.rs`
- `crates/codestory-contracts/src/events.rs`
- `crates/codestory-contracts/src/grounding.rs`
- `crates/codestory-contracts/src/trail.rs`

## How To Extend It

- Add new shared graph or trail primitives under `graph/` and export them through `graph.rs`.
- Add new request/response DTOs under `api/` and export them through `api.rs`.
- Add cross-layer lifecycle or boundary events under `events/` and export only the public ones.
- Keep runtime-local commands and CLI-only formatting types out of this crate.

## Failure Signatures

- runtime or CLI imports a private type from another crate instead of using `codestory-contracts`
- wildcard exports creep back into the public surface
- UI-only or adapter-only types leak into `events`
