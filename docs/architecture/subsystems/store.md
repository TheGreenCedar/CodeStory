# Store Subsystem

`codestory-store` is the only SQLite persistence layer. It owns durable core
publication and read consistency; callers own neither raw SQL nor database-file
recovery.

## Durable state

- file, node, edge, occurrence, component, callable, and trail rows;
- retained legacy `bookmark_category`/`bookmark_node` tables, read-only for one
  release behind the schema-31 writer barrier; the annotations sidecar owns
  user annotations;
- grounding snapshots and canonical paged search-symbol reads from the node
  table; the legacy materialized search projection remains compatibility-only;
- graph-native symbol documents, component reports, reusable embedding-free dense-anchor inputs, and their complete publication manifest;
- verified source-policy exclusion rows and their project/workspace/core-bound
  count-and-digest manifest;
- versioned structural text units, per-file complete projections, their
  dedicated artifact cache, and a core-generation-bound publication manifest;
- core `generation_id`/`run_id` and retrieval-manifest records;
- schema migrations and a versioned promotion journal.

## Publication and reads

Full refresh builds and validates a staged database. Promotion durably records a
prepared journal with previous and candidate identities, installs and validates
the candidate, records committed, then performs best-effort cleanup. Recovery
may restore only a valid recorded prepared backup; a committed publication is
never rolled back merely because a backup remains.

The fresh full-refresh stage is explicitly disposable until publication. It
keeps WAL so a bounded artifact-cache reader can be opened when verified
structural rows were copied forward, uses relaxed synchronous writes with a
bounded nonzero checkpoint window, and is never served or resumed. Parser rows
are not copied, and a stage with no copied structural rows opens no cache
reader. Its consuming publish path restores NORMAL synchronization, completes a
TRUNCATE checkpoint, syncs the standalone database and directory, and permits
no later stage writes before entering the promotion journal. Live stores,
generic build callers, and staged incremental clones remain WAL/NORMAL.

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
Each row binds observed bytes and structural-unit count plus both active caps.
A unit-bound row has no file, graph, structural-text, typed-target, or semantic
projection.

The structural-unit manifest binds descriptor schema, migration state, complete
unit and projection counts and digests, and exact core generation/run. Each
structural file has a verified source hash and a count-and-digest projection
that carries its producer, including zero-unit files. Replacing one file's
hash, graph rows, units, projection, and dedicated cache entry is atomic and
invalidates the complete manifest until runtime republishes it. Schema migration
creates the tables but no synthetic completeness claim.

The projection transaction also replaces file-scoped errors and marks
grounding summary/detail plus resolution-support state dirty. Those writes do
not follow the graph commit as independent autocommits. Store telemetry counts
logical row attempts, prepared-statement executions, and estimated raw bind
payload bytes by family; the byte count describes input shape, not database,
WAL, or physical-write bytes.

Promotion journals record candidate and rollback structural identities.
Prepared install, committed recovery, and rollback validate the recorded
manifest and current row digest before accepting a database. Missing, legacy,
or corrupt structural publication state therefore cannot become the current
core generation.

Schema v25 also stores the current retrieval manifest and its deeply verified
rollback record in the same SQLite row. They change in one transaction. The
filesystem retention marker is derived after commit and can only make cleanup
more conservative; it is not a publication authority.

## Annotations sidecar

User annotations are not core state. They live in `annotations.sqlite3` beside
the core database, outside the promotion fence, with their own WAL connection,
busy timeout, foreign keys, and explicit schema-version row. Schema v1 owns
`bookmark_category(id, name UNIQUE)` and
`bookmark(uuid PK, category_id FK CASCADE, canonical_id, file_identity,
qualified_name, kind, normalized_signature, start_line, comment,
resolution_status, orphan_reason, last_known_evidence, created_at, updated_at)`,
plus an idempotent migration journal and the native-root location registry.

The sidecar is created and migrated only by an annotation write or by an
operation that can replace core projections. Project-open, status, and doctor
paths open it observationally and never materialize it. Before the cutover the
retained core `bookmark_category`/`bookmark_node` tables are the source of
truth; after it the sidecar is. There is no instant at which both are, and no
dual write.

Resolution is recomputed from the anchor on every read. Re-resolving an
unchanged anchor — an exact canonical id, or the exact
`(file_identity, qualified_name, kind)` tuple — is an identity lookup, so a
position-shifting edit or a rebuilt projection simply finds the symbol again.
Rebinding a *changed* anchor — a rename or a move — is an inference and requires
an adjacent core generation, agreeing normalized-signature evidence, and a
unique candidate. Ambiguity never guesses: the annotation becomes a visible,
user-owned orphan carrying `orphan_reason` and its last known evidence until an
explicit relink or delete.

The two inferences are looked up from opposite ends, because a rename and a move
change opposite halves of the anchor. A move keeps the qualified name and
changes the file, so it is found by name in another file and then checked
against the normalized signature; a unique candidate whose signature disagrees
is a visible `signature_changed` orphan. A rename keeps the file and changes the
name, so it is found by normalized signature within the same file and kind.

The normalized signature backing both is
`callable_projection_state.normalized_signature`, computed by the indexer from
the callable's kind, line extent, and body projection expressed relative to its
own start. It is deliberately not `signature_hash`, which is an
incremental-projection change detector over the symbol's own name and exact
start position and therefore changes on every rename and every move. The value
is tagged: `shape:` when the body projected at least one edge or occurrence,
`outline:` when it projected nothing and only the kind and line count remain. A
rename may only be inferred from a `shape:` signature, because it has no other
evidence and an outline is shared by every stub of the same length. A move
accepts either, because the qualified name has already identified the symbol and
the signature only has to agree.

Each bind also records how well its evidence separated the symbol at the time —
whether the signature matched exactly one symbol of that kind in the file, and
whether the qualified name matched exactly one symbol anywhere. An inference may
only rest on evidence that was discriminating when it was proven, so a surviving
same-shaped sibling never inherits a deleted symbol's annotation.

The migration is paired with the schema-31 core writer barrier. Forward-only
migration already refuses a newer schema, so a 0.16.3 CLI opening a migrated
database fails closed on the whole database instead of silently writing the
retained legacy tables and forking annotation truth.

Reads switch source of truth on the migration journal row, not on the sidecar
file existing. The cutover creates and binds the sidecar before it imports, so
gating on the file would report zero annotations for the whole window between
those two steps — and permanently, if the import never completed. Annotations
imported from the retained tables take a uuid derived from their legacy row id,
so an id a pre-cutover read already handed out still addresses the same
annotation afterwards.

**Downgrade path.** Export annotations first
(`AppController::export_annotations`, written to the retained
`annotations.pre-migration.json` shape), then run EV-9's guided derived-cache
reset to drop the schema-31 core. The exported file is re-importable with
`AppController::import_annotations`. The same export/import pair is the only
supported way to move annotations onto a clone or a cross-volume copy: the
native-root location registry binds a same-filesystem move by filesystem
identity and fails closed on any other root.

## Entry points

- `src/storage_impl/mod.rs`: schema lifecycle, reads/writes, publication journal,
  recovery, and staged promotion
- `src/annotations/mod.rs`: the versioned annotation sidecar, its journaled
  cutover, and the native-root location registry
- `src/annotations/resolution.rs`: the conservative rebind ladder
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
