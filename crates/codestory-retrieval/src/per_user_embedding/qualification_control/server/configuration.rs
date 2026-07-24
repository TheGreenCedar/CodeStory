use super::super::super::hex_sha256;
use super::ServerQualificationControl;
use super::event_log::ServerQualificationEventLog;
use super::filesystem::{
    NativeFileIdentity, native_file_identity, native_path_identity,
    validate_private_qualification_directory_metadata,
};
use anyhow::{Context, Result, bail};
use std::fs;
#[cfg(unix)]
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64};

#[derive(Debug)]
pub(in crate::per_user_embedding) struct PinnedQualificationDirectory {
    pub(in crate::per_user_embedding) path: PathBuf,
    identity: NativeFileIdentity,
    #[cfg(unix)]
    handle: File,
}

pub(in crate::per_user_embedding) fn server_qualification_control_from_env()
-> Result<Option<ServerQualificationControl>> {
    server_qualification_control_from_values(
        std::env::var_os("CODESTORY_EMBED_QUALIFICATION_DIR"),
        std::env::var("CODESTORY_EMBED_QUALIFICATION_NONCE").ok(),
    )
}

pub(in crate::per_user_embedding) fn server_qualification_control_from_values(
    directory: Option<std::ffi::OsString>,
    nonce: Option<String>,
) -> Result<Option<ServerQualificationControl>> {
    match (directory, nonce) {
        (None, None) => Ok(None),
        (Some(directory), Some(nonce))
            if !directory.is_empty()
                && !nonce.is_empty()
                && nonce.len() <= 128
                && nonce
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')) =>
        {
            let directory = PinnedQualificationDirectory::open(&PathBuf::from(directory))?;
            let events = ServerQualificationEventLog::open(&directory, &nonce)?;
            let last_sequence = events.last_sequence;
            Ok(Some(ServerQualificationControl {
                directory,
                events: Mutex::new(events),
                nonce_sha256: hex_sha256(nonce.as_bytes()),
                nonce,
                last_sequence: AtomicU64::new(last_sequence),
                processed_command_sha256: Mutex::new(None),
                force_incompatible: AtomicBool::new(false),
                freeze_owner: AtomicBool::new(false),
            }))
        }
        _ => bail!("embedding_qualification_gate_incomplete"),
    }
}

impl PinnedQualificationDirectory {
    fn open(path: &Path) -> Result<Self> {
        if !path.is_absolute() {
            bail!("embedding_qualification_directory_not_absolute");
        }
        let source = fs::symlink_metadata(path).with_context(|| {
            format!(
                "inspect embedding qualification directory {}",
                path.display()
            )
        })?;
        validate_private_qualification_directory_metadata(&source)?;
        let canonical = fs::canonicalize(path).with_context(|| {
            format!(
                "canonicalize embedding qualification directory {}",
                path.display()
            )
        })?;
        #[cfg(not(windows))]
        if canonical != path {
            bail!("embedding_qualification_directory_untrusted");
        }
        let metadata = fs::symlink_metadata(&canonical)
            .context("reinspect canonical embedding qualification directory")?;
        validate_private_qualification_directory_metadata(&metadata)?;
        let identity = native_path_identity(&canonical)?;
        #[cfg(windows)]
        {
            let caller = fs::symlink_metadata(path)
                .context("reinspect caller embedding qualification directory")?;
            validate_private_qualification_directory_metadata(&caller)?;
            if native_path_identity(path)? != identity {
                bail!("embedding_qualification_directory_untrusted");
            }
        }
        #[cfg(unix)]
        let handle = {
            use std::os::unix::fs::OpenOptionsExt;
            let handle = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW)
                .open(&canonical)
                .context("pin embedding qualification directory")?;
            let opened = handle
                .metadata()
                .context("inspect pinned embedding qualification directory")?;
            validate_private_qualification_directory_metadata(&opened)?;
            if native_file_identity(&handle)? != identity {
                bail!("embedding_qualification_directory_replaced");
            }
            handle
        };
        let pinned = Self {
            path: canonical,
            identity,
            #[cfg(unix)]
            handle,
        };
        pinned.revalidate()?;
        Ok(pinned)
    }

    pub(in crate::per_user_embedding) fn revalidate(&self) -> Result<()> {
        let metadata = fs::symlink_metadata(&self.path)
            .context("revalidate embedding qualification directory")?;
        validate_private_qualification_directory_metadata(&metadata)?;
        if native_path_identity(&self.path)? != self.identity {
            bail!("embedding_qualification_directory_replaced");
        }
        #[cfg(unix)]
        {
            let opened = self
                .handle
                .metadata()
                .context("revalidate pinned embedding qualification directory")?;
            validate_private_qualification_directory_metadata(&opened)?;
            if native_file_identity(&self.handle)? != self.identity {
                bail!("embedding_qualification_directory_replaced");
            }
        }
        Ok(())
    }

    pub(in crate::per_user_embedding) fn join(&self, name: impl AsRef<Path>) -> PathBuf {
        self.path.join(name)
    }
}
