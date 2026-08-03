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
- language-support profiles and evidence tiers;
- sealed validation receipts for immutable artifacts.

## Entry points

- `src/graph.rs` and `src/graph/`: graph domain
- `src/api.rs` and `src/api/`: adapter-facing DTOs, errors, IDs, and events
- `src/grounding.rs` and `src/trail.rs`: evidence groupings
- `src/workspace.rs`: shared workspace contracts
- `src/language_support.rs`: source-of-truth support labels
- `src/validation_receipts.rs`: sealed, process-local validation receipts

## Sealed validation receipts and their platform limit

A receipt caches one successful validation of an immutable artifact and is
reusable only while the artifact's native identity matches the observation the
receipt was sealed to. A failing validation is never cached and removes any
receipt the key already held.

The seal is only as strong as the metadata the platform reports, and Windows
reports less than Unix does. On Unix a seal carries the device/inode pair and
the inode-change instant, so an in-place rewrite breaks it even when the writer
restores the modification time, and a replacement breaks it even when the new
bytes and timestamps match. `std::fs` exposes neither field on Windows, so a
seal there compares presence, length, the creation and modification instants,
and the read-only bit. **On Windows a same-length in-place rewrite that restores
the modification time satisfies the seal, and the receipt answers for bytes it
never read.** A receipt proves the artifact was not casually touched on that
platform; it does not prove the bytes are the ones the validation read, and
nothing that must detect deliberate corruption may rest on a receipt alone
there. `SealFidelity` names which of the two an observation is, and
`ArtifactSeal::fidelity` reports it, so callers and tests read the limit off the
observation instead of assuming the stronger case.

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
- a support or readiness label exists without one source-of-truth definition;
- a sealed receipt is treated as corruption detection on a platform whose
  observations are timestamps-only.
