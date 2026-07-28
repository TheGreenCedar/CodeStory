use super::request::{
    QualificationContracts, QualificationPackage, QualificationRuntime, QualificationSource,
};
use anyhow::{Context, Result, bail};
use codestory_workspace::atomic_file::{PublishNewFileError, publish_new_private_file_atomic};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;

/// Names the temporaries this protocol leaves beside a publication, so a
/// half-written qualification document is recognisable as one.
const QUALIFICATION_TEMP_PREFIX: &str = "codestory-qualification";

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

/// Publish a qualification document at `path`, which must not already exist.
///
/// The driver writes the same protocol the worker reads, so this states the
/// same contract as `codestory_cli`'s `write_atomic_json` - private-directory
/// proof, refusal to republish, pretty body with a trailing newline - and
/// reaches the publication mechanism at the same owner,
/// [`codestory_workspace::atomic_file`]. The two used to carry byte-identical
/// copies of that mechanism, which is why one Windows defect had to be fixed
/// twice.
pub(super) fn write_atomic_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let parent = path
        .parent()
        .context("atomic qualification output has no parent")?;
    super::request::validate_private_directory(parent)?;
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

#[cfg(test)]
mod tests {
    use super::write_atomic_json;
    use serde_json::{Value, json};

    #[test]
    fn publishing_an_artifact_succeeds_and_leaves_only_the_complete_file() {
        // Regression: calibration run 30304210146, windows-x64 vulkan cell.
        // The driver shared this publish shape with the worker
        // (`codestory-cli/src/embedding_qualification/worker/gate.rs`), whose
        // copy exited 1 with `publish atomic qualification output: Access is
        // denied. (os error 5)` after its rename had already succeeded: the
        // post-rename durability step opened the parent directory with
        // `File::open`, which Windows refuses without
        // FILE_FLAG_BACKUP_SEMANTICS.
        //
        // The step now lives once, in
        // `codestory_workspace::atomic_file::sync_parent_directory`, and
        // reverting it is proven there. What this covers is the wrapper: that
        // the driver still reaches that mechanism, that the published document
        // is whole and pretty-printed with a trailing newline, that the
        // temporary is consumed by the rename, and that a second publication
        // is refused with the `embedding_qualification_output_exists` code the
        // packaged-proof scripts read.
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

        let republished = write_atomic_json(&path, &value)
            .expect_err("a published artifact must not be republished");
        assert!(
            republished
                .to_string()
                .contains("embedding_qualification_output_exists"),
            "unexpected refusal: {republished:?}"
        );
    }
}
