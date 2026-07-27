use anyhow::{Context, Result, bail};
use codestory_workspace::{
    WorkspacePathIdentity, workspace_file_identity, workspace_path_identity,
};
use std::fs::{self, File};
use std::io;
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

pub(in crate::per_user_embedding) fn native_path_identity(
    path: &Path,
) -> Result<NativeFileIdentity> {
    workspace_path_identity(path)
        .context("embedding qualification filesystem path identity is unavailable")
}

/// Why one poll tick found no command to consume at the pinned path.
///
/// The two classes carry different certainty, and the caller bounds them
/// differently: `Removed` is proof that the writer took its file back, while
/// `Denied` is an observation that is *consistent with* a removal in progress
/// but cannot rule out a file that is simply unreadable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::per_user_embedding) enum CommandAbsence {
    /// The writer took its file back. A Unix unlink answers `NotFound`, a name
    /// that lost its last link mid-consume reports a zero link count, and a
    /// Windows host with POSIX delete semantics -- which is what the proof host
    /// measured, Windows 11 build 26200 on NTFS -- also answers `NotFound`.
    Removed,
    /// Windows answered `ACCESS_DENIED`. A file left delete-pending answers that
    /// way, and so does a present file whose ACL denies the open; one
    /// observation cannot separate them, which is why this class is bounded
    /// rather than tolerated indefinitely.
    ///
    /// A compatibility guard, not the measured behaviour of the current proof
    /// host: `DeleteFileW` there unlinks the name at once. It still matters
    /// where POSIX delete is unavailable -- older Windows, FAT/exFAT, network
    /// filesystems, or a memory-mapped file -- because that is where a routine
    /// command cleanup can still leave a delete-pending entry behind.
    Denied,
}

/// One observation of the pinned command path: what was there, or why nothing
/// was. `Err` is not a failure -- it is the attributable reason for an absence.
pub(super) type Observation<T> = std::result::Result<T, CommandAbsence>;

/// Whether one filesystem error means there is nothing to consume this tick,
/// rather than that the qualification control plane is broken.
///
/// Deliberately narrow. Every other error class stays fatal so the accept loop
/// keeps failing closed on a control plane that is genuinely broken. In
/// particular `ERROR_SHARING_VIOLATION` does not map to `PermissionDenied` and
/// so stays fatal.
pub(super) fn qualification_file_absence(error: &io::Error) -> Option<CommandAbsence> {
    classify_absence(error.kind(), cfg!(windows))
}

/// The platform argument is explicit so both answers stay testable from either
/// host, rather than only on whichever one happens to run the suite.
fn classify_absence(kind: io::ErrorKind, windows: bool) -> Option<CommandAbsence> {
    match kind {
        io::ErrorKind::NotFound => Some(CommandAbsence::Removed),
        io::ErrorKind::PermissionDenied if windows => Some(CommandAbsence::Denied),
        _ => None,
    }
}

/// Whether one observation shows a file that has lost its last name.
///
/// Unix drops an unlinked file's link count to zero while it stays fully
/// readable through any open handle, and a `stat` that resolves the name just
/// as it is being removed sees the same zero. The shared private-metadata gate
/// rejects that as an untrusted link count, which is the wrong verdict for a
/// command the writer took back: the file is not untrusted, it is gone.
///
/// This never relaxes the guard that gate exists for. A hard-linked command is
/// `nlink >= 2` and still fails closed; only the impossible-to-attack `nlink ==
/// 0` is folded into the vanish class, and only for the command file.
#[cfg(unix)]
pub(super) fn qualification_file_lost_its_last_name(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    metadata.nlink() == 0
}

#[cfg(not(unix))]
pub(super) fn qualification_file_lost_its_last_name(_metadata: &fs::Metadata) -> bool {
    false
}

/// Native path identity that reports an absence instead of failing.
///
/// Only the [`qualification_file_absence`] classes become an absence. A missing
/// Unix path yields a lexical identity rather than an error, so callers still
/// confirm presence before treating an identity mismatch as a substitution.
pub(super) fn optional_native_path_identity(
    path: &Path,
) -> Result<Observation<NativeFileIdentity>> {
    match workspace_path_identity(path) {
        Ok(identity) => Ok(Ok(identity)),
        Err(error) => match qualification_file_absence(&error) {
            Some(absence) => Ok(Err(absence)),
            None => Err(error)
                .context("embedding qualification filesystem path identity is unavailable"),
        },
    }
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

#[cfg(test)]
mod tests {
    use super::{CommandAbsence, classify_absence};
    use std::io;

    /// The absence classes must stay exactly as narrow as they are documented,
    /// on both platforms, from whichever host runs the suite.
    #[test]
    fn only_removal_and_windows_denial_read_as_an_absent_command() {
        for windows in [false, true] {
            assert_eq!(
                classify_absence(io::ErrorKind::NotFound, windows),
                Some(CommandAbsence::Removed)
            );
            for fatal in [
                io::ErrorKind::InvalidData,
                io::ErrorKind::Interrupted,
                io::ErrorKind::IsADirectory,
                io::ErrorKind::Other,
                // ERROR_SHARING_VIOLATION does not map to PermissionDenied, so
                // a genuinely locked command file stays fatal on Windows too.
                io::ErrorKind::ResourceBusy,
            ] {
                assert_eq!(classify_absence(fatal, windows), None, "{fatal:?}");
            }
        }
        assert_eq!(
            classify_absence(io::ErrorKind::PermissionDenied, false),
            None
        );
        assert_eq!(
            classify_absence(io::ErrorKind::PermissionDenied, true),
            Some(CommandAbsence::Denied)
        );
    }
}
