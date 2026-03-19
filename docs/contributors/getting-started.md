# Contributor Setup

## First Commands

Run these from the repo root:

```powershell
cargo fmt --check
cargo check
cargo test
```

Run them serially. This workspace shares Cargo build locks.

If you touch graph extraction or semantic resolution, plan to run the fidelity suites from the testing matrix before you finish.

## Recommended Reading Order

Build a mental model in this order before editing the biggest implementation paths:

1. [README](../../README.md)
2. [Architecture overview](../architecture/overview.md)
3. [Runtime execution path](../architecture/runtime-execution-path.md)
4. the subsystem page for the owning crate
5. [Debugging guide](debugging.md)
6. [Testing matrix](testing-matrix.md)

## Mental Model

Before changing code, answer these two questions:

1. Which crate owns the behavior?
2. Is the change source-of-truth logic or a derived/read-model concern?

```mermaid
flowchart TD
    start["Before changing code"] --> owner{"Which crate owns the behavior?"}
    owner -->|"Manifest or discovery"| workspace["codestory-workspace"]
    owner -->|"Parse, extract, or resolution"| indexer["codestory-indexer"]
    owner -->|"SQLite, snapshots, trails, bookmarks, or search docs"| store["codestory-store"]
    owner -->|"Search, grounding, orchestration, or agent flows"| runtime["codestory-runtime"]
    owner -->|"Args or output rendering"| cli["codestory-cli"]
    owner -->|"Shared DTOs, graph, or events"| contracts["codestory-contracts"]
    start --> truth{"Source of truth or derived read model?"}
    truth -->|"Source of truth"| first["Change the owning crate first"]
    truth -->|"Derived or read model"| follow["Verify store and runtime boundaries before patching projections"]
```

Use this mapping:

- manifest or discovery issue: `codestory-workspace`
- parse, extract, or resolution issue: `codestory-indexer`
- SQLite, snapshots, trails, bookmarks, or search docs: `codestory-store`
- search ranking, grounding, orchestration, or agent flows: `codestory-runtime`
- args or output rendering: `codestory-cli`
- shared DTOs or graph/event types: `codestory-contracts`

## Before Large Changes

Read these pages first:

- `docs/architecture/overview.md`
- `docs/architecture/runtime-execution-path.md`
- the subsystem page for the owning crate
- `docs/contributors/debugging.md`
- `docs/contributors/testing-matrix.md`
