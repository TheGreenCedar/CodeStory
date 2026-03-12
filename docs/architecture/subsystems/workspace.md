# Workspace Subsystem

`codestory-workspace` owns repo discovery and refresh planning.

## Ownership

- manifest loading and default manifest creation
- source-group and ignore configuration
- file discovery under the workspace root
- refresh inputs and refresh-plan computation

## Entry Points

- `crates/codestory-workspace/src/lib.rs`
- `WorkspaceManifest::open`
- `WorkspaceManifest::source_files`
- `WorkspaceManifest::full_refresh_execution_plan`
- `WorkspaceManifest::build_execution_plan`

## Extension Points

- add new manifest options in the workspace crate
- add new discovery or exclusion rules here
- keep store-backed inventory as plain input data, not a direct dependency

## Failure Signatures

- workspace begins depending on store or runtime crates
- refresh planning requires live SQLite handles instead of inventory inputs
- discovery returns paths that are not stable for downstream indexing
