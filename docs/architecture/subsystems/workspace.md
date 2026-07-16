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

`codestory_project.json` defines source groups. An optional
`codestory_workspace.json` can name monorepo members; without either file the
crate creates a synthetic single-root manifest.

Discovery returns an explicit complete, partial, unreadable, or bounded
inventory with traversal failures. Only a complete inventory can prove absence
and schedule deletion. `workspace_relative_path` is the shared boundary for
mapping existing candidates into a project without cross-root or
case-folding mistakes.

## Refresh planning

Refresh plans compare discovered files with stored inventory using metadata and
verified source hashes where available. They identify new, changed, retained,
and removable files without depending on a live store handle.

## Filesystem safety

- `atomic_file.rs` owns durable temporary-write and rename publication helpers.
- `owned_deletion.rs` owns handle-relative deletion below a trusted root,
  rejecting symlink/reparse traversal and ancestor-swap escapes.

Retrieval retention and core-generation pruning use these primitives rather
than validating a pathname and later recursing through it.

## Entry points

- `src/lib.rs`: manifests, inventories, relative paths, and refresh plans
- `src/repository_identity.rs`: repository/project/workspace identity
- `src/atomic_file.rs`: atomic file publication
- `src/owned_deletion.rs`: trusted-root deletion

## Failure signatures

- a path spelling or global active directory replaces repository identity;
- an incomplete inventory schedules deletion;
- workspace depends on store or runtime;
- cleanup follows a pathname after its ancestors can be swapped.
