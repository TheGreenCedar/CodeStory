# Workspace Subsystem

`codestory-workspace` owns repo discovery and refresh planning.

## Ownership

- manifest loading and default manifest creation
- optional `codestory_workspace.json` member loading for monorepo sessions
- source-group and ignore configuration
- file discovery under the workspace root
- refresh inputs and refresh-plan computation

## Entry Points

- `crates/codestory-workspace/src/lib.rs`
- `WorkspaceManifest::open`
- `WorkspaceManifest::source_files`
- `WorkspaceManifest::full_refresh_execution_plan`
- `WorkspaceManifest::build_execution_plan`

## Manifests

When `codestory_project.json` exists, the workspace crate uses its explicit source groups. When neither config file exists, it creates a synthetic single-root manifest and keeps language filtering broad enough for mixed-language repos.

When `codestory_workspace.json` exists at the selected project root, it is treated as a lightweight monorepo manifest:

```json
{
  "members": ["backend/", "frontend/", "shared/"]
}
```

Each member becomes a synthetic source group rooted at that path. Discovery still applies the default generated-output exclusions such as `target`, `node_modules`, `.git`, `dist`, and `build`, and index output reports per-member counts through the runtime summary.

## Extension Points

- add new manifest options in the workspace crate
- add new discovery or exclusion rules here
- keep store-backed inventory as plain input data, not a direct dependency

## Failure Signatures

- workspace begins depending on store or runtime crates
- refresh planning requires live SQLite handles instead of inventory inputs
- discovery returns paths that are not stable for downstream indexing
