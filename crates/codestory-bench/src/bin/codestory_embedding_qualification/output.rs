use super::request::{
    QualificationContracts, QualificationPackage, QualificationRuntime, QualificationSource,
};
use anyhow::{Context, Result, bail};
use serde::Serialize;
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug, Serialize)]
pub(super) struct QualificationRawOutput {
    pub(super) schema_version: u32,
    pub(super) tier: String,
    pub(super) source: QualificationSource,
    pub(super) package: QualificationPackage,
    pub(super) contracts: QualificationContracts,
    pub(super) runtime: QualificationRuntime,
    pub(super) request_sha256: String,
    pub(super) measurements: QualificationMeasurementsSummary,
    pub(super) scenarios: BTreeMap<String, QualificationScenarioSummary>,
}

#[derive(Debug, Serialize)]
pub(super) struct QualificationMeasurementsSummary {
    pub(super) artifact: String,
    pub(super) metric_count: u64,
    pub(super) sample_count: u64,
}

#[derive(Debug, Serialize)]
pub(super) struct QualificationScenarioSummary {
    pub(super) artifact: String,
    pub(super) process_count: u64,
    pub(super) control_event_count: u64,
    pub(super) process_observation_count: u64,
    pub(super) observation_count: u64,
    pub(super) event_count: u64,
}

pub(super) fn write_atomic_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let parent = path
        .parent()
        .context("atomic qualification output has no parent")?;
    super::request::validate_private_directory(parent)?;
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
    fs::File::open(path)?.sync_all()
}

#[cfg(windows)]
fn sync_published_directory(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::write_atomic_json;
    use serde_json::{Value, json};

    #[test]
    fn publishing_an_artifact_succeeds_and_leaves_only_the_complete_file() {
        // Regression: calibration run 30304210146, windows-x64 vulkan cell.
        // The driver shares this publish shape with the worker
        // (`codestory-cli/src/embedding_qualification/worker/gate.rs`), whose
        // copy exited 1 with `publish atomic qualification output: Access is
        // denied. (os error 5)` after its rename had already succeeded: the
        // post-rename durability step opened the parent directory with
        // `File::open`, which Windows refuses without
        // FILE_FLAG_BACKUP_SEMANTICS. This asserts the publication reports its
        // own success, that the destination holds the whole document, and that
        // the temporary is consumed by the rename rather than left beside it.
        //
        // Its revert value is windows-only. On unix the reverted body is
        // exactly the `#[cfg(not(windows))]` arm this still exercises, so a
        // unix-only lane passes with or without the fix and gives this change
        // no regression protection; the revert-proof was run on windows 11
        // 26200, where it fails with the calibration error.
        let directory = tempfile::tempdir().expect("qualification output directory");
        // The producer publishes into a private directory; reproduce that here
        // so the publish is exercised, not the private-directory guard.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .expect("make the qualification output directory private");
        }
        let path = directory.path().join("measurements.raw.json");
        let value = json!({"schema_version": 2, "scenario": "measurements"});

        write_atomic_json(&path, &value).expect("publish qualification artifact");

        let published = std::fs::read_to_string(&path).expect("read published artifact");
        assert!(
            published.ends_with('\n'),
            "published artifact is truncated: {published:?}"
        );
        assert_eq!(
            serde_json::from_str::<Value>(&published).expect("published artifact parses"),
            value
        );
        let residue = std::fs::read_dir(directory.path())
            .expect("list qualification output directory")
            .map(|entry| entry.expect("qualification directory entry").file_name())
            .filter(|name| name != "measurements.raw.json")
            .collect::<Vec<_>>();
        assert!(
            residue.is_empty(),
            "publish left temporaries beside the artifact: {residue:?}"
        );
    }
}
