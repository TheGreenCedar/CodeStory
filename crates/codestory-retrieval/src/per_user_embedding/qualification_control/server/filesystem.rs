use anyhow::{Context, Result, bail};
use codestory_workspace::{
    WorkspacePathIdentity, workspace_file_identity, workspace_path_identity,
};
use std::fs::{self, File};
use std::path::Path;

pub(in crate::per_user_embedding) type NativeFileIdentity = WorkspacePathIdentity;

pub(super) fn validate_private_qualification_directory_metadata(
    metadata: &fs::Metadata,
) -> Result<()> {
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        bail!("embedding_qualification_directory_untrusted");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o077 != 0 {
            bail!("embedding_qualification_directory_untrusted");
        }
    }
    Ok(())
}

pub(in crate::per_user_embedding) fn validate_private_qualification_file_metadata(
    metadata: &fs::Metadata,
    maximum_bytes: u64,
) -> Result<()> {
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() > maximum_bytes {
        bail!("embedding_qualification_file_untrusted");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o077 != 0
            || metadata.nlink() != 1
        {
            bail!("embedding_qualification_file_untrusted");
        }
    }
    Ok(())
}

pub(super) fn native_path_identity(path: &Path) -> Result<NativeFileIdentity> {
    workspace_path_identity(path)
        .context("embedding qualification filesystem path identity is unavailable")
}

pub(super) fn native_file_identity(file: &File) -> Result<NativeFileIdentity> {
    workspace_file_identity(file)
        .context("embedding qualification filesystem file identity is unavailable")
}

#[cfg(unix)]
pub(in crate::per_user_embedding) fn sync_qualification_directory(path: &Path) -> Result<()> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .context("sync embedding qualification directory")
}

#[cfg(not(unix))]
pub(in crate::per_user_embedding) fn sync_qualification_directory(_path: &Path) -> Result<()> {
    Ok(())
}
