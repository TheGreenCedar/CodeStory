use super::request::{
    QualificationContracts, QualificationPackage, QualificationRuntime, QualificationSource,
};
use anyhow::{Context, Result, bail};
use serde::Serialize;
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
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
            File::open(parent)?.sync_all()?;
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
