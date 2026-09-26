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
- sealed call-resolution facts, with source hash, parser fingerprint, and
  canonical dependency hashes stored once per distinct file-provenance group;
- verified source-policy exclusion rows and their project/workspace/core-bound
  count-and-digest manifest;
- versioned structural text units, per-file complete projections, their
  dedicated artifact cache, and a core-generation-bound publication manifest;
- core `generation_id`/`run_id` and retrieval-manifest records;
- schema migrations and a versioned promotion journal.

## Publication and reads

Full refresh builds and validates a staged database. Publication seals and
installs the candidate as an immutable generation, then atomically replaces
the current/rollback pointer. Pinned readers keep their selected generation.
Legacy promotion-journal recovery remains bounded by recorded candidate and
backup identities; backup existence alone never authorizes rollback.

The fresh full-refresh stage is explicitly disposable until publication. It
keeps WAL so a bounded artifact-cache reader can be opened when verified
structural rows were copied forward, uses relaxed synchronous writes with a
bounded nonzero checkpoint window, limits retained WAL allocation to that same
window when SQLite can safely reset the journal, and is never served or resumed.
Active WAL growth may exceed the retention limit while a transaction or pinned
reader needs its frames. Parser rows are not copied, and a stage with no copied
structural rows opens no cache reader. Its consuming publish path restores
NORMAL synchronization, completes a TRUNCATE checkpoint, syncs the standalone
database and directory, and permits no later stage writes before immutable
generation publication. Live stores, generic build callers, and staged incremental
clones remain WAL/NORMAL with their default journal retention.

Incremental refresh stages a sealed, immutable core image with a native file
clone where available, or a cancellable chunked byte copy. The source stays
under a generation reader lease for the entire stage. The stage is create-new,
synced before use, and removed on failure only when its native file identity
still matches the file this operation created. A source with SQLite WAL/SHM
sidecars cannot use this path; mutable legacy databases use a coherent SQLite
online backup instead. Full builds, incremental stages, and one-time legacy
backups check available cache-volume space against the source's logical SQLite
bytes plus a 64 MiB reserve before writing a full-size image. A refusal leaves
the previous publication in place. The completed replacement is installed as
an immutable generation and selected by the same atomic pointer. Readers that need publication coherence
use store read snapshots and compare the recorded generation/run identity;
retrieval owns the session that combines that transaction with immutable
generation leases before returning evidence.

Each newly installed immutable core image has a writer-provisioned lease.
Readers pin the selected image across their Store lifetime; a short
acquisition lock closes the pointer-to-lease race without holding every old
reader behind cleanup. Successful publication schedules best-effort core
retention, and explicit retrieval GC can retry it. The runtime coordinates
retrieval publication, while the store protects active and rollback core
identities plus every current or rollback retrieval binding. A pass completes
its root and generation-directory scan before deleting anything, then removes
at most 16 authenticated, unpinned images. Discovery is O(directory entries),
not constant time. Unknown neighbors, old images without a provisioned lease,
and SQLite images whose WAL, journal, or sidecar state cannot be observed
safely are retained. Unix may retain an empty generation directory after its
image is removed because the final directory pathname cannot be removed with
the same handle-bound identity guarantee. Its final file unlink is also
name-based after a native identity recheck; the acquisition and publication
fences exclude CodeStory writers in that interval, while an external actor
replacing the name at the last syscall remains outside that guarantee.

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

Schema v33 normalizes proof provenance without changing the typed fact or its
seal. Each fact retains its callsite file and an internal reference to a unique
`(file, source hash, parser fingerprint, canonical dependency list)` group;
reads reconstruct the original `CallResolutionFact` before validating its fact
and publication digests. Dependency sequences remain part of those sealed bytes:
Bash, Ruby, PHP, C#, Swift and Dart retain unique source-first encounter order;
other adapters use ascending file IDs. The shared `ProofDependencyOrder` contract
keeps Store shape checks and checked/compact consumers consistent. Store still
authenticates the complete language-specific source/evidence dependency sequence
and hashes; an order check alone does not authorize a fact. Multiple groups for
one file remain valid. Missing,
orphaned, extra, or cross-file provenance references fail closed. The v32 row
rewrite, its row-count/publication checks, and the schema-33 writer barrier
commit atomically, and migration never creates a proof publication receipt.

Schema v34 replaces the full canonical-ID index with a 32-byte binary suffix
expression index. Canonical strings and node IDs remain unchanged. The suffix
selects a candidate bucket only; exact canonical-string equality still
authorizes every result, including multiple nodes with the same canonical
string and suffix collisions. Live migration replaces the indexes and advances
the schema version in one transaction, while staged builds use the existing
deferred-index fence.

The projection transaction also replaces file-scoped errors and marks
grounding summary/detail plus resolution-support state dirty. Those writes do
not follow the graph commit as independent autocommits. Store telemetry counts
logical row attempts, prepared-statement executions, and estimated raw bind
payload bytes by family; the byte count describes input shape, not database,
WAL, or physical-write bytes.

Legacy promotion journals record candidate and rollback structural identities.
Their recovery validates the recorded manifest and current row digest before
accepting a database. Current publication installs a sealed immutable generation
and atomically replaces its pointer after validation. Missing, legacy, or
corrupt structural publication state cannot become the current core generation.

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

Selective anchor lookup errors are not evidence that a symbol is absent. A
rebind resolves every bookmark before writing and commits the outcomes in one
sidecar transaction; a failed query or write leaves the prior evidence intact.
That persisted generation is the retry checkpoint: the next core writer catches
it up against the still-current complete generation before publishing another.
The adjacent-generation rule for changed anchors remains unchanged.

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

**Downgrade path.** There is no bookmark export/import command. Recovery after
a newer schema is `cache reset --derived-only` from a 0.17 binary, then
`index --refresh full`. Reset includes the immutable pointer, generations,
stages and retrieval-publication database without opening them. The canonical
artifact registry preserves annotations and their retained migration export,
and leaves writer, promotion and acquisition coordination inodes at their live
paths. Its exclusion order is the global retrieval fence, index writer,
promotion, acquisition, then all named generation leases. The complete locked
enumeration must prove every generation idle before any quarantine move; an
unknown entry, enumeration error, absent lease or held reader fails closed.
The plan is refreshed under those exclusions. Older unprovisioned immutable
layouts require a compatible cache backup or manual recovery after all clients
are stopped; reset cannot prove their readers idle.
Internal controller export/import types are test
helpers, not an operator workflow.

## Entry points

- `src/storage_impl/mod.rs`: schema lifecycle, reads/writes, publication journal,
  recovery, and staged promotion
- `src/annotations/mod.rs`: the versioned annotation sidecar, its journaled
  cutover, and the native-root location registry
- `src/annotations/resolution.rs`: the conservative rebind ladder
- `src/snapshot_store.rs`: staged and live grounding snapshots
- `src/sealed_file_stage.rs`: shared sealed-file native clone or cancellable copy
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
