use super::{Path, WorkspaceManifest};

pub(super) fn runtime_workspace_manifest(
    root: &Path,
    storage_path: &Path,
) -> anyhow::Result<WorkspaceManifest> {
    WorkspaceManifest::open_with_storage_owned_exclusions(root.to_path_buf(), storage_path)
}
