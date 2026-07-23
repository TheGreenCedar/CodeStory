# Contracts Subsystem

`codestory-contracts` owns stable types shared across source-of-truth layers,
runtime, and adapters. It contains contracts, not orchestration or rendering.

## Ownership

- graph nodes, edges, occurrences, locations, bookmarks, and trails;
- workspace and refresh DTOs;
- API requests, responses, IDs, structured errors, and lifecycle events;
- grounding, readiness, status, publication, and symbol-workflow DTOs;
- tagged packet probes plus normalized resolution, ambiguity, and rejection
  metadata;
- language-support profiles and evidence tiers.

## Entry points

- `src/graph.rs` and `src/graph/`: graph domain
- `src/api.rs` and `src/api/`: adapter-facing DTOs, errors, IDs, and events
- `src/grounding.rs` and `src/trail.rs`: evidence groupings
- `src/workspace.rs`: shared workspace contracts
- `src/language_support.rs`: source-of-truth support labels

## Extension rules

- add a type here when two owning layers must exchange the same stable meaning;
- keep storage schemas, runtime planners, and CLI formatting private to their
  owning crates;
- prefer closed enums and structured errors at trust and readiness boundaries;
- preserve wire names deliberately when implementation vocabulary changes.

## Failure signatures

- runtime or CLI imports another crate's private type instead of a contract;
- DTOs perform I/O or choose product behavior;
- adapter-only formatting types become shared domain concepts;
- a support or readiness label exists without one source-of-truth definition.
