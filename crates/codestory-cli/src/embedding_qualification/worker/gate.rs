use anyhow::{Context, Result, bail};
use codestory_retrieval::{
    AwakeMonotonicClock, EmbeddingQualificationWorkerError as WorkerError, SidecarRuntimeConfig,
    embedding_retry_state,
};
use codestory_runtime::ProcessStartProbe;
use codestory_workspace::atomic_file::{PublishNewFileError, publish_new_private_file_atomic};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Names the temporaries this protocol leaves beside a publication, so a
/// half-written qualification document is recognisable as one.
const QUALIFICATION_TEMP_PREFIX: &str = "codestory-qualification";
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

pub(super) fn required_absolute_qualification_directory() -> Result<PathBuf> {
    let value = codestory_retrieval::qualification_gate_environment()
        .directory
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

/// Publish a qualification document at `path`, which must not already exist.
///
/// This owns the qualification protocol's vocabulary - the private-directory
/// proof, the refusal to republish, the pretty-printed body with a trailing
/// newline, and the error codes the packaged-proof scripts read - and nothing
/// else. The publication mechanism belongs to
/// [`codestory_workspace::atomic_file`], which the benchmark driver's
/// `write_atomic_json` reaches through as well: the two used to carry
/// byte-identical copies of it, which is why one Windows defect had to be
/// fixed twice.
pub(super) fn write_atomic_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let parent = path
        .parent()
        .context("atomic qualification output has no parent")?;
    validate_private_directory(parent)?;
    if path.exists() {
        bail!("embedding_qualification_output_exists");
    }
    let mut bytes = serde_json::to_vec_pretty(value).context("serialize qualification output")?;
    bytes.push(b'\n');
    match publish_new_private_file_atomic(path, QUALIFICATION_TEMP_PREFIX, &bytes) {
        Ok(()) => Ok(()),
        Err(PublishNewFileError::NoParent) => {
            bail!("atomic qualification output has no parent")
        }
        Err(PublishNewFileError::UnsafeTempPrefix) => {
            bail!("embedding_qualification_temp_prefix_unsafe")
        }
        Err(PublishNewFileError::TempNamesExhausted) => {
            bail!("embedding_qualification_temp_name_exhausted")
        }
        Err(PublishNewFileError::CreateTemp(error)) => {
            Err(error).context("create atomic qualification temp file")
        }
        Err(PublishNewFileError::Publish(error)) => {
            Err(error).context("publish atomic qualification output")
        }
    }
}

pub(super) fn sha256_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub(super) fn qualification_nonce() -> Result<String> {
    codestory_retrieval::qualification_gate_environment()
        .nonce_string()
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
    current_process_start_identity_from_probe(codestory_runtime::process_start_identity(
        std::process::id(),
    ))
}

fn current_process_start_identity_from_probe(probe: ProcessStartProbe) -> Result<String> {
    match probe {
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
    use super::{ProcessStartProbe, current_process_start_identity_from_probe, write_atomic_json};
    use serde_json::{Value, json};

    #[test]
    fn worker_process_start_identity_preserves_all_probe_outcomes() {
        assert_eq!(
            current_process_start_identity_from_probe(ProcessStartProbe::Running {
                start_identity: "start-a".to_string(),
            })
            .expect("running worker identity"),
            "start-a"
        );
        assert_eq!(
            current_process_start_identity_from_probe(ProcessStartProbe::NotRunning)
                .expect_err("missing worker must fail")
                .to_string(),
            "embedding_qualification_worker_not_running"
        );
        assert_eq!(
            current_process_start_identity_from_probe(ProcessStartProbe::Unknown {
                reason: "permission denied".to_string(),
            })
            .expect_err("unknown worker identity must fail")
            .to_string(),
            "embedding_qualification_worker_identity_unknown:permission denied"
        );
    }

    #[test]
    fn publishing_an_output_succeeds_and_leaves_only_the_complete_file() {
        // Regression: calibration run 30304210146, windows-x64 vulkan cell.
        // The worker serialized, synced, and renamed
        // `publication-fault-residency-1-worker-output.json` into place, and
        // then exited 1 with `publish atomic qualification output: Access is
        // denied. (os error 5)` because the post-rename durability step opened
        // the parent directory with `File::open`, which Windows refuses
        // without FILE_FLAG_BACKUP_SEMANTICS.
        //
        // The step now lives once, in
        // `codestory_workspace::atomic_file::sync_parent_directory`, and
        // reverting it is proven there. What this covers is the wrapper: that
        // the worker still reaches that mechanism, that the published document
        // is whole and pretty-printed with a trailing newline, that the
        // temporary is consumed by the rename, and that a second publication
        // is refused with the `embedding_qualification_output_exists` code the
        // packaged-proof scripts read.
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

        let republished = write_atomic_json(&path, &value)
            .expect_err("a published output must not be republished");
        assert!(
            republished
                .to_string()
                .contains("embedding_qualification_output_exists"),
            "unexpected refusal: {republished:?}"
        );
    }
}
