# Architecture History

This page is a short summary of the durable architecture choices that still shape the current workspace.

Use the architecture pages as the source of truth for how the system works today.

## Durable Boundaries

CodeStory stays split into durable owning crates so contributors can reason about source-of-truth responsibilities without tracing every call path first.

- boundaries and dependency direction: [architecture overview](architecture/overview.md)
- runtime-owned orchestration path: [runtime execution path](architecture/runtime-execution-path.md)

## Workspace Plans, Indexer Execution, Store Persistence

Refresh planning stays in `codestory-workspace`, parse and resolution work stay in `codestory-indexer`, and persistence plus snapshot lifecycle stay in `codestory-store`.

- planning and ownership overview: [architecture overview](architecture/overview.md)
- full pipeline from CLI to store state: [indexing pipeline](architecture/indexing-pipeline.md)
- crate-specific ownership: [workspace subsystem](architecture/subsystems/workspace.md), [indexer subsystem](architecture/subsystems/indexer.md), [store subsystem](architecture/subsystems/store.md)

## Snapshot Lifecycle Stays Store-Owned

Runtime decides when to run full or incremental indexing, but staged-build preparation, staged publish, live snapshot refresh, and derived-state invalidation remain store mechanics.

- runtime orchestration: [runtime execution path](architecture/runtime-execution-path.md)
- storage responsibilities: [store subsystem](architecture/subsystems/store.md)
- index-to-snapshot path: [indexing pipeline](architecture/indexing-pipeline.md)

## Retrieval And Grounding Stay Runtime-Orchestrated

Search ranking, grounding assembly, fallback reporting, and other workflow orchestration stay in runtime instead of leaking into CLI or storage adapters.

- runtime ownership: [runtime subsystem](architecture/subsystems/runtime.md)
- command path context: [runtime execution path](architecture/runtime-execution-path.md)

Keep future architecture guidance in the owning architecture pages instead of reviving a separate ADR track.
