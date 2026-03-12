# ADR 0004: Snapshot Lifecycle Ownership

```mermaid
stateDiagram-v2
    [*] --> RuntimeChoosesRefresh
    RuntimeChoosesRefresh --> OpenStagedBuild: SnapshotStore opens staged path
    OpenStagedBuild --> RefreshGrounding: SnapshotStore refreshes summary/detail grounding
    RefreshGrounding --> PublishSnapshot: full refresh completes
    RefreshGrounding --> DiscardStagedBuild: staged refresh is abandoned
    RefreshGrounding --> InvalidateDerivedData: incremental writes land
    PublishSnapshot --> [*]
    DiscardStagedBuild --> [*]
    InvalidateDerivedData --> [*]
```

## Current State

`codestory-store::SnapshotStore` now owns staged snapshot pathing, staged-build open, staged publish or discard, and summary or detail grounding refresh operations.

`codestory-runtime` now decides when to run a full or incremental index, and it uses the store snapshot surface for staged publish and refresh.

## Target State

All snapshot lifecycle transitions stay behind `codestory-store`, including staged publish preparation, summary or detail refresh, and invalidation of derived grounding data.

## Decision

Keep snapshot lifecycle responsibilities store-owned so runtime and CLI layers orchestrate indexing without duplicating SQLite snapshot mechanics.

