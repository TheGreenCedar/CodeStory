# Store Subsystem

`codestory-store` is the only SQLite persistence layer. It owns durable core
publication and read consistency; callers own neither raw SQL nor database-file
recovery.

## Durable state

- file, node, edge, occurrence, component, callable, bookmark, and trail rows;
- grounding snapshots and search projections;
- graph-native symbol documents, component reports, and reusable dense anchors;
- core `generation_id`/`run_id` and retrieval-manifest records;
- schema migrations and a versioned promotion journal.

## Publication and reads

Full refresh builds and validates a staged database. Promotion durably records a
prepared journal with previous and candidate identities, installs and validates
the candidate, records committed, then performs best-effort cleanup. Recovery
may restore only a valid recorded prepared backup; a committed publication is
never rolled back merely because a backup remains.

Incremental refresh updates the live database and refreshes derived snapshots
in place. Readers that need publication coherence use store read snapshots and
compare the recorded generation/run identity; retrieval runtime combines those
reads with generation leases before returning evidence.

## Entry points

- `src/storage_impl/mod.rs`: schema lifecycle, reads/writes, publication journal,
  recovery, and staged promotion
- `src/snapshot_store.rs`: staged and live grounding snapshots
- `src/file_store.rs`: focused file persistence
- `src/storage_impl/trail.rs`: trail queries

## Extension rules

- add SQL and recovery behavior here, with fault coverage at the durable fence;
- expose typed store methods rather than raw connections;
- keep retrieval artifact files in `codestory-retrieval` and product
  orchestration in runtime.

## Failure signatures

- backup existence alone authorizes rollback;
- callers reopen current storage during a pinned publication read;
- runtime or CLI manages SQLite files or writes SQL;
- a partial promotion can be reported as successful.
