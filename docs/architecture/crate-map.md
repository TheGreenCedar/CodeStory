# Crate Map

## Final Workspace Crates

- `codestory-contracts`
  Purpose: shared graph model, DTOs, grounding/trail types, and shared event contracts.
  Start in: `src/graph.rs`, `src/api.rs`, `src/events.rs`.

- `codestory-workspace`
  Purpose: repo discovery, manifest handling, and refresh-plan computation.
  Start in: `src/lib.rs`.

- `codestory-store`
  Purpose: SQLite open/build lifecycle, graph persistence, projection flushing, snapshots, trails, bookmarks, and search-doc persistence.
  Start in: `src/lib.rs`, `src/storage_impl/mod.rs`, `src/snapshot_store.rs`.

- `codestory-indexer`
  Purpose: language registry, parser/extractor flow, batch construction, resolution, and indexing regression suites.
  Start in: `src/lib.rs`, `src/resolution/`, `src/semantic/`.

- `codestory-runtime`
  Purpose: project open/index/search/ground/trail/agent orchestration and runtime-owned search engine internals.
  Start in: `src/lib.rs`, `src/services.rs`, `src/search/`, `src/grounding.rs`.

- `codestory-cli`
  Purpose: command parsing, runtime invocation, and output rendering.
  Start in: `src/main.rs`, `src/args.rs`, `src/output.rs`.

- `codestory-bench`
  Purpose: criterion benches for indexing, grounding, graph fidelity, resolution scope, and incremental cleanup.
  Start in: `benches/`.

## Typical Entry By Problem

- Refresh plan or manifest bug: `codestory-workspace`
- Missing symbol, bad edge, bad resolution: `codestory-indexer`
- Snapshot, bookmark, trail, or search-doc persistence issue: `codestory-store`
- Search ranking, grounding assembly, or orchestration issue: `codestory-runtime`
- Argument parsing or output shape issue: `codestory-cli`
