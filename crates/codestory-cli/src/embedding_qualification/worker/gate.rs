use anyhow::{Context, Result, bail};
use codestory_retrieval::{
    AwakeMonotonicClock, EmbeddingQualificationWorkerError as WorkerError, ProcessStartProbe,
    SidecarRuntimeConfig, embedding_retry_state,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

const QUALIFICATION_NONCE_ENV: &str = "CODESTORY_EMBED_QUALIFICATION_NONCE";
const MAX_REQUEST_BYTES: u64 = 1024 * 1024;
pub(super) const POLL: Duration = Duration::from_millis(25);

pub(super) fn read_private_request(path: &Path) -> Result<Vec<u8>> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("inspect qualification request {}", path.display()))?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() > MAX_REQUEST_BYTES
    {
        bail!("embedding_qualification_request_file_untrusted");
    }
    validate_private_file_metadata(&metadata)?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    File::open(path)
        .with_context(|| format!("open qualification request {}", path.display()))?
        .take(MAX_REQUEST_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_REQUEST_BYTES {
        bail!("embedding_qualification_request_too_large");
    }
    Ok(bytes)
}

pub(super) fn validate_direct_child(
    path: &Path,
    directory: &Path,
    must_exist: bool,
) -> Result<PathBuf> {
    let Some(parent) = path.parent() else {
        bail!("embedding_qualification_path_untrusted");
    };
    let Some(file_name) = path.file_name() else {
        bail!("embedding_qualification_path_untrusted");
    };
    if !path.is_absolute() || path.extension().and_then(|value| value.to_str()) != Some("json") {
        bail!("embedding_qualification_path_untrusted");
    }
    if canonical_existing(parent)? != directory {
        bail!("embedding_qualification_parent_replaced");
    }
    let canonical_path = directory.join(file_name);
    if must_exist && canonical_existing(path)? != canonical_path {
        bail!("embedding_qualification_path_untrusted");
    }
    Ok(canonical_path)
}

pub(super) fn required_absolute_directory(name: &str) -> Result<PathBuf> {
    let value = std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| anyhow::anyhow!("embedding_qualification_gate_closed"))?;
    if !value.is_absolute() {
        bail!("embedding_qualification_directory_not_absolute");
    }
    canonical_existing(&value)
}

pub(super) fn validate_private_directory(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("inspect qualification directory {}", path.display()))?;
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

pub(super) fn validate_private_file_metadata(metadata: &fs::Metadata) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o077 != 0 {
            bail!("embedding_qualification_request_file_untrusted");
        }
    }
    Ok(())
}

pub(super) fn canonical_existing(path: &Path) -> Result<PathBuf> {
    fs::canonicalize(path).with_context(|| format!("canonicalize {}", path.display()))
}

pub(super) fn write_atomic_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let parent = path
        .parent()
        .context("atomic qualification output has no parent")?;
    validate_private_directory(parent)?;
    if path.exists() {
        bail!("embedding_qualification_output_exists");
    }
    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
    let bytes = serde_json::to_vec_pretty(value).context("serialize qualification output")?;
    for _ in 0..32 {
        let sequence = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let temp = parent.join(format!(
            ".codestory-qualification-{}-{sequence}.tmp",
            std::process::id()
        ));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = match options.open(&temp) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error).context("create atomic qualification temp file"),
        };
        let result = (|| {
            file.write_all(&bytes)?;
            file.write_all(b"\n")?;
            file.sync_all()?;
            drop(file);
            fs::rename(&temp, path)?;
            sync_published_directory(parent)?;
            Ok::<_, std::io::Error>(())
        })();
        if let Err(error) = result {
            let _ = fs::remove_file(&temp);
            return Err(error).context("publish atomic qualification output");
        }
        return Ok(());
    }
    bail!("embedding_qualification_temp_name_exhausted")
}

/// Make an already-published name durable.
///
/// The rename is what makes the publication atomic; this step only decides
/// whether the new directory entry survives a crash, so it must never be able
/// to fail a publication that already happened. Unix fsyncs the parent
/// directory. Windows skips it, because `File::open` does not pass
/// `FILE_FLAG_BACKUP_SEMANTICS` and `CreateFileW` therefore refuses a directory
/// with `ERROR_ACCESS_DENIED` - that denial, not the rename, is what failed the
/// publish.
///
/// Opening the directory properly is not the fix. Windows documents no
/// directory-fsync contract, and the behaviour bears that out: measured on
/// windows 11 26200 / NTFS, with and without backup-class privileges,
/// `FlushFileBuffers` on a backup-semantics handle is denied when the handle is
/// `GENERIC_READ` and accepted when it is `GENERIC_WRITE`, so what it would
/// commit is an accident of access mode rather than a guarantee. Skipping the
/// step is what every other atomic publisher here already does
/// (`codestory_workspace::atomic_file`, `codestory_cli::native_launcher`,
/// `native_staging`, `codestory_store::sync_promotion_parent`,
/// `sync_qualification_directory`); these two copies did not. Windows rename
/// durability, if it is ever wanted, comes from
/// `MoveFileExW(.., MOVEFILE_WRITE_THROUGH)` - which `native_launcher` already
/// uses - not from a directory fsync.
#[cfg(not(windows))]
fn sync_published_directory(path: &Path) -> std::io::Result<()> {
    File::open(path)?.sync_all()
}

