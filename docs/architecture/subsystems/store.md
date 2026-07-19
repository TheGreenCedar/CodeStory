# Store Subsystem

`codestory-store` is the only SQLite persistence layer. It owns durable core
publication and read consistency; callers own neither raw SQL nor database-file
recovery.

## Durable state

- file, node, edge, occurrence, component, callable, bookmark, and trail rows;
- grounding snapshots and canonical paged search-symbol reads from the node
  table; the legacy materialized search projection remains compatibility-only;
- graph-native symbol documents, component reports, reusable embedding-free dense-anchor inputs, and their complete publication manifest;
- verified source-policy exclusion rows and their project/workspace/core-bound
  count-and-digest manifest;
- versioned structural text units, per-file complete projections, their
  dedicated artifact cache, and a project/workspace/core-bound publication
  manifest;
- core `generation_id`/`run_id` and retrieval-manifest records;
- schema migrations and a versioned promotion journal.

## Publication and reads

Full refresh builds and validates a staged database. Promotion durably records a
prepared journal with previous and candidate identities, installs and validates
the candidate, records committed, then performs best-effort cleanup. Recovery
may restore only a valid recorded prepared backup; a committed publication is
never rolled back merely because a backup remains.

The fresh full-refresh stage is explicitly disposable until publication. It
keeps WAL for the bounded artifact-cache reader, uses relaxed synchronous writes
with a bounded nonzero checkpoint window, and is never served or resumed. Its
consuming publish path restores NORMAL synchronization, completes a TRUNCATE
checkpoint, syncs the standalone database and directory, and permits no later
stage writes before entering the promotion journal. Live stores, generic build
callers, and staged incremental clones remain WAL/NORMAL.

Incremental refresh writes a durable clone and promotes the completed
replacement through the same journal. Readers that need publication coherence
use store read snapshots and compare the recorded generation/run identity;
retrieval owns the session that combines that transaction with immutable
generation leases before returning evidence.

The dense-anchor manifest is part of the core publication boundary. It binds
the complete row count and digest, policy version, migration state, and every
row's source identity to the current core generation/run. A migrated cache has
no complete manifest until core indexing republishes it.

The source-policy exclusion manifest follows the same fail-closed rule. Rows
and manifest replace together in one SQLite transaction, and staged promotion
records their candidate and rollback identities. A schema migration creates no
synthetic manifest; runtime must republish from a complete verified inventory.

The structural-unit manifest binds descriptor schema and producer version,
complete unit count and digest, project/workspace identity, and exact core
generation/run. Each structural file has a verified source hash and a
count-and-digest projection, including zero-unit files. Replacing one file's
hash, graph rows, units, projection, and dedicated cache entry is atomic and
invalidates the complete manifest until runtime republishes it. Schema
migration creates the tables but no synthetic completeness claim.

Promotion journals record candidate and rollback structural identities.
Prepared install, committed recovery, and rollback validate the recorded
manifest and current row digest before accepting a database. Missing, legacy,
or corrupt structural publication state therefore cannot become the current
core generation.

Schema v25 also stores the current retrieval manifest and its deeply verified
rollback record in the same SQLite row. They change in one transaction. The
filesystem retention marker is derived after commit and can only make cleanup
more conservative; it is not a publication authority.

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
- a structural cache row is copied through the generic parser cache or is
  published without matching source and projection identities;
- a partial promotion can be reported as successful.
