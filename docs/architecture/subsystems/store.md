# Store Subsystem

`codestory-store` is the only persistence crate.

## Ownership

- SQLite open/build lifecycle
- schema setup and migrations
- graph rows, file rows, occurrence rows, and projection state
- search-doc persistence
- bookmarks and trail queries
- summary/detail grounding snapshots and staged publish lifecycle

## Entry Points

- `crates/codestory-store/src/lib.rs`
- `crates/codestory-store/src/storage_impl/mod.rs`
- `crates/codestory-store/src/storage_impl/trail.rs`
- `crates/codestory-store/src/snapshot_store.rs`
- `crates/codestory-store/src/file_store.rs`

`GraphStore`, `TrailStore`, and `SearchDocStore` wrapper facades were removed;
callers should use the direct `Store` methods or the surviving focused stores
listed above.

## Extension Points

- add new read/write surfaces as focused sub-stores only when they own behavior
  beyond forwarding to `Store`
- keep snapshot lifecycle changes inside `snapshot_store.rs`
- keep SQL-heavy persistence logic inside `storage_impl/`

## Failure Signatures

- callers try to reach a raw storage object instead of `Store`
- snapshot promotion or invalidation is reimplemented outside the store
- runtime or CLI starts owning SQL or SQLite file management