#[cfg(windows)]
fn sync_published_directory(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

pub(super) fn sha256_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub(super) fn qualification_nonce() -> Result<String> {
    std::env::var(QUALIFICATION_NONCE_ENV)
        .ok()
        .filter(|nonce| {
            !nonce.is_empty()
                && nonce.len() <= 128
                && nonce
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        })
        .ok_or_else(|| anyhow::anyhow!("embedding_qualification_gate_closed"))
}

pub(super) fn validate_worker_project(project: &Path) -> Result<()> {
    if !project.is_absolute() {
        bail!("embedding_qualification_project_not_absolute");
    }
    let metadata = fs::symlink_metadata(project)
        .with_context(|| format!("inspect qualification project {}", project.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        bail!("embedding_qualification_project_untrusted");
    }
    canonical_existing(project)?;
    Ok(())
}

pub(super) fn validate_gate_path(path: &Path, directory: &Path) -> Result<()> {
    if !path.is_absolute()
        || path.parent() != Some(directory)
        || path.extension().and_then(|extension| extension.to_str()) != Some("json")
    {
        bail!("embedding_qualification_start_gate_untrusted");
    }
    Ok(())
}

pub(super) fn wait_for_gate(
    clock: &dyn AwakeMonotonicClock,
    path: &Path,
    timeout: Duration,
) -> Result<()> {
    let started = clock.now_ns();
    loop {
        match fs::symlink_metadata(path) {
            Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
                return Ok(());
            }
            Ok(_) => bail!("embedding_qualification_start_gate_untrusted"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("inspect embedding qualification start gate"),
        }
        if elapsed(clock, started) >= timeout {
            bail!("embedding_qualification_start_gate_timeout");
        }
        clock.sleep(POLL);
    }
}

pub(super) fn current_process_start_identity() -> Result<String> {
    match codestory_retrieval::probe_process_start_identity(std::process::id()) {
        ProcessStartProbe::Running { start_identity } => Ok(start_identity),
        ProcessStartProbe::NotRunning => bail!("embedding_qualification_worker_not_running"),
        ProcessStartProbe::Unknown { reason } => {
            bail!("embedding_qualification_worker_identity_unknown:{reason}")
        }
    }
}

pub(super) fn worker_error(error: &anyhow::Error) -> WorkerError {
    if let Some(retry) = embedding_retry_state(error) {
        return WorkerError {
            code: retry.code,
            message_head: retry.message.chars().take(128).collect(),
            retry_class: retry.retry_class,
            retry_after_ms: retry.retry_after_ms,
            retry_condition: retry.retry_condition,
            capacity: retry.capacity,
        };
    }
    let message_head = error_head(error);
    WorkerError {
        code: message_head.clone(),
        message_head,
        retry_class: "terminal".into(),
        retry_after_ms: 0,
        retry_condition: "the qualification request is corrected".into(),
        capacity: None,
    }
}

pub(super) fn error_head(error: &anyhow::Error) -> String {
    error
        .to_string()
        .split([':', '\n'])
        .next()
        .unwrap_or("embedding_qualification_failed")
        .chars()
        .take(128)
        .collect()
}

pub(super) fn qualification_request_id(prefix: &str, now_ns: u64) -> String {
    format!("{prefix}-{}-{now_ns}", std::process::id())
}

pub(super) fn project_identity_sha256(runtime: &SidecarRuntimeConfig) -> String {
    let seed = runtime
        .project_identity
        .as_ref()
        .map(|identity| format!("{}:{}", identity.project_id, identity.workspace_id))
        .unwrap_or_else(|| runtime.namespace.clone());
    sha256_bytes(seed.as_bytes())
}

pub(super) fn elapsed(clock: &dyn AwakeMonotonicClock, started_ns: u64) -> Duration {
    Duration::from_nanos(clock.now_ns().saturating_sub(started_ns))
}

#[cfg(test)]
mod tests {
    use super::write_atomic_json;
    use serde_json::{Value, json};

    #[test]
    fn publishing_an_output_succeeds_and_leaves_only_the_complete_file() {
        // Regression: calibration run 30304210146, windows-x64 vulkan cell.
        // The worker serialized, synced, and renamed
        // `publication-fault-residency-1-worker-output.json` into place, and
        // then exited 1 with `publish atomic qualification output: Access is
        // denied. (os error 5)` because the post-rename durability step opened
        // the parent directory with `File::open`, which Windows refuses
        // without FILE_FLAG_BACKUP_SEMANTICS. The preserved failure evidence
        // holds the complete published output next to the failed command, so
        // the publication had already happened when the step reported denial.
        // This asserts the publication reports its own success, that the
        // destination holds the whole document, and that the temporary is
        // consumed by the rename rather than left beside it.
        //
        // Its revert value is windows-only. On unix the reverted body is
        // exactly the `#[cfg(not(windows))]` arm this still exercises, so a
        // unix-only lane passes with or without the fix and gives this change
        // no regression protection; the revert-proof was run on windows 11
        // 26200, where it fails with the calibration error.
        let directory = tempfile::tempdir().expect("qualification output directory");
        // The producer hands the worker a private directory; reproduce that
        // here so the publish is exercised, not the private-directory guard.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .expect("make the qualification output directory private");
        }
        let path = directory.path().join("worker-output.json");
        let value = json!({"schema_version": 2, "scenario": "query"});

        write_atomic_json(&path, &value).expect("publish qualification output");

        let published = std::fs::read_to_string(&path).expect("read published output");
        assert!(
            published.ends_with('\n'),
            "published output is truncated: {published:?}"
        );
        assert_eq!(
            serde_json::from_str::<Value>(&published).expect("published output parses"),
            value
        );
        let residue = std::fs::read_dir(directory.path())
            .expect("list qualification output directory")
            .map(|entry| entry.expect("qualification directory entry").file_name())
            .filter(|name| name != "worker-output.json")
            .collect::<Vec<_>>();
        assert!(
            residue.is_empty(),
            "publish left temporaries beside the output: {residue:?}"
        );
    }
}
