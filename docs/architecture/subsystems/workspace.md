# Workspace Subsystem

`codestory-workspace` owns repository identity, source discovery, refresh
planning, and filesystem safety primitives shared by publication layers.

## Identity and discovery

Project, workspace, and artifact scopes remain distinct. Repository identity v3
uses the strongest available repository/native filesystem identity and applies
platform lexical comparison only to missing paths: case-sensitive on Unix,
Windows-native case and verbatim-path rules on Windows.

`workspace_path_identity` exposes that same native rule as a fallible hash key
for bounded operation-local maps. Callers must treat an unavailable identity as
incomplete evidence and must not retain the key across file replacement.

`resolve_project_relative_path` applies the same native identity and containment
rules to exact-target probes. Existing symlinks are checked after filesystem
resolution, while missing paths use only operation-scoped native lexical
identity. It never turns an outside target into a project-relative path.

`codestory_project.json` defines source groups. An optional
`codestory_workspace.json` can name monorepo members; without either file the
crate creates a synthetic single-root manifest.

Discovery returns an explicit complete, partial, unreadable, or bounded
inventory with traversal failures. Only a complete inventory can prove absence
and schedule deletion. `workspace_relative_path` is the shared boundary for
mapping existing candidates into a project without cross-root or
case-folding mistakes.

The shared bounded-source policy classifies stable, content-hashed bytes above
the parser cap before indexer scheduling. Structural collectors may add a
candidate when verified content stays below that cap but exceeds the versioned
unit bound. Classification is based on observed content and policy rather than
one repository path. Partial discovery or a file that changes while being
hashed cannot produce a deletion-capable exclusion set.

## Refresh planning

Refresh plans compare discovered files with stored inventory using metadata and
verified source hashes where available. They identify new, changed, retained,
removable, and verified policy-excluded files without depending on a live store
handle.

A matching modification time never authorises reuse on its own: the content
hash is the verification, because same-mtime drift is a defended invariant.
`source_freshness.rs` therefore caches the *verdict*, not the metadata. A
caller may arm a `SourceFreshnessScope` around one operation; inside it, a
stored file's verdict is keyed by path, observed mtime, observed byte length,
and the stored content hash it was compared against, so re-indexing or any
metadata movement produces a new key. Only a torn-read-clean hash whose
observed metadata still agrees with the key may be recorded, and the memo is
inert with no scope armed. `source_freshness_counts` reports the content
hashes, verdict reuses, and strict-readiness fingerprint passes one scope paid
for.

A memoized verdict describes one instant, so it may only answer derivations
asking about that instant. Any check whose job is to detect drift that happened
*since* an earlier derivation calls `reverify_source_freshness_from_content()`
first, which drops the recorded verdicts and forces the next derivation to hash
content again. The runtime's post-build "source inputs changed while running
{operation}" refusal is exactly such a check, and it is the only mechanism that
sees a mutation preserving both mtime and byte length.

## Filesystem safety

- `atomic_file.rs` owns durable temporary-write and rename publication helpers.
- `owned_deletion.rs` owns handle-relative deletion below a trusted root,
  rejecting symlink/reparse traversal and ancestor-swap escapes.

Retrieval retention and core-generation pruning use these primitives rather
than validating a pathname and later recursing through it.

## Entry points

- `src/lib.rs`: manifests, inventories, relative paths, and refresh plans
- `src/source_freshness.rs`: operation-scoped freshness verdict memo and its
  pass counters
- `src/repository_identity.rs`: repository/project/workspace identity
- `src/atomic_file.rs`: atomic file publication
- `src/owned_deletion.rs`: trusted-root deletion

## Failure signatures

- a path spelling or global active directory replaces repository identity;
- an incomplete inventory schedules deletion;
- workspace depends on store or runtime;
- cleanup follows a pathname after its ancestors can be swapped.
