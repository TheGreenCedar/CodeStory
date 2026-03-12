# Architecture Invariants

- `codestory-contracts` is the only shared-model source for final V2 crates.
- `codestory-workspace` does not depend on `store`, `indexer`, `runtime`, or CLI crates.
- `codestory-store` owns SQLite open/build lifecycle and exposes no raw storage escape hatch.
- `codestory-indexer` does not depend on `runtime` or CLI crates.
- `codestory-runtime` is the only orchestration layer and owns the search engine implementation.
- `codestory-cli` depends on `runtime` and `contracts`, not on `store` or `indexer`.
- Snapshot promotion, discard, and derived refresh behavior lives in `codestory-store` only.
- Public DTOs, events, graph primitives, grounding types, and trail types must be added in `codestory-contracts` first.
