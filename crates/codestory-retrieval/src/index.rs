use crate::config::{SidecarLayout, SidecarRuntimeConfig, dir_size_bytes};
use crate::embedded_vector::{
    AttestedSemanticPoint, EmbeddedVectorIndex, ExpectedVectorAnchor, SemanticPoint,
    VectorEvidenceContract, VectorGenerationManifest, build_vector_producer_evidence,
    producer_evidence_mismatches, vector_compatibility_identity,
    vector_producer_compatibility_identity,
};
use crate::generation::{
    SIDECAR_SCHEMA_VERSION, manifest_has_current_sidecar_contract,
    manifest_unavailable_reason_for_runtime, sidecar_generation_id,
};
use crate::health::probe_sidecar_health_for_runtime;
use crate::lexical_index::{
    LEXICAL_INDEX_VERSION, LexicalInputFingerprint, build_lexical_shard,
    finish_lexical_input_for_store, lexical_source_input,
};
use crate::retention::{
    FsGenerationRemover, GLOBAL_GENERATION_GC_LOCK_SCOPE, GenerationRetentionApplyReport,
    GenerationRetentionLock, GenerationRetentionMarker, GenerationRetentionPlan,
    apply_generation_retention, global_generation_gc_state_file, plan_generation_retention,
    scan_retention_protection, write_retention_marker,
};
use crate::scip_index::{
    SCIP_PRECISE_SEMANTIC_IMPORT_DIR, emit_scip_artifacts_from_store,
    import_precise_semantic_scip_artifact,
};
use anyhow::{Context, Result, bail};
use chrono::Utc;
use codestory_contracts::api::EmbeddingVectorPublicationIdentityDto;
#[cfg(test)]
use codestory_store::LlmSymbolDoc;
use codestory_store::{
    DenseAnchorInput, FileRole, RetrievalIndexManifest, RetrievalIndexRollbackRecord, Store,
    SymbolSearchDoc,
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
#[cfg(any(not(feature = "test-support"), test))]
use std::fs::{self, File, OpenOptions};
#[cfg(any(not(feature = "test-support"), test))]
use std::io::{Read, Write};
use std::path::Path;
#[cfg(any(not(feature = "test-support"), test))]
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(any(not(feature = "test-support"), test))]
use std::time::{Duration, Instant};
use tracing::{info, warn};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FinalizeIndexOutcome {
    pub project_id: String,
    pub manifest: RetrievalIndexManifest,
    pub degraded_modes: Vec<String>,
    pub scip_stubbed: bool,
    pub generation_retention_plan: GenerationRetentionPlan,
    pub generation_retention: GenerationRetentionApplyReport,
}

/// Typed signal that source-derived retrieval input drifted during preparation.
#[derive(Debug, Clone, thiserror::Error)]
#[error("sidecar generation input changed while {stage}: expected {expected}, observed {observed}")]
pub struct SidecarInputChanged {
    stage: String,
    expected: String,
    observed: String,
}

impl SidecarInputChanged {
    pub(crate) fn new(
        stage: impl Into<String>,
        expected: impl Into<String>,
        observed: impl Into<String>,
    ) -> Self {
        Self {
            stage: stage.into(),
            expected: expected.into(),
            observed: observed.into(),
        }
    }
}

#[derive(Debug, Clone, thiserror::Error)]
#[error("retrieval index cancelled before {boundary}")]
pub struct RetrievalIndexCancelled {
    boundary: &'static str,
}

pub fn is_retrieval_index_cancelled(error: &anyhow::Error) -> bool {
    error.downcast_ref::<RetrievalIndexCancelled>().is_some()
}

pub fn is_sidecar_input_changed(error: &anyhow::Error) -> bool {
    error.downcast_ref::<SidecarInputChanged>().is_some()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SidecarInputFingerprint {
    pub(crate) hash: String,
    pub(crate) symbol_doc_count: i64,
    pub(crate) projection_count: i64,
    pub(crate) dense_projection_count: i64,
    pub(crate) semantic_policy_version: Option<String>,
    pub(crate) graph_artifact_hash: String,
    pub(crate) dense_reason_counts_json: String,
    pub(crate) lexical_file_count: u32,
    pub(crate) lexical_hash: String,
    pub(crate) lexical_coverage: crate::lexical_index::LexicalCoverage,
}

struct SidecarEmbeddingContract<'a> {
    backend: &'a str,
    dimension: i32,
    producer_compatibility_identity: &'a str,
}

struct SemanticGeneration<'a> {
    layout: &'a SidecarLayout,
    collection: &'a str,
    generation: &'a str,
    input_hash: &'a str,
    embedding_backend: &'a str,
    embedding_dim: i32,
    expected_points: i64,
}

#[derive(Debug, Clone, Copy)]
struct SidecarStubFlags {
    scip_stubbed: bool,
}

struct LexicalGenerationOutcome {
    version: String,
}

struct GenerationRetentionContext<'a> {
    runtime: &'a SidecarRuntimeConfig,
    layout: &'a SidecarLayout,
    workspace_id: &'a str,
    previous_manifest: Option<&'a RetrievalIndexManifest>,
    embedding_device: &'a crate::embeddings::EmbeddingDeviceReadiness,
    embedding_residency: crate::embeddings::ProductEmbeddingResidencyLease,
    producer_compatibility_identity: String,
}

struct PreparedGenerationRetention {
    verified_previous: Option<RetrievalIndexRollbackRecord>,
}

const SIDECAR_INPUT_BATCH_SIZE: usize = 4096;
#[cfg(any(not(feature = "test-support"), test))]
const EMBEDDING_QUALIFICATION_DIR_ENV: &str = "CODESTORY_EMBED_QUALIFICATION_DIR";
#[cfg(any(not(feature = "test-support"), test))]
const EMBEDDING_QUALIFICATION_NONCE_ENV: &str = "CODESTORY_EMBED_QUALIFICATION_NONCE";
#[cfg(any(not(feature = "test-support"), test))]
const PUBLICATION_QUALIFICATION_SCHEMA_VERSION: u32 = 1;
#[cfg(any(not(feature = "test-support"), test))]
const PUBLICATION_QUALIFICATION_MAX_CONTROL_BYTES: u64 = 16 * 1024;
#[cfg(any(not(feature = "test-support"), test))]
const PUBLICATION_QUALIFICATION_POLL_INTERVAL: Duration = Duration::from_millis(10);
#[cfg(any(not(feature = "test-support"), test))]
const PUBLICATION_QUALIFICATION_WAIT_TIMEOUT: Duration = Duration::from_secs(120);

#[cfg(any(not(feature = "test-support"), test))]
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct PublicationQualificationCommand {
    schema_version: u32,
    nonce_sha256: String,
    correlation_id: String,
    action: String,
}

#[cfg(any(not(feature = "test-support"), test))]
#[derive(Debug, serde::Serialize)]
struct PublicationQualificationEventClock {
    domain: &'static str,
    api: &'static str,
    elapsed_ns: u64,
}

#[cfg(any(not(feature = "test-support"), test))]
#[derive(Debug, serde::Serialize)]
struct PublicationQualificationEvent<'a> {
    schema_version: u32,
    sequence: u64,
    correlation_id: &'a str,
    action: &'a str,
    status: &'a str,
    clock: PublicationQualificationEventClock,
}

#[cfg(any(not(feature = "test-support"), test))]
struct PublicationQualificationHook {
    directory: PathBuf,
    nonce_sha256: String,
    correlation_id: String,
    events: File,
    started: Instant,
    sequence: u64,
}

#[cfg(any(not(feature = "test-support"), test))]
impl PublicationQualificationHook {
    #[cfg(not(feature = "test-support"))]
    fn from_environment() -> Result<Option<Self>> {
        Self::from_environment_values(
            std::env::var_os(EMBEDDING_QUALIFICATION_DIR_ENV),
            std::env::var(EMBEDDING_QUALIFICATION_NONCE_ENV).ok(),
        )
    }

    fn from_environment_values(
        directory: Option<std::ffi::OsString>,
        nonce: Option<String>,
    ) -> Result<Option<Self>> {
        match (directory, nonce) {
            (None, None) => Ok(None),
            (Some(directory), Some(nonce)) => Self::from_gate(&PathBuf::from(directory), &nonce),
            _ => bail!(
                "embedding_publication_qualification_gate_incomplete: both \
                 {EMBEDDING_QUALIFICATION_DIR_ENV} and \
                 {EMBEDDING_QUALIFICATION_NONCE_ENV} are required"
            ),
        }
    }

    fn from_gate(directory: &Path, nonce: &str) -> Result<Option<Self>> {
        if nonce.is_empty()
            || nonce.len() > 128
            || !nonce
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            bail!("embedding_publication_qualification_nonce_invalid");
        }
        let directory = validate_publication_qualification_directory(directory)?;
        let nonce_sha256 = hex_sha256(nonce.as_bytes());
        let control_path = directory.join(format!("publication-pause-{nonce_sha256}.json"));
        let Some(command) =
            read_publication_qualification_command(&control_path, "pause_before_manifest_commit")?
        else {
            return Ok(None);
        };
        if command.nonce_sha256 != nonce_sha256 {
            bail!("embedding_publication_qualification_nonce_mismatch");
        }
        validate_publication_qualification_correlation_id(&command.correlation_id)?;
        let events_path = directory.join(format!(
            "publication-events-{}.jsonl",
            command.correlation_id
        ));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let events = options
            .open(&events_path)
            .context("create private publication qualification event log")?;
        Ok(Some(Self {
            directory,
            nonce_sha256,
            correlation_id: command.correlation_id,
            events,
            started: Instant::now(),
            sequence: 0,
        }))
    }

    fn pause_before_lease_revalidation(&mut self) -> Result<()> {
        self.record("pause_before_manifest_commit", "waiting_for_resume")?;
        let resume_path = self
            .directory
            .join(format!("publication-resume-{}.json", self.correlation_id));
        let deadline = self.started + PUBLICATION_QUALIFICATION_WAIT_TIMEOUT;
        loop {
            if Instant::now() >= deadline {
                self.record("resume_manifest_commit", "timed_out")?;
                bail!("embedding_publication_qualification_resume_timeout");
            }
            match read_publication_qualification_command(&resume_path, "resume_manifest_commit")? {
                Some(command)
                    if command.nonce_sha256 == self.nonce_sha256
                        && command.correlation_id == self.correlation_id =>
                {
                    self.record("resume_manifest_commit", "observed")?;
                    return Ok(());
                }
                Some(_) => {
                    self.record("resume_manifest_commit", "rejected")?;
                    bail!("embedding_publication_qualification_resume_mismatch");
                }
                None => std::thread::sleep(PUBLICATION_QUALIFICATION_POLL_INTERVAL),
            }
        }
    }

    fn record(&mut self, action: &str, status: &str) -> Result<()> {
        let elapsed_ns = u64::try_from(self.started.elapsed().as_nanos()).unwrap_or(u64::MAX);
        let event = PublicationQualificationEvent {
            schema_version: PUBLICATION_QUALIFICATION_SCHEMA_VERSION,
            sequence: self.sequence,
            correlation_id: &self.correlation_id,
            action,
            status,
            clock: PublicationQualificationEventClock {
                domain: "process_monotonic",
                api: "std::time::Instant",
                elapsed_ns,
            },
        };
        serde_json::to_writer(&mut self.events, &event)
            .context("encode publication qualification event")?;
        self.events
            .write_all(b"\n")
            .context("terminate publication qualification event")?;
        self.events
            .flush()
            .context("flush publication qualification event")?;
        self.events
            .sync_all()
            .context("sync publication qualification event")?;
        self.sequence = self.sequence.saturating_add(1);
        Ok(())
    }
}

#[cfg(any(not(feature = "test-support"), test))]
fn validate_publication_qualification_directory(path: &Path) -> Result<PathBuf> {
    if !path.is_absolute() {
        bail!("embedding_publication_qualification_directory_not_absolute");
    }
    let metadata = fs::symlink_metadata(path).with_context(|| {
        format!(
            "inspect publication qualification directory {}",
            path.display()
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        bail!("embedding_publication_qualification_directory_untrusted");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o077 != 0 {
            bail!("embedding_publication_qualification_directory_untrusted");
        }
    }
    fs::canonicalize(path).with_context(|| {
        format!(
            "canonicalize publication qualification directory {}",
            path.display()
        )
    })
}

#[cfg(any(not(feature = "test-support"), test))]
fn read_publication_qualification_command(
    path: &Path,
    expected_action: &str,
) -> Result<Option<PublicationQualificationCommand>> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "inspect publication qualification control {}",
                    path.display()
                )
            });
        }
    };
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() > PUBLICATION_QUALIFICATION_MAX_CONTROL_BYTES
    {
        bail!("embedding_publication_qualification_control_untrusted");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o077 != 0 {
            bail!("embedding_publication_qualification_control_untrusted");
        }
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let file = options
        .open(path)
        .with_context(|| format!("open publication qualification control {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let opened = file
            .metadata()
            .context("inspect opened publication qualification control")?;
        if opened.dev() != metadata.dev()
            || opened.ino() != metadata.ino()
            || opened.uid() != metadata.uid()
            || opened.mode() & 0o077 != 0
        {
            bail!("embedding_publication_qualification_control_replaced");
        }
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(PUBLICATION_QUALIFICATION_MAX_CONTROL_BYTES + 1)
        .read_to_end(&mut bytes)
        .context("read publication qualification control")?;
    if bytes.len() as u64 > PUBLICATION_QUALIFICATION_MAX_CONTROL_BYTES {
        bail!("embedding_publication_qualification_control_too_large");
    }
    let command: PublicationQualificationCommand =
        serde_json::from_slice(&bytes).context("parse publication qualification control")?;
    if command.schema_version != PUBLICATION_QUALIFICATION_SCHEMA_VERSION
        || command.action != expected_action
    {
        bail!("embedding_publication_qualification_control_invalid");
    }
    Ok(Some(command))
}

#[cfg(any(not(feature = "test-support"), test))]
fn validate_publication_qualification_correlation_id(value: &str) -> Result<()> {
    if value.len() != 32
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        bail!("embedding_publication_qualification_correlation_invalid");
    }
    Ok(())
}

#[cfg(any(not(feature = "test-support"), test))]
fn hex_sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn ensure_retrieval_index_not_cancelled(
    cancelled: &AtomicBool,
    boundary: &'static str,
) -> Result<()> {
    if cancelled.load(Ordering::Acquire) {
        return Err(RetrievalIndexCancelled { boundary }.into());
    }
    Ok(())
}

pub fn project_id_for_root(project_root: &Path) -> String {
    codestory_workspace::workspace_id_v3_for_root(project_root)
}

pub fn sidecar_project_id_for_root(project_root: &Path) -> String {
    codestory_workspace::project_identity_v3(project_root).artifact_scope_id
}

pub(crate) fn sidecar_project_id_for_runtime(
    project_root: &Path,
    runtime: &SidecarRuntimeConfig,
) -> Result<String> {
    Ok(runtime
        .validated_project_identity(project_root)?
        .artifact_scope_id)
}

pub fn finalize_index(project_root: &Path, storage_path: &Path) -> Result<FinalizeIndexOutcome> {
    let runtime = crate::config::SidecarRuntimeConfig::for_project_auto(project_root);
    finalize_index_for_runtime(project_root, storage_path, &runtime)
}

pub fn finalize_index_for_runtime(
    project_root: &Path,
    storage_path: &Path,
    runtime: &SidecarRuntimeConfig,
) -> Result<FinalizeIndexOutcome> {
    finalize_index_for_runtime_with_progress(project_root, storage_path, runtime, |_| {})
}

pub fn finalize_index_for_runtime_with_cancel(
    project_root: &Path,
    storage_path: &Path,
    runtime: &SidecarRuntimeConfig,
    cancelled: &AtomicBool,
) -> Result<FinalizeIndexOutcome> {
    finalize_index_for_runtime_with_progress_and_cancel(
        project_root,
        storage_path,
        runtime,
        cancelled,
        |_| {},
    )
}

pub fn finalize_index_for_runtime_with_progress(
    project_root: &Path,
    storage_path: &Path,
    runtime: &SidecarRuntimeConfig,
    progress: impl FnMut(&'static str),
) -> Result<FinalizeIndexOutcome> {
    let cancelled = AtomicBool::new(false);
    finalize_index_for_runtime_with_progress_and_cancel(
        project_root,
        storage_path,
        runtime,
        &cancelled,
        progress,
    )
}

pub fn finalize_index_for_runtime_with_progress_and_cancel(
    project_root: &Path,
    storage_path: &Path,
    runtime: &SidecarRuntimeConfig,
    cancelled: &AtomicBool,
    mut progress: impl FnMut(&'static str),
) -> Result<FinalizeIndexOutcome> {
    ensure_retrieval_index_not_cancelled(cancelled, "preflight")?;
    let layout = runtime.layout.clone();
    let project_identity = runtime.validated_project_identity(project_root)?;
    let project_id = project_identity.artifact_scope_id;
    let workspace_id = project_identity.workspace_id;
    let global_gc_state_file = global_generation_gc_state_file(runtime);
    let _global_gc_lock = GenerationRetentionLock::acquire_shared(
        &global_gc_state_file,
        GLOBAL_GENERATION_GC_LOCK_SCOPE,
    )
    .context("coordinate sidecar publication with global generation cleanup")?;
    let _generation_lock = GenerationRetentionLock::acquire(&layout.state_file, &project_id)
        .context("lock sidecar generation publication and retention")?;
    layout.ensure_data_dirs()?;
    let embedding_residency =
        crate::embeddings::acquire_product_embedding_residency_for_runtime(runtime)
            .context("mandatory retrieval embedding runtime could not be pinned")?;
    let degraded_modes = Vec::new();
    let scip_stubbed = false;

    let embedding_backend = crate::embeddings::embedding_runtime_id_for_runtime(runtime);
    let embedding_dim = i32::try_from(crate::embeddings::semantic_vector_dim())
        .unwrap_or(crate::embeddings::RETRIEVAL_EMBEDDING_DIM as i32);
    crate::embeddings::ensure_product_embedding_backend_for_runtime(runtime)?;
    let embedding_device = crate::embeddings::embedding_device_readiness_for_runtime(runtime);
    let producer_compatibility_identity = vector_producer_compatibility_identity(
        &embedding_device,
        embedding_residency.identity(),
        u32::try_from(embedding_dim).context("negative embedding dimension")?,
    )?;

    let storage = Store::open(storage_path).context("open storage for retrieval sidecar input")?;
    let lexical_source = lexical_source_input(project_root).context("hash lexical source input")?;
    let input_snapshot = storage
        .read_snapshot()
        .context("open coherent sidecar input snapshot")?;
    let embedding_contract = SidecarEmbeddingContract {
        backend: &embedding_backend,
        dimension: embedding_dim,
        producer_compatibility_identity: &producer_compatibility_identity,
    };
    let sidecar_input = compute_sidecar_input_fingerprint_with_lexical_source(
        input_snapshot.storage(),
        project_root,
        &project_id,
        &embedding_contract,
        lexical_source,
    )?;
    input_snapshot
        .finish()
        .context("finish coherent sidecar input snapshot")?;
    let previous_manifest = storage
        .get_retrieval_index_manifest(&project_id)
        .context("load previous retrieval_index_manifest")?;
    let mut previous_manifest_unavailable_reason =
        previous_manifest.as_ref().and_then(|manifest| {
            manifest_unavailable_reason_for_runtime(&project_id, &storage, manifest, runtime)
        });
    if previous_manifest_unavailable_reason.is_none()
        && let Some(manifest) = previous_manifest.as_ref()
        && manifest_matches_sidecar_input(
            manifest,
            &sidecar_input,
            &embedding_backend,
            embedding_dim,
        )
    {
        let validation = storage
            .get_complete_index_publication()
            .context("load complete core publication for retrieval reuse")?
            .context("retrieval reuse requires a complete core publication")
            .and_then(|publication| {
                crate::embedded_vector::validate_generation_evidence_for_publication(
                    &layout,
                    &storage,
                    manifest,
                    &publication,
                    runtime,
                    &embedding_device,
                    embedding_residency.identity(),
                )
                .context("deep-validate reusable retrieval generation")
            });
        if let Err(error) = validation {
            previous_manifest_unavailable_reason =
                Some(format!("retrieval_generation_evidence_invalid: {error:#}"));
        }
    }
    drop(storage);
    let retention_context = GenerationRetentionContext {
        runtime,
        layout: &layout,
        workspace_id: &workspace_id,
        previous_manifest: previous_manifest.as_ref(),
        embedding_device: &embedding_device,
        embedding_residency,
        producer_compatibility_identity,
    };

    if let Some(previous) = previous_manifest.as_ref() {
        if let Some(reason) = previous_manifest_unavailable_reason.as_ref() {
            warn!(
                project_id = %project_id,
                reason = %reason,
                "existing retrieval sidecar manifest is stale; rebuilding"
            );
        } else if manifest_matches_sidecar_input(
            previous,
            &sidecar_input,
            &embedding_backend,
            embedding_dim,
        ) {
            let status = probe_sidecar_health_for_runtime(
                &layout,
                &project_id,
                Some(previous.clone()),
                &embedding_device,
                runtime,
            );
            let previous_semantic = SemanticGeneration {
                layout: &layout,
                collection: &previous.semantic_generation,
                generation: previous.sidecar_generation.as_deref().unwrap_or_default(),
                input_hash: previous.sidecar_input_hash.as_deref().unwrap_or_default(),
                embedding_backend: &embedding_backend,
                embedding_dim,
                expected_points: sidecar_input.projection_count,
            };
            let semantic_point_count = semantic_ready_point_count(&previous_semantic);
            if status.retrieval_mode == "full" && semantic_point_count.is_some() {
                let mut manifest = previous.clone();
                if let Some(generation) = manifest.sidecar_generation.as_deref() {
                    let scip_dir = layout.scip_project_dir(generation);
                    if update_precise_semantic_import_status(&scip_dir, &mut manifest)? {
                        return persist_finalized_manifest(
                            project_root,
                            storage_path,
                            &retention_context,
                            cancelled,
                            &sidecar_input,
                            project_id,
                            manifest,
                            degraded_modes,
                            SidecarStubFlags { scip_stubbed },
                        );
                    }
                }
                info!(
                    project_id = %project_id,
                    sidecar_generation = ?previous.sidecar_generation,
                    projection_count = sidecar_input.projection_count,
                    semantic_point_count = semantic_point_count.unwrap_or_default(),
                    lexical_file_count = sidecar_input.lexical_file_count,
                    "retrieval sidecar generation unchanged; reused existing full sidecars"
                );
                return persist_finalized_manifest(
                    project_root,
                    storage_path,
                    &retention_context,
                    cancelled,
                    &sidecar_input,
                    project_id,
                    previous.clone(),
                    degraded_modes,
                    SidecarStubFlags { scip_stubbed },
                );
            }
            warn!(
                project_id = %project_id,
                retrieval_mode = %status.retrieval_mode,
                degraded_reason = ?status.degraded_reason,
                semantic_point_count = ?semantic_point_count,
                "sidecar input unchanged but current generation is not healthy; rebuilding"
            );
        }
    }

    let generation = sidecar_generation_id(&project_id, &sidecar_input.hash);
    let collection = crate::generation::sidecar_vector_generation(&project_id, &sidecar_input.hash);
    let scip_dir = layout.scip_project_dir(&generation);
    let semantic_generation = SemanticGeneration {
        layout: &layout,
        collection: &collection,
        generation: &generation,
        input_hash: &sidecar_input.hash,
        embedding_backend: &embedding_backend,
        embedding_dim,
        expected_points: sidecar_input.projection_count,
    };
    let mut manifest = retrieval_manifest_for_sidecar(
        &project_id,
        &generation,
        &collection,
        &embedding_backend,
        embedding_dim,
        &sidecar_input,
    );
    manifest.scip_revision = read_scip_revision(&scip_dir);

    let existing_status = probe_sidecar_health_for_runtime(
        &layout,
        &project_id,
        Some(manifest.clone()),
        &embedding_device,
        runtime,
    );
    let lexical_ready = existing_status.lexical.capabilities.lexical
        && crate::lexical_index::shard_matches_lexical_input(
            &layout.lexical_data_dir,
            &generation,
            sidecar_input.lexical_file_count,
            &sidecar_input.lexical_hash,
            &sidecar_input.hash,
        );
    let semantic_ready_points = if existing_status.semantic.capabilities.semantic {
        semantic_ready_point_count(&semantic_generation)
    } else {
        None
    };
    let scip_ready = existing_status.scip.capabilities.graph;

    let lexical_outcome = with_finalize_progress(&mut progress, "lexical sidecar", || {
        ensure_lexical_generation(
            project_root,
            storage_path,
            &layout,
            &generation,
            &LexicalInputFingerprint {
                file_count: sidecar_input.lexical_file_count,
                hash: sidecar_input.lexical_hash.clone(),
                coverage: sidecar_input.lexical_coverage.clone(),
            },
            &sidecar_input.hash,
            lexical_ready,
        )
    })?;

    let _semantic_point_count = ensure_semantic_index(
        storage_path,
        &project_id,
        &semantic_generation,
        semantic_ready_points,
        &retention_context,
        cancelled,
        &mut progress,
    )?;

    with_finalize_progress(&mut progress, "graph artifact", || {
        ensure_scip_artifacts(
            storage_path,
            &scip_dir,
            &project_id,
            &generation,
            scip_ready,
            &mut manifest,
        )
    })?;
    update_precise_semantic_import_status(&scip_dir, &mut manifest)?;

    manifest.lexical_version = lexical_outcome.version;
    manifest.scip_revision = read_scip_revision(&scip_dir).or(manifest.scip_revision);
    manifest.disk_bytes = sidecar_disk_bytes(&layout, &generation, &collection, &scip_dir);

    with_finalize_progress(&mut progress, "manifest write", || {
        persist_finalized_manifest(
            project_root,
            storage_path,
            &retention_context,
            cancelled,
            &sidecar_input,
            project_id,
            manifest,
            degraded_modes,
            SidecarStubFlags { scip_stubbed },
        )
    })
}

fn with_finalize_progress<T>(
    progress: &mut impl FnMut(&'static str),
    phase: &'static str,
    action: impl FnOnce() -> Result<T>,
) -> Result<T> {
    progress(phase);
    action()
}

fn ensure_lexical_generation(
    project_root: &Path,
    storage_path: &Path,
    layout: &SidecarLayout,
    generation: &str,
    expected: &LexicalInputFingerprint,
    sidecar_input_hash: &str,
    mut lexical_ready: bool,
) -> Result<LexicalGenerationOutcome> {
    if lexical_ready {
        info!(sidecar_generation = %generation, "SQLite lexical shard reused");
    } else {
        match build_lexical_shard(
            project_root,
            Some(storage_path),
            &layout.lexical_data_dir,
            generation,
            expected,
            sidecar_input_hash,
        ) {
            Ok(_) => {
                lexical_ready = true;
                info!(sidecar_generation = %generation, "SQLite lexical shard built");
            }
            Err(error) => {
                bail!("mandatory SQLite lexical shard build failed for {generation}: {error}")
            }
        }
    }
    if !lexical_ready
        || !crate::lexical_index::shard_matches_lexical_input(
            &layout.lexical_data_dir,
            generation,
            expected.file_count,
            &expected.hash,
            sidecar_input_hash,
        )
    {
        bail!("mandatory SQLite lexical shard is incomplete for {generation}");
    }
    Ok(LexicalGenerationOutcome {
        version: LEXICAL_INDEX_VERSION.to_string(),
    })
}

fn ensure_semantic_index(
    storage_path: &Path,
    project_id: &str,
    semantic: &SemanticGeneration<'_>,
    semantic_ready_points: Option<u64>,
    retention: &GenerationRetentionContext<'_>,
    cancelled: &AtomicBool,
    progress: &mut impl FnMut(&'static str),
) -> Result<u64> {
    ensure_retrieval_index_not_cancelled(cancelled, "pinning dense anchor inputs")?;
    let storage = Store::open(storage_path).context("open core storage for dense anchors")?;
    let snapshot = storage
        .read_snapshot()
        .context("pin dense anchor input generation")?;
    let publication = snapshot
        .storage()
        .get_complete_index_publication()
        .context("read pinned core publication for vector generation")?
        .context("dense anchor inputs require a complete core publication")?;
    let expected_source_identity =
        format!("core:{}:{}", publication.generation_id, publication.run_id);
    let mut anchors = Vec::<DenseAnchorInput>::new();
    let mut after = None;
    loop {
        let batch = snapshot
            .storage()
            .get_dense_anchor_inputs_batch_after(after, SIDECAR_INPUT_BATCH_SIZE)
            .context("load pinned dense anchor inputs")?;
        if batch.is_empty() {
            break;
        }
        after = batch.last().map(|anchor| anchor.node_id);
        for anchor in batch {
            if anchor.source_identity != expected_source_identity {
                bail!(
                    "dense anchor {} belongs to source identity {}, expected {}",
                    anchor.node_id.0,
                    anchor.source_identity,
                    expected_source_identity
                );
            }
            anchors.push(anchor);
        }
    }
    if i64::try_from(anchors.len()).unwrap_or(i64::MAX) != semantic.expected_points {
        bail!(
            "pinned dense anchor generation count changed: expected {}, found {}",
            semantic.expected_points,
            anchors.len()
        );
    }
    let evidence = build_vector_producer_evidence(
        retention.embedding_device,
        retention.embedding_residency.identity(),
        u32::try_from(semantic.embedding_dim).context("negative embedding dimension")?,
        EmbeddingVectorPublicationIdentityDto {
            core_generation_id: publication.generation_id.clone(),
            core_run_id: publication.run_id.clone(),
            retrieval_generation: semantic.generation.to_string(),
            retrieval_input_hash: semantic.input_hash.to_string(),
            semantic_generation: semantic.collection.to_string(),
        },
    );
    let compatibility_identity = vector_compatibility_identity(&evidence)?;
    let dimension =
        usize::try_from(semantic.embedding_dim).context("negative embedding dimension")?;
    let contract = VectorEvidenceContract::new(
        semantic.embedding_backend,
        dimension,
        crate::embeddings::PRODUCT_EMBEDDING_RUNTIME_ID,
        &compatibility_identity,
    );
    let expected_anchors = anchors
        .iter()
        .map(|anchor| ExpectedVectorAnchor {
            node_id: anchor.node_id.0.to_string(),
            document_hash: anchor.document_hash.clone(),
        })
        .collect::<Vec<_>>();

    let reusable_point_count = semantic_ready_points.and_then(|point_count| {
        let validation = (|| {
            let manifest = EmbeddedVectorIndex::load_generation_manifest(
                semantic.layout,
                semantic.collection,
            )?;
            let mismatches = producer_evidence_mismatches(&evidence, &manifest.evidence);
            if !mismatches.is_empty() {
                bail!(
                    "stored vector producer evidence is incompatible: {}",
                    mismatches.join(", ")
                );
            }
            EmbeddedVectorIndex::validate_published_attestation(
                semantic.layout,
                semantic.collection,
                semantic.generation,
                semantic.input_hash,
                &contract,
                &expected_anchors,
                &manifest.vectors,
            )
        })();
        match validation {
            Ok(_) => Some(point_count),
            Err(error) => {
                warn!(
                    project_id = %project_id,
                    sidecar_generation = %semantic.generation,
                    error = %format!("{error:#}"),
                    "existing vector candidate is incomplete or incompatible; rebuilding"
                );
                None
            }
        }
    });

    let point_count = if let Some(point_count) = reusable_point_count {
        info!(
            project_id = %project_id,
            sidecar_generation = %semantic.generation,
            point_count,
            "attested vector generation reused"
        );
        point_count
    } else {
        let attestation = with_finalize_progress(progress, "embedded vectors", || {
            EmbeddedVectorIndex::build_attested_with_points_with_cancel(
                crate::embedded_vector::AttestedVectorPublication {
                    layout: semantic.layout,
                    collection: semantic.collection,
                    generation: semantic.generation,
                    input_hash: semantic.input_hash,
                    contract: &contract,
                    expected_anchors: &expected_anchors,
                },
                || ensure_retrieval_index_not_cancelled(cancelled, "vector database publication"),
                |visit| {
                    let client = crate::embeddings::ProductEmbeddingClient::new(retention.runtime);
                    for batch in
                        anchors.chunks(retention.runtime.retrieval.llm_doc_embed_batch_size.max(1))
                    {
                        ensure_retrieval_index_not_cancelled(cancelled, "embedding batch")?;
                        let texts = batch
                            .iter()
                            .map(|anchor| anchor.text.clone())
                            .collect::<Vec<_>>();
                        let vectors = client
                            .embed_documents_with_control(&texts, None, &|| {
                                cancelled.load(Ordering::Acquire)
                            })
                            .context("embed pinned dense anchor batch")?;
                        ensure_retrieval_index_not_cancelled(
                            cancelled,
                            "persisting an embedding batch",
                        )?;
                        if vectors.len() != batch.len() {
                            bail!(
                                "embedding engine returned {} vectors for {} anchors",
                                vectors.len(),
                                batch.len()
                            );
                        }
                        for (anchor, vector) in batch.iter().zip(vectors) {
                            ensure_retrieval_index_not_cancelled(
                                cancelled,
                                "persisting an embedded vector",
                            )?;
                            visit(AttestedSemanticPoint {
                                point: SemanticPoint {
                                    display_name: anchor
                                        .qualified_name
                                        .clone()
                                        .unwrap_or_else(|| anchor.display_name.clone()),
                                    node_id: anchor.node_id.0.to_string(),
                                    file_path: anchor.file_path.clone(),
                                    file_role: Some(anchor.file_role),
                                    dense_reason: Some(anchor.selection_reason.clone()),
                                    vector: normalize_vector(vector)?,
                                },
                                document_hash: anchor.document_hash.clone(),
                            })?;
                        }
                    }
                    Ok(())
                },
            )
        })?;
        let point_count = attestation.point_count;
        let generation_manifest = VectorGenerationManifest::new(evidence, attestation.clone())?;
        EmbeddedVectorIndex::publish_generation_manifest_with_cancel(
            semantic.layout,
            semantic.collection,
            &generation_manifest,
            || {
                ensure_retrieval_index_not_cancelled(
                    cancelled,
                    "producer-evidence manifest publication",
                )
            },
        )?;
        EmbeddedVectorIndex::validate_published_attestation(
            semantic.layout,
            semantic.collection,
            semantic.generation,
            semantic.input_hash,
            &contract,
            &expected_anchors,
            &attestation,
        )?;
        point_count
    };
    snapshot
        .finish()
        .context("finish pinned dense anchor input generation")?;
    if point_count != u64::try_from(semantic.expected_points).unwrap_or(u64::MAX) {
        bail!(
            "embedded vector generation incomplete for {project_id}: expected {} points, found {point_count}",
            semantic.expected_points
        );
    }
    info!(
        project_id = %project_id,
        sidecar_generation = %semantic.generation,
        point_count,
        "embedded SQLite vector generation published"
    );
    Ok(point_count)
}

fn normalize_vector(mut vector: Vec<f32>) -> Result<Vec<f32>> {
    if vector.is_empty() || vector.iter().any(|value| !value.is_finite()) {
        bail!("embedding engine returned an empty or non-finite vector");
    }
    let norm = vector
        .iter()
        .map(|value| f64::from(*value) * f64::from(*value))
        .sum::<f64>()
        .sqrt();
    if !norm.is_finite() || norm <= f64::EPSILON {
        bail!("embedding engine returned a zero vector");
    }
    for value in &mut vector {
        *value = (f64::from(*value) / norm) as f32;
    }
    Ok(vector)
}

fn ensure_scip_artifacts(
    storage_path: &Path,
    scip_dir: &Path,
    project_id: &str,
    generation: &str,
    scip_ready: bool,
    manifest: &mut RetrievalIndexManifest,
) -> Result<()> {
    if scip_ready {
        info!(project_id = %project_id, sidecar_generation = %generation, "SCIP graph artifacts reused");
        return Ok(());
    }
    match emit_scip_artifacts_from_store(storage_path, scip_dir) {
        Ok(Some(revision)) => {
            manifest.scip_revision = Some(revision.clone());
            info!(project_id = %project_id, sidecar_generation = %generation, %revision, "SCIP graph artifacts emitted from store");
            Ok(())
        }
        Ok(None) => {
            bail!("mandatory SCIP graph artifacts unavailable for {project_id}");
        }
        Err(error) => {
            bail!("mandatory SCIP graph artifact emit failed for {project_id}: {error}");
        }
    }
}

fn update_precise_semantic_import_status(
    scip_dir: &Path,
    manifest: &mut RetrievalIndexManifest,
) -> Result<bool> {
    let Some(artifact) = std::env::var_os("CODESTORY_PRECISE_SEMANTIC_SCIP_ARTIFACT") else {
        return Ok(false);
    };
    let status = import_precise_semantic_scip_artifact(
        Path::new(&artifact),
        &scip_dir.join(SCIP_PRECISE_SEMANTIC_IMPORT_DIR),
    )?;
    manifest.precise_semantic_import_status = Some(status.status);
    manifest.precise_semantic_import_reason = status.reason;
    manifest.precise_semantic_import_revision = status.revision;
    manifest.precise_semantic_import_producer = status.producer;
    Ok(true)
}

#[allow(dead_code)] // Phase 2 cache keys
pub fn query_fingerprint(query: &str) -> String {
    let digest = Sha256::digest(query.as_bytes());
    format!("{:x}", digest)[..16].to_string()
}

fn manifest_matches_sidecar_input(
    manifest: &RetrievalIndexManifest,
    sidecar_input: &SidecarInputFingerprint,
    embedding_backend: &str,
    embedding_dim: i32,
) -> bool {
    if !manifest_has_current_sidecar_contract(&manifest.project_id, manifest) {
        return false;
    }
    manifest.sidecar_schema_version == Some(SIDECAR_SCHEMA_VERSION)
        && manifest.sidecar_input_hash.as_deref() == Some(sidecar_input.hash.as_str())
        && manifest.projection_count == Some(sidecar_input.projection_count)
        && manifest.symbol_doc_count == Some(sidecar_input.symbol_doc_count)
        && manifest.dense_projection_count == Some(sidecar_input.dense_projection_count)
        && manifest.semantic_policy_version == sidecar_input.semantic_policy_version
        && manifest.graph_artifact_hash.as_deref()
            == Some(sidecar_input.graph_artifact_hash.as_str())
        && manifest.dense_reason_counts_json.as_deref()
            == Some(sidecar_input.dense_reason_counts_json.as_str())
        && manifest.embedding_backend.as_deref() == Some(embedding_backend)
        && manifest.embedding_dim == Some(embedding_dim)
}

fn retrieval_manifest_for_sidecar(
    project_id: &str,
    generation: &str,
    collection: &str,
    embedding_backend: &str,
    embedding_dim: i32,
    sidecar_input: &SidecarInputFingerprint,
) -> RetrievalIndexManifest {
    RetrievalIndexManifest {
        project_id: project_id.to_string(),
        lexical_version: LEXICAL_INDEX_VERSION.to_string(),
        semantic_generation: collection.to_string(),
        scip_revision: None,
        built_at_epoch_ms: Utc::now().timestamp_millis(),
        disk_bytes: None,
        degraded_modes_json: "[]".into(),
        embedding_backend: Some(embedding_backend.to_string()),
        embedding_dim: Some(embedding_dim),
        sidecar_schema_version: Some(SIDECAR_SCHEMA_VERSION),
        sidecar_input_hash: Some(sidecar_input.hash.clone()),
        sidecar_generation: Some(generation.to_string()),
        projection_count: Some(sidecar_input.projection_count),
        symbol_doc_count: Some(sidecar_input.symbol_doc_count),
        dense_projection_count: Some(sidecar_input.dense_projection_count),
        semantic_policy_version: sidecar_input.semantic_policy_version.clone(),
        graph_artifact_hash: Some(sidecar_input.graph_artifact_hash.clone()),
        dense_reason_counts_json: Some(sidecar_input.dense_reason_counts_json.clone()),
        precise_semantic_import_status: None,
        precise_semantic_import_reason: None,
        precise_semantic_import_revision: None,
        precise_semantic_import_producer: None,
    }
}

fn read_scip_revision(scip_dir: &Path) -> Option<String> {
    std::fs::read_to_string(scip_dir.join("revision.txt"))
        .ok()
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

fn sidecar_disk_bytes(
    layout: &SidecarLayout,
    generation: &str,
    collection: &str,
    scip_dir: &Path,
) -> Option<i64> {
    Some(
        dir_size_bytes(&crate::lexical_index::shard_dir_for(
            &layout.lexical_data_dir,
            generation,
        ))
        .saturating_add(dir_size_bytes(
            &layout
                .semantic_data_dir
                .join("collections")
                .join(collection),
        ))
        .saturating_add(dir_size_bytes(scip_dir)) as i64,
    )
}

fn semantic_ready_point_count(semantic: &SemanticGeneration<'_>) -> Option<u64> {
    let expected = u64::try_from(semantic.expected_points).ok()?;
    let health = EmbeddedVectorIndex::health(
        semantic.layout,
        semantic.collection,
        semantic.generation,
        semantic.input_hash,
        expected,
        semantic.embedding_backend,
        usize::try_from(semantic.embedding_dim).ok()?,
    );
    health.ready.then_some(health.point_count)
}

fn with_embedding_publication_residency<T>(
    residency: &crate::embeddings::ProductEmbeddingResidencyLease,
    publish: impl FnOnce() -> Result<T>,
) -> Result<T> {
    if residency.identity().is_none() && !cfg!(feature = "test-support") {
        bail!("embedding publication fence is missing its residency lease identity");
    }
    publish()
}

#[allow(clippy::too_many_arguments)]
fn persist_finalized_manifest(
    project_root: &Path,
    storage_path: &Path,
    retention_context: &GenerationRetentionContext<'_>,
    cancelled: &AtomicBool,
    sidecar_input: &SidecarInputFingerprint,
    project_id: String,
    mut manifest: RetrievalIndexManifest,
    degraded_modes: Vec<String>,
    stub_flags: SidecarStubFlags,
) -> Result<FinalizeIndexOutcome> {
    manifest.built_at_epoch_ms = Utc::now().timestamp_millis();
    manifest.degraded_modes_json =
        serde_json::to_string(&degraded_modes).unwrap_or_else(|_| "[]".into());
    let mut storage = Store::open(storage_path).context("open storage for retrieval manifest")?;
    let embedding_backend =
        crate::embeddings::embedding_runtime_id_for_runtime(retention_context.runtime);
    let embedding_dim = i32::try_from(crate::embeddings::semantic_vector_dim())
        .unwrap_or(crate::embeddings::RETRIEVAL_EMBEDDING_DIM as i32);
    #[cfg(not(feature = "test-support"))]
    let mut publication_qualification = PublicationQualificationHook::from_environment()?;
    let prepared_retention_result = with_embedding_publication_residency(
        &retention_context.embedding_residency,
        || {
            promote_retrieval_manifest_with_cancel(
                &mut storage,
                sidecar_input,
                &manifest,
                |storage| {
                    let lexical_source = lexical_source_input(project_root)
                        .context("rescan lexical source at publication fence")?;
                    let embedding_contract = SidecarEmbeddingContract {
                        backend: &embedding_backend,
                        dimension: embedding_dim,
                        producer_compatibility_identity: &retention_context
                            .producer_compatibility_identity,
                    };
                    let current_input = compute_sidecar_input_fingerprint_with_lexical_source(
                        storage,
                        project_root,
                        &project_id,
                        &embedding_contract,
                        lexical_source,
                    )?;
                    if let Some(reason) = manifest_unavailable_reason_for_runtime(
                        &project_id,
                        storage,
                        &manifest,
                        retention_context.runtime,
                    ) {
                        bail!(
                            "mandatory retrieval sidecar manifest would be unavailable immediately for {project_id}: {reason}"
                        );
                    }
                    Ok(current_input)
                },
                |storage| {
                    validate_candidate_generation(
                        &project_id,
                        sidecar_input,
                        &manifest,
                        retention_context,
                        storage,
                    )
                },
                |storage| {
                    prepare_generation_retention(retention_context, &project_id, &manifest, storage)
                },
                |prepared| Ok(prepared.verified_previous.clone()),
                || {
                    ensure_retrieval_index_not_cancelled(
                        cancelled,
                        "retrieval publication commit",
                    )?;
                    #[cfg(not(feature = "test-support"))]
                    {
                        if let Some(hook) = publication_qualification.as_mut() {
                            hook.pause_before_lease_revalidation()?;
                        }
                        let lease_identity =
                            match retention_context.embedding_residency.revalidate() {
                                Ok(identity) => identity,
                                Err(error) => {
                                    if let Some(hook) = publication_qualification.as_mut() {
                                        hook.record("lease_revalidation", "failed")?;
                                    }
                                    return Err(error).context(
                                        "revalidate embedding server lease before publication",
                                    );
                                }
                            };
                        let lease_matches = embedding_identity_matches(
                            retention_context
                                .embedding_residency
                                .identity()
                                .context("embedding publication fence is missing its identity")?,
                            &lease_identity,
                        );
                        if let Some(hook) = publication_qualification.as_mut() {
                            hook.record(
                                "lease_revalidation",
                                if lease_matches { "matched" } else { "changed" },
                            )?;
                        }
                        if !lease_matches {
                            bail!(
                                "embedding engine load generation changed before manifest publication"
                            );
                        }
                    }
                    Ok(())
                },
            )
        },
    );
    match prepared_retention_result {
        Ok(_prepared_retention) =>
        {
            #[cfg(not(feature = "test-support"))]
            if let Some(hook) = publication_qualification.as_mut() {
                hook.record("manifest_commit", "committed")?;
            }
        }
        Err(error) => {
            #[cfg(not(feature = "test-support"))]
            if let Some(hook) = publication_qualification.as_mut() {
                hook.record("manifest_commit", "returned_error")?;
            }
            return Err(error);
        }
    }

    let marker_error = match publish_derived_retention_marker(
        &storage,
        retention_context.layout,
        retention_context.workspace_id,
        &project_id,
    ) {
        Ok(()) => None,
        Err(error) => {
            let error = format!("publish derived generation retention marker: {error:#}");
            warn!(project_id = %project_id, error = %error, "retention marker update failed after SQLite publication");
            Some(error)
        }
    };

    let (generation_retention_plan, generation_retention) =
        retain_published_generations(storage_path, retention_context, &project_id, marker_error)?;

    info!(
        project_id = %project_id,
        lexical_version = %manifest.lexical_version,
        semantic_generation = %manifest.semantic_generation,
        sidecar_generation = ?manifest.sidecar_generation,
        degraded_modes = ?degraded_modes,
        "retrieval index manifest persisted"
    );

    Ok(FinalizeIndexOutcome {
        project_id,
        manifest,
        degraded_modes,
        scip_stubbed: stub_flags.scip_stubbed,
        generation_retention_plan,
        generation_retention,
    })
}

fn ensure_sidecar_input_unchanged(
    expected: &SidecarInputFingerprint,
    current: &SidecarInputFingerprint,
) -> Result<()> {
    if current != expected {
        return Err(SidecarInputChanged::new(
            "manifest publication",
            &expected.hash,
            &current.hash,
        )
        .into());
    }
    Ok(())
}

#[cfg(test)]
fn promote_retrieval_manifest<T>(
    storage: &mut Store,
    expected: &SidecarInputFingerprint,
    manifest: &RetrievalIndexManifest,
    current_input: impl FnOnce(&Store) -> Result<SidecarInputFingerprint>,
    validate_candidate: impl FnOnce(&Store) -> Result<()>,
    prepare_publication: impl FnOnce(&Store) -> Result<T>,
    publication_rollback: impl FnOnce(&T) -> Result<Option<RetrievalIndexRollbackRecord>>,
) -> Result<T> {
    promote_retrieval_manifest_with_cancel(
        storage,
        expected,
        manifest,
        current_input,
        validate_candidate,
        prepare_publication,
        publication_rollback,
        || Ok(()),
    )
}

#[allow(clippy::too_many_arguments)]
fn promote_retrieval_manifest_with_cancel<T>(
    storage: &mut Store,
    expected: &SidecarInputFingerprint,
    manifest: &RetrievalIndexManifest,
    current_input: impl FnOnce(&Store) -> Result<SidecarInputFingerprint>,
    validate_candidate: impl FnOnce(&Store) -> Result<()>,
    prepare_publication: impl FnOnce(&Store) -> Result<T>,
    publication_rollback: impl FnOnce(&T) -> Result<Option<RetrievalIndexRollbackRecord>>,
    mut ensure_not_cancelled: impl FnMut() -> Result<()>,
) -> Result<T> {
    ensure_not_cancelled()?;
    validate_candidate(storage)?;
    let prepared = prepare_publication(storage)?;
    let mut publication = storage
        .write_transaction()
        .context("lock sidecar input and manifest publication")?;
    let current = current_input(publication.storage())?;
    ensure_sidecar_input_unchanged(expected, &current)?;
    let rollback = publication_rollback(&prepared)?;
    ensure_not_cancelled()?;
    publication
        .storage_mut()
        .publish_retrieval_index_publication(manifest, rollback.as_ref())
        .context("persist atomic retrieval current and rollback pointers")?;
    ensure_not_cancelled()?;
    publication
        .finish()
        .context("commit retrieval manifest publication")?;
    Ok(prepared)
}

fn publish_derived_retention_marker(
    storage: &Store,
    layout: &SidecarLayout,
    workspace_id: &str,
    project_id: &str,
) -> Result<()> {
    let (active, rollback) = storage
        .get_retrieval_index_publication(project_id)
        .context("read committed retrieval publication for retention marker")?
        .context("committed retrieval publication is missing")?;
    let marker = GenerationRetentionMarker::next(
        workspace_id,
        active,
        rollback,
        Utc::now().timestamp_millis(),
    )
    .context("derive generation retention marker from SQLite publication")?;
    write_retention_marker(&layout.state_file, &marker)
        .context("write derived generation retention marker")?;
    Ok(())
}

#[derive(Clone)]
struct CandidateGenerationEvidence {
    lexical_matches: bool,
    scip_revision: Option<String>,
    scip_graph: bool,
    semantic_points: Option<u64>,
    semantic_ready: bool,
    semantic_zero_dense_policy: bool,
    embedding_device: crate::embeddings::EmbeddingDeviceReadiness,
    embedding_accelerator_smoke_elapsed_ms: Option<u64>,
    embedding_identity_before: crate::embedding_server_compat::ProductEmbeddingIdentity,
    embedding_identity_after: crate::embedding_server_compat::ProductEmbeddingIdentity,
    retrieval_mode: String,
    degraded_reason: Option<String>,
}

fn validate_candidate_generation_evidence(
    project_id: &str,
    sidecar_input: &SidecarInputFingerprint,
    manifest: &RetrievalIndexManifest,
    runtime: &SidecarRuntimeConfig,
    evidence: &CandidateGenerationEvidence,
) -> Result<()> {
    let expected_embedding_backend = crate::embeddings::embedding_runtime_id_for_runtime(runtime);
    let expected_embedding_dim = i32::try_from(crate::embeddings::semantic_vector_dim())
        .unwrap_or(crate::embeddings::RETRIEVAL_EMBEDDING_DIM as i32);
    if !manifest_matches_sidecar_input(
        manifest,
        sidecar_input,
        &expected_embedding_backend,
        expected_embedding_dim,
    ) {
        bail!("mandatory candidate manifest does not match its sidecar generation input");
    }
    if !evidence.lexical_matches {
        bail!("mandatory candidate generation component failed validation: lexical");
    }
    if manifest.scip_revision.is_none()
        || manifest.scip_revision != evidence.scip_revision
        || !evidence.scip_graph
    {
        bail!("mandatory candidate generation component failed validation: scip");
    }
    let zero_dense_candidate = sidecar_input.projection_count == 0
        && sidecar_input.dense_projection_count == 0
        && sidecar_input.semantic_policy_version.as_deref()
            == Some(crate::generation::SEMANTIC_POLICY_VERSION);
    let semantic_valid = if zero_dense_candidate {
        evidence.semantic_points == Some(0)
            && evidence.semantic_ready
            && evidence.semantic_zero_dense_policy
    } else {
        evidence.semantic_points.is_some()
            && evidence.semantic_ready
            && !evidence.semantic_zero_dense_policy
    };
    if !semantic_valid {
        bail!("mandatory candidate generation component failed validation: semantic");
    }
    if !crate::embeddings::manifest_embedding_backend_is_product(
        manifest.embedding_backend.as_deref(),
    ) || !embedding_identity_matches(
        &evidence.embedding_identity_before,
        &evidence.embedding_identity_after,
    ) {
        bail!("mandatory candidate generation component failed validation: embedding_runtime");
    }
    let runtime_cpu_allowed = runtime.embedding.allow_cpu;
    let device_policy_valid = if evidence.embedding_device.cpu_allowed && runtime_cpu_allowed {
        evidence.embedding_device.full_retrieval_allowed
            && evidence.embedding_device.observed_state == "cpu_explicit"
            && evidence.embedding_device.observation_source == "per_user_server"
            && evidence.embedding_accelerator_smoke_elapsed_ms.is_none()
    } else if !evidence.embedding_device.cpu_allowed && !runtime_cpu_allowed {
        evidence.embedding_accelerator_smoke_elapsed_ms.is_some()
            && evidence.embedding_device.accelerator_requested
            && evidence.embedding_device.observed_state == "accelerated"
            && evidence.embedding_device.observation_source == "per_user_server"
    } else {
        false
    };
    if !device_policy_valid {
        bail!("mandatory candidate generation component failed validation: accelerator_proof");
    }
    if evidence.retrieval_mode != "full" {
        bail!(
            "mandatory candidate generation did not reach full mode for {project_id}: {} {:?}",
            evidence.retrieval_mode,
            evidence.degraded_reason
        );
    }
    Ok(())
}

fn embedding_identity_matches(
    before: &crate::embedding_server_compat::ProductEmbeddingIdentity,
    after: &crate::embedding_server_compat::ProductEmbeddingIdentity,
) -> bool {
    before.instance_id == after.instance_id
        && before.load_generation == after.load_generation
        && before.model_load_count == after.model_load_count
        && before.residency == "resident"
        && after.residency == "resident"
        && before.worker_alive
        && after.worker_alive
        && before.load_error.is_none()
        && after.load_error.is_none()
        && before.model_digest == after.model_digest
        && before.ggml_build_identity == after.ggml_build_identity
        && before.backend == after.backend
        && before.adapter_name == after.adapter_name
        && before.policy == after.policy
        && before.accelerator_execution_verified == after.accelerator_execution_verified
}

fn embedding_identity_matches_lease(
    lease: &crate::embedding_server_compat::ProductEmbeddingIdentity,
    before: &crate::embedding_server_compat::ProductEmbeddingIdentity,
    after: &crate::embedding_server_compat::ProductEmbeddingIdentity,
) -> bool {
    embedding_identity_matches(lease, before) && embedding_identity_matches(before, after)
}

fn validate_candidate_generation(
    project_id: &str,
    sidecar_input: &SidecarInputFingerprint,
    manifest: &RetrievalIndexManifest,
    context: &GenerationRetentionContext<'_>,
    storage: &Store,
) -> Result<()> {
    let generation = manifest
        .sidecar_generation
        .as_deref()
        .context("mandatory sidecar manifest is missing its generation")?;
    let scip_dir = context.layout.scip_project_dir(generation);
    let embedding_identity_before =
        crate::embedding_server_compat::product_embedding_identity(context.runtime)
            .context("validate managed per-user embedding server identity before final probes")?;
    let semantic_generation = SemanticGeneration {
        layout: context.layout,
        collection: &manifest.semantic_generation,
        generation,
        input_hash: &sidecar_input.hash,
        embedding_backend: manifest.embedding_backend.as_deref().unwrap_or_default(),
        embedding_dim: manifest.embedding_dim.unwrap_or_default(),
        expected_points: sidecar_input.projection_count,
    };
    let semantic_points = semantic_ready_point_count(&semantic_generation);
    let embedding_accelerator_smoke =
        crate::embeddings::ensure_embedding_accelerator_smoke_for_runtime(context.runtime)
            .context("validate candidate embedding accelerator with a fresh timed smoke")?;
    let embedding_device = embedding_accelerator_smoke
        .as_ref()
        .map(|smoke| smoke.device.clone())
        .unwrap_or_else(|| {
            crate::embeddings::embedding_device_readiness_for_runtime(context.runtime)
        });
    let status = probe_sidecar_health_for_runtime(
        context.layout,
        project_id,
        Some(manifest.clone()),
        &embedding_device,
        context.runtime,
    );
    let embedding_identity_after =
        crate::embedding_server_compat::product_embedding_identity(context.runtime)
            .context("validate managed per-user embedding server identity after final probes")?;
    if let Some(lease_identity) = context.embedding_residency.identity() {
        if !embedding_identity_matches_lease(
            lease_identity,
            &embedding_identity_before,
            &embedding_identity_after,
        ) {
            bail!("embedding engine load generation changed inside the publication fence");
        }
    } else if !cfg!(feature = "test-support") {
        bail!("embedding publication fence is missing its residency lease identity");
    }
    let evidence = CandidateGenerationEvidence {
        lexical_matches: crate::lexical_index::shard_matches_lexical_input(
            &context.layout.lexical_data_dir,
            generation,
            sidecar_input.lexical_file_count,
            &sidecar_input.lexical_hash,
            &sidecar_input.hash,
        ),
        scip_revision: read_scip_revision(&scip_dir),
        scip_graph: status.scip.capabilities.graph,
        semantic_points,
        semantic_ready: status.semantic.capabilities.semantic,
        semantic_zero_dense_policy: sidecar_input.projection_count == 0
            && sidecar_input.dense_projection_count == 0
            && status.semantic.status == crate::health::ComponentStatus::Healthy
            && status.semantic.degraded_reason.is_none()
            && status.semantic.capabilities.semantic,
        embedding_device,
        embedding_accelerator_smoke_elapsed_ms: embedding_accelerator_smoke
            .map(|smoke| smoke.elapsed_ms),
        embedding_identity_before,
        embedding_identity_after,
        retrieval_mode: status.retrieval_mode,
        degraded_reason: status.degraded_reason,
    };
    validate_candidate_generation_evidence(
        project_id,
        sidecar_input,
        manifest,
        context.runtime,
        &evidence,
    )?;
    let core_publication = storage
        .get_complete_index_publication()
        .context("load complete core publication at retrieval publication fence")?
        .context("retrieval publication requires a complete core publication")?;
    crate::embedded_vector::validate_generation_evidence_for_publication(
        context.layout,
        storage,
        manifest,
        &core_publication,
        context.runtime,
        &evidence.embedding_device,
        Some(&evidence.embedding_identity_after),
    )
    .context("deep-validate vector generation at retrieval publication fence")?;
    Ok(())
}

fn prepare_generation_retention(
    context: &GenerationRetentionContext<'_>,
    project_id: &str,
    active: &RetrievalIndexManifest,
    storage: &Store,
) -> Result<PreparedGenerationRetention> {
    let now = Utc::now().timestamp_millis();
    let active_generation = active.sidecar_generation.as_deref();
    let mut candidates = Vec::new();
    if let Some(previous) = context.previous_manifest {
        candidates.push(previous.clone());
    }
    match storage.get_retrieval_index_publication(project_id) {
        Ok(Some((_, Some(rollback)))) => candidates.push(rollback.manifest),
        Ok(_) => {}
        Err(error) => warn!(
            project_id = %project_id,
            error = %error,
            "stored rollback pointer is unreadable and will not be retained as authoritative"
        ),
    }
    candidates.retain(|candidate| {
        candidate.project_id == project_id
            && candidate.sidecar_generation.as_deref() != active_generation
            && manifest_has_current_sidecar_contract(project_id, candidate)
    });
    candidates.sort_by_key(|candidate| candidate.built_at_epoch_ms);
    candidates.dedup_by(|left, right| left.sidecar_generation == right.sidecar_generation);

    let publication = storage
        .get_complete_index_publication()
        .context("load complete core publication for rollback validation")?;
    let verified_previous = publication.and_then(|publication| {
        candidates.into_iter().rev().find_map(|candidate| {
            let validation = crate::embedded_vector::validate_generation_evidence_for_publication(
                context.layout,
                storage,
                &candidate,
                &publication,
                context.runtime,
                context.embedding_device,
                context.embedding_residency.identity(),
            )
            .context("validate rollback vector bytes, producer evidence, and anchor coverage")
            .and_then(|_| {
                let status = probe_sidecar_health_for_runtime(
                    context.layout,
                    project_id,
                    Some(candidate.clone()),
                    context.embedding_device,
                    context.runtime,
                );
                if status.retrieval_mode != "full" {
                    bail!(
                        "rollback generation is not full: {} {:?}",
                        status.retrieval_mode,
                        status.degraded_reason
                    );
                }
                Ok(())
            });
            match validation {
                Ok(()) => Some(RetrievalIndexRollbackRecord {
                    manifest: candidate,
                    verified_at_epoch_ms: now,
                }),
                Err(error) => {
                    warn!(
                        project_id = %project_id,
                        sidecar_generation = ?candidate.sidecar_generation,
                        error = %format!("{error:#}"),
                        "retrieval rollback candidate failed deep validation"
                    );
                    None
                }
            }
        })
    });

    Ok(PreparedGenerationRetention { verified_previous })
}

fn retain_published_generations(
    storage_path: &Path,
    context: &GenerationRetentionContext<'_>,
    project_id: &str,
    marker_error: Option<String>,
) -> Result<(GenerationRetentionPlan, GenerationRetentionApplyReport)> {
    let mut protection = scan_retention_protection(
        &crate::config::user_cache_root(),
        Some(storage_path),
        &context.layout.state_file,
    );
    if let Some(error) = marker_error {
        protection.errors.push(error);
    }
    let plan = plan_generation_retention(context.layout, project_id, &protection);
    let mut remover = FsGenerationRemover::new(context.layout)?;
    let apply = apply_generation_retention(&plan, &mut remover);
    Ok((plan, apply))
}

pub(crate) fn compute_sidecar_input_fingerprint(
    storage: &Store,
    project_root: &Path,
    project_id: &str,
    embedding_backend: &str,
    embedding_dim: i32,
    producer_compatibility_identity: &str,
) -> Result<SidecarInputFingerprint> {
    let lexical_source = lexical_source_input(project_root).context("hash lexical source input")?;
    let embedding_contract = SidecarEmbeddingContract {
        backend: embedding_backend,
        dimension: embedding_dim,
        producer_compatibility_identity,
    };
    compute_sidecar_input_fingerprint_with_lexical_source(
        storage,
        project_root,
        project_id,
        &embedding_contract,
        lexical_source,
    )
}

fn compute_sidecar_input_fingerprint_with_lexical_source(
    storage: &Store,
    project_root: &Path,
    project_id: &str,
    embedding: &SidecarEmbeddingContract<'_>,
    lexical_source: crate::lexical_index::LexicalSourceInput,
) -> Result<SidecarInputFingerprint> {
    let lexical = finish_lexical_input_for_store(lexical_source, project_root, storage)
        .context("hash lexical symbol input")?;
    let mut hasher = Sha256::new();
    let mut graph_hasher = Sha256::new();
    hash_part(&mut hasher, "codestory-sidecar-input-v10");
    hash_part(&mut graph_hasher, "codestory-symbol-search-docs-v1");
    hash_part(&mut hasher, project_id);
    let core_publication = storage
        .get_complete_index_publication()
        .context("load complete core publication for sidecar hash")?;
    hash_part(
        &mut hasher,
        core_publication
            .as_ref()
            .map_or("<missing>", |publication| {
                publication.generation_id.as_str()
            }),
    );
    hash_part(
        &mut hasher,
        core_publication
            .as_ref()
            .map_or("<missing>", |publication| publication.run_id.as_str()),
    );
    hash_part(&mut hasher, &SIDECAR_SCHEMA_VERSION.to_string());
    hash_part(&mut hasher, LEXICAL_INDEX_VERSION);
    hash_part(&mut hasher, &lexical.file_count.to_string());
    hash_part(&mut hasher, &lexical.hash);
    hash_part(&mut hasher, embedding.backend);
    hash_part(&mut hasher, &embedding.dimension.to_string());
    hash_part(&mut hasher, embedding.producer_compatibility_identity);
    hash_part(&mut hasher, "dense-anchor-inputs-v1");
    hash_part(&mut hasher, "scip-symbols-json-v1");

    let mut symbol_doc_count = 0_i64;
    let mut policy_versions = BTreeSet::<String>::new();
    let mut after_symbol_doc = None;
    loop {
        let batch = storage
            .get_symbol_search_docs_batch_after(after_symbol_doc, SIDECAR_INPUT_BATCH_SIZE)
            .context("load symbol search docs for sidecar hash")?;
        if batch.is_empty() {
            break;
        }
        after_symbol_doc = batch.last().map(|doc| doc.node_id);
        symbol_doc_count += i64::try_from(batch.len()).unwrap_or(i64::MAX);
        for doc in batch {
            observe_policy_version(&mut policy_versions, Some(doc.policy_version.as_str()));
            hash_symbol_search_doc_detail(&mut graph_hasher, project_root, &doc);
        }
    }
    let graph_artifact_hash = format!("{:x}", graph_hasher.finalize());
    hash_part(&mut hasher, &symbol_doc_count.to_string());
    hash_part(&mut hasher, &graph_artifact_hash);

    let mut dense_projection_count = 0_i64;
    let mut dense_reason_counts = BTreeMap::<String, i64>::new();
    let mut after = None;
    loop {
        let batch = storage
            .get_dense_anchor_inputs_batch_after(after, SIDECAR_INPUT_BATCH_SIZE)
            .context("load dense anchor inputs for sidecar hash")?;
        if batch.is_empty() {
            break;
        }
        after = batch.last().map(|doc| doc.node_id);
        dense_projection_count += i64::try_from(batch.len()).unwrap_or(i64::MAX);
        for doc in batch {
            observe_policy_version(&mut policy_versions, Some(doc.policy_version.as_str()));
            let reason = doc.selection_reason.clone();
            *dense_reason_counts.entry(reason).or_insert(0) += 1;
            hash_dense_anchor_input(&mut hasher, project_root, &doc);
        }
    }
    let dense_reason_counts_json =
        serde_json::to_string(&dense_reason_counts).unwrap_or_else(|_| "{}".into());
    let semantic_policy_version = policy_version_from_observed(&policy_versions)
        .or_else(|| Some(crate::generation::SEMANTIC_POLICY_VERSION.into()));
    hash_part(
        &mut hasher,
        semantic_policy_version.as_deref().unwrap_or("<missing>"),
    );
    hash_part(&mut hasher, &dense_projection_count.to_string());
    hash_part(&mut hasher, &dense_reason_counts_json);

    Ok(SidecarInputFingerprint {
        hash: format!("{:x}", hasher.finalize()),
        symbol_doc_count,
        projection_count: dense_projection_count,
        dense_projection_count,
        semantic_policy_version,
        graph_artifact_hash,
        dense_reason_counts_json,
        lexical_file_count: lexical.file_count,
        lexical_hash: lexical.hash,
        lexical_coverage: lexical.coverage,
    })
}

fn observe_policy_version(policy_versions: &mut BTreeSet<String>, policy: Option<&str>) {
    if let Some(policy) = policy.map(str::trim).filter(|policy| !policy.is_empty()) {
        policy_versions.insert(policy.to_string());
    }
}

fn policy_version_from_observed(policy_versions: &BTreeSet<String>) -> Option<String> {
    match policy_versions.len() {
        0 => None,
        1 => policy_versions.iter().next().cloned(),
        _ => Some("mixed".into()),
    }
}

fn hash_symbol_search_doc_detail(hasher: &mut Sha256, project_root: &Path, doc: &SymbolSearchDoc) {
    let file_path = doc
        .file_path
        .as_deref()
        .and_then(|path| normalize_sidecar_file_path(path, project_root).ok())
        .unwrap_or_default();
    let file_role = if file_path.is_empty() {
        ""
    } else {
        FileRole::classify_path(Path::new(&file_path)).as_str()
    };
    hash_part(hasher, &doc.node_id.0.to_string());
    hash_part(
        hasher,
        &doc.file_node_id
            .map(|node_id| node_id.0.to_string())
            .unwrap_or_default(),
    );
    hash_part(hasher, &(doc.kind as i32).to_string());
    hash_part(hasher, &doc.display_name);
    hash_part(hasher, doc.qualified_name.as_deref().unwrap_or(""));
    hash_part(hasher, &file_path);
    hash_part(hasher, file_role);
    hash_part(
        hasher,
        &doc.start_line
            .map(|line| line.to_string())
            .unwrap_or_default(),
    );
    hash_part(hasher, &doc.doc_version.to_string());
    hash_part(hasher, &doc.doc_hash);
    hash_part(hasher, &doc.policy_version);
    hash_part(hasher, &doc.source_provenance);
}

fn hash_dense_anchor_input(hasher: &mut Sha256, project_root: &Path, doc: &DenseAnchorInput) {
    let file_path = doc
        .file_path
        .as_deref()
        .and_then(|path| normalize_sidecar_file_path(path, project_root).ok())
        .unwrap_or_default();
    let file_role = if file_path.is_empty() {
        ""
    } else {
        FileRole::classify_path(Path::new(&file_path)).as_str()
    };
    hash_part(hasher, &doc.node_id.0.to_string());
    hash_part(hasher, &(doc.kind as i32).to_string());
    hash_part(hasher, &doc.display_name);
    hash_part(hasher, doc.qualified_name.as_deref().unwrap_or(""));
    hash_part(hasher, &file_path);
    hash_part(hasher, file_role);
    hash_part(
        hasher,
        &doc.start_line
            .map(|line| line.to_string())
            .unwrap_or_default(),
    );
    hash_part(
        hasher,
        &doc.end_line
            .map(|line| line.to_string())
            .unwrap_or_default(),
    );
    hash_part(hasher, doc.file_role.as_str());
    hash_part(hasher, &doc.source_provenance);
    hash_part(hasher, &doc.text);
    hash_part(hasher, &doc.document_hash);
    hash_part(hasher, &doc.selection_reason);
    hash_part(hasher, &doc.policy_version);
}

#[cfg(test)]
fn semantic_projection_row(row: &codestory_store::SearchSymbolProjectionDetail) -> bool {
    let Some(kind) = row
        .node_kind
        .and_then(|kind| i32::try_from(kind).ok())
        .and_then(|kind| codestory_contracts::graph::NodeKind::try_from(kind).ok())
    else {
        return false;
    };
    crate::generation::sidecar_semantic_node_kind(kind)
}

fn hash_part(hasher: &mut Sha256, value: &str) {
    hasher.update(value.len().to_le_bytes());
    hasher.update(value.as_bytes());
}

fn normalize_sidecar_file_path(path: &str, project_root: &Path) -> Result<String> {
    let path = Path::new(path);
    if path.is_absolute() {
        path.strip_prefix(project_root)
            .with_context(|| format!("strip project root from {}", path.display()))
            .map(|rel| rel.to_string_lossy().replace('\\', "/"))
    } else {
        Ok(path.to_string_lossy().replace('\\', "/"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::retention::read_retention_marker;
    use codestory_contracts::graph::{Node, NodeId, NodeKind};
    use codestory_store::{SearchSymbolProjection, SearchSymbolProjectionDetail};
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn secure_test_directory(path: &Path) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o700))
                .expect("secure qualification directory");
        }
    }

    fn write_private_control(path: &Path, value: &serde_json::Value) {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let mut file = options.open(path).expect("create private control");
        serde_json::to_writer(&mut file, value).expect("write private control");
        file.write_all(b"\n").expect("terminate private control");
        file.sync_all().expect("sync private control");
    }

    #[test]
    fn publication_qualification_hook_is_absent_when_gate_is_closed() {
        assert!(
            PublicationQualificationHook::from_environment_values(None, None)
                .expect("closed environment gate")
                .is_none()
        );
    }

    #[test]
    fn publication_qualification_hook_is_absent_without_a_pause_control() {
        let directory = TempDir::new().expect("qualification directory");
        secure_test_directory(directory.path());
        assert!(
            PublicationQualificationHook::from_gate(directory.path(), "test-nonce")
                .expect("closed hook")
                .is_none()
        );
    }

    #[test]
    fn publication_qualification_hook_emits_only_correlated_raw_events() {
        let directory = TempDir::new().expect("qualification directory");
        secure_test_directory(directory.path());
        let nonce = "qualification-secret";
        let nonce_sha256 = hex_sha256(nonce.as_bytes());
        let correlation_id = "0123456789abcdef0123456789abcdef";
        let pause_path = directory
            .path()
            .join(format!("publication-pause-{nonce_sha256}.json"));
        write_private_control(
            &pause_path,
            &serde_json::json!({
                "schema_version": 1,
                "nonce_sha256": nonce_sha256,
                "correlation_id": correlation_id,
                "action": "pause_before_manifest_commit"
            }),
        );
        let mut hook = PublicationQualificationHook::from_gate(directory.path(), nonce)
            .expect("open hook")
            .expect("pause control enables hook");
        let resume_path = directory
            .path()
            .join(format!("publication-resume-{correlation_id}.json"));
        let nonce_sha256_for_resume = nonce_sha256.clone();
        let correlation_for_resume = correlation_id.to_string();
        let resume = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            write_private_control(
                &resume_path,
                &serde_json::json!({
                    "schema_version": 1,
                    "nonce_sha256": nonce_sha256_for_resume,
                    "correlation_id": correlation_for_resume,
                    "action": "resume_manifest_commit"
                }),
            );
        });
        hook.pause_before_lease_revalidation()
            .expect("resume qualification hook");
        hook.record("lease_revalidation", "failed")
            .expect("record lease failure");
        hook.record("manifest_commit", "returned_error")
            .expect("record publication result");
        resume.join().expect("resume writer");
        drop(hook);

        let events_path = directory
            .path()
            .join(format!("publication-events-{correlation_id}.jsonl"));
        let raw = fs::read_to_string(events_path).expect("read raw event log");
        assert!(!raw.contains(nonce), "raw event log leaked the nonce");
        assert!(
            !raw.contains(directory.path().to_string_lossy().as_ref()),
            "raw event log leaked its private directory"
        );
        let events = raw
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).expect("parse event"))
            .collect::<Vec<_>>();
        assert_eq!(events.len(), 4);
        assert_eq!(events[0]["sequence"], 0);
        assert_eq!(events[0]["action"], "pause_before_manifest_commit");
        assert_eq!(events[0]["status"], "waiting_for_resume");
        assert_eq!(events[1]["sequence"], 1);
        assert_eq!(events[1]["action"], "resume_manifest_commit");
        assert_eq!(events[1]["status"], "observed");
        assert_eq!(events[2]["action"], "lease_revalidation");
        assert_eq!(events[2]["status"], "failed");
        assert_eq!(events[3]["action"], "manifest_commit");
        assert_eq!(events[3]["status"], "returned_error");
        assert!(
            events.iter().all(|event| {
                event["correlation_id"] == correlation_id
                    && event["clock"]["domain"] == "process_monotonic"
                    && event["clock"]["api"] == "std::time::Instant"
                    && event["clock"]["elapsed_ns"].is_u64()
            }),
            "raw events omitted their local monotonic clock or correlation"
        );
    }

    #[cfg(unix)]
    #[test]
    fn publication_qualification_hook_rejects_a_broad_directory() {
        use std::os::unix::fs::PermissionsExt;
        let directory = TempDir::new().expect("qualification directory");
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o755))
            .expect("broaden qualification directory");
        let error = PublicationQualificationHook::from_gate(directory.path(), "test-nonce")
            .err()
            .expect("broad directory must fail");
        assert!(
            error
                .to_string()
                .contains("embedding_publication_qualification_directory_untrusted")
        );
    }

    #[cfg(unix)]
    #[test]
    fn publication_qualification_hook_rejects_a_symlink_control() {
        use std::os::unix::fs::symlink;
        let directory = TempDir::new().expect("qualification directory");
        secure_test_directory(directory.path());
        let nonce = "qualification-secret";
        let nonce_sha256 = hex_sha256(nonce.as_bytes());
        let target = directory.path().join("target.json");
        write_private_control(
            &target,
            &serde_json::json!({
                "schema_version": 1,
                "nonce_sha256": nonce_sha256,
                "correlation_id": "0123456789abcdef0123456789abcdef",
                "action": "pause_before_manifest_commit"
            }),
        );
        let control = directory
            .path()
            .join(format!("publication-pause-{nonce_sha256}.json"));
        symlink(&target, &control).expect("create control symlink");
        let error = PublicationQualificationHook::from_gate(directory.path(), nonce)
            .err()
            .expect("symlink control must fail");
        assert!(
            error
                .to_string()
                .contains("embedding_publication_qualification_control_untrusted")
        );
    }

    fn test_embedding_identity(
        policy: &'static str,
    ) -> crate::embedding_server_compat::ProductEmbeddingIdentity {
        let accelerated = policy == "accelerated";
        crate::embedding_server_compat::ProductEmbeddingIdentity {
            instance_id: "inprocess:test".into(),
            load_generation: 1,
            model_load_count: 1,
            residency: "resident",
            worker_alive: true,
            load_error: None,
            model_digest: codestory_llama_sys::MODEL_SHA256,
            ggml_build_identity: codestory_llama_sys::GGML_BUILD_IDENTITY,
            backend: if accelerated { "Metal" } else { "CPU" }.into(),
            adapter_name: if accelerated {
                "test accelerator"
            } else {
                "CPU"
            }
            .into(),
            adapter_description: "test".into(),
            policy,
            embedded_model: true,
            materialized_path: PathBuf::from("model.gguf"),
            materialized_reused: true,
            initialization_ms: 1,
            smoke_ms: 1,
            adapter_memory_total: 1,
            adapter_memory_used_by_load: usize::from(accelerated),
            execution_device_names: if accelerated {
                vec!["test accelerator".into()]
            } else {
                Vec::new()
            },
            execution_backend_names: if accelerated {
                vec!["Metal".into()]
            } else {
                Vec::new()
            },
            execution_observation_source: "ggml_eval_callback",
            encode_count: 1,
            execution_node_count: u64::from(accelerated),
            resident_accelerator_tensor_count: u64::from(accelerated),
            resident_accelerator_tensor_bytes: u64::from(accelerated),
            model_layer_count: 13,
            offloaded_layer_count: if accelerated { 13 } else { 0 },
            accelerator_execution_verified: accelerated,
        }
    }

    #[test]
    fn finalize_index_fails_before_publication_without_runtime_or_artifacts() {
        let _env = crate::test_support::env_lock();
        let project = TempDir::new().expect("project dir");
        let storage_dir = TempDir::new().expect("storage dir");
        let storage_path = storage_dir.path().join("codestory.db");
        {
            let storage = Store::open(&storage_path).expect("open empty db");
            drop(storage);
        }
        let error = finalize_index(project.path(), &storage_path)
            .expect_err("empty stores cannot satisfy mandatory sidecar indexing");
        let message = error.to_string();
        assert!(
            message.contains("mandatory")
                || message.contains("embedding_device_unverified")
                || message.contains("without its embedded embedding model")
                || message.contains("complete core publication"),
            "expected a pre-publication retrieval trust-gate error, got {error:#}"
        );
    }

    #[test]
    fn pre_cancelled_finalize_stops_before_runtime_or_artifact_mutation() {
        let project = TempDir::new().expect("project dir");
        let storage_dir = TempDir::new().expect("storage dir");
        let storage_path = storage_dir.path().join("codestory.db");
        let runtime = SidecarRuntimeConfig::local();
        let cancelled = AtomicBool::new(true);

        let error = finalize_index_for_runtime_with_cancel(
            project.path(),
            &storage_path,
            &runtime,
            &cancelled,
        )
        .expect_err("pre-cancelled finalize must fail before opening storage");

        assert!(is_retrieval_index_cancelled(&error));
        assert!(error.to_string().contains("cancelled before preflight"));
        assert!(!storage_path.exists());
    }

    #[test]
    fn finalize_progress_is_emitted_before_blocking_work() {
        let phases = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let progress_phases = std::rc::Rc::clone(&phases);
        let action_phases = std::rc::Rc::clone(&phases);

        with_finalize_progress(
            &mut |phase| progress_phases.borrow_mut().push(phase),
            "lexical sidecar",
            || {
                assert_eq!(&*action_phases.borrow(), &["lexical sidecar"]);
                Ok(())
            },
        )
        .expect("progress wrapper should return action result");

        assert_eq!(&*phases.borrow(), &["lexical sidecar"]);
    }

    #[test]
    fn publication_requires_one_pinned_embedding_load_generation() {
        let before = test_embedding_identity("accelerated");
        let mut after = before.clone();
        assert!(embedding_identity_matches_lease(&before, &before, &after));

        after.load_generation += 1;
        after.model_load_count += 1;
        assert!(!embedding_identity_matches_lease(&before, &before, &after));

        after = before.clone();
        after.residency = "sleeping";
        assert!(!embedding_identity_matches_lease(&before, &before, &after));

        let mut lease = before.clone();
        lease.load_generation += 1;
        lease.model_load_count += 1;
        assert!(!embedding_identity_matches_lease(&lease, &before, &before));
    }

    #[test]
    fn publication_scope_retains_the_residency_guard_through_commit() {
        use std::sync::atomic::Ordering;

        let (residency, active) = crate::embeddings::ProductEmbeddingResidencyLease::test_lease(
            test_embedding_identity("accelerated"),
        );
        with_embedding_publication_residency(&residency, || {
            assert!(active.load(Ordering::Acquire));
            Ok(())
        })
        .expect("publication under residency guard");
        assert!(active.load(Ordering::Acquire));

        drop(residency);
        assert!(!active.load(Ordering::Acquire));
    }

    #[test]
    fn generated_manifest_helper_records_current_sidecar_contract() {
        let project_id = "proj";
        let input = SidecarInputFingerprint {
            hash: "0123456789abcdef0123456789abcdef".into(),
            symbol_doc_count: 42,
            projection_count: 42,
            dense_projection_count: 42,
            semantic_policy_version: Some(crate::generation::SEMANTIC_POLICY_VERSION.into()),
            graph_artifact_hash: "graph-hash".into(),
            dense_reason_counts_json: "{\"public_api\":42}".into(),
            lexical_file_count: 3,
            lexical_hash: "lexical".into(),
            lexical_coverage: Default::default(),
        };
        let generation = sidecar_generation_id(project_id, &input.hash);
        let collection = crate::generation::sidecar_vector_generation(project_id, &input.hash);

        let manifest = retrieval_manifest_for_sidecar(
            project_id,
            &generation,
            &collection,
            crate::embeddings::PRODUCT_EMBEDDING_RUNTIME_ID,
            768,
            &input,
        );

        assert!(manifest_has_current_sidecar_contract(project_id, &manifest));
        assert_eq!(manifest.projection_count, Some(42));
        assert_eq!(manifest.symbol_doc_count, Some(42));
        assert_eq!(manifest.dense_projection_count, Some(42));
        assert_eq!(
            manifest.semantic_policy_version.as_deref(),
            Some(crate::generation::SEMANTIC_POLICY_VERSION)
        );
        assert_eq!(
            manifest.sidecar_generation.as_deref(),
            Some(generation.as_str())
        );
        assert_eq!(manifest.semantic_generation, collection);
    }

    #[test]
    fn cancellation_before_sqlite_commit_preserves_current_and_rollback_pointers() {
        let storage_dir = TempDir::new().expect("storage dir");
        let mut storage = Store::open(storage_dir.path().join("codestory.db"))
            .expect("open retrieval publication store");
        let input = |hash: &str| SidecarInputFingerprint {
            hash: hash.into(),
            symbol_doc_count: 0,
            projection_count: 0,
            dense_projection_count: 0,
            semantic_policy_version: Some(crate::generation::SEMANTIC_POLICY_VERSION.into()),
            graph_artifact_hash: format!("graph-{hash}"),
            dense_reason_counts_json: "{}".into(),
            lexical_file_count: 0,
            lexical_hash: format!("lexical-{hash}"),
            lexical_coverage: Default::default(),
        };
        let manifest = |input: &SidecarInputFingerprint, built_at_epoch_ms: i64| {
            let mut manifest = retrieval_manifest_for_sidecar(
                "proj",
                &sidecar_generation_id("proj", &input.hash),
                &crate::generation::sidecar_vector_generation("proj", &input.hash),
                crate::embeddings::PRODUCT_EMBEDDING_RUNTIME_ID,
                crate::embeddings::RETRIEVAL_EMBEDDING_DIM as i32,
                input,
            );
            manifest.built_at_epoch_ms = built_at_epoch_ms;
            manifest
        };
        let rollback_input = input("11111111111111111111111111111111");
        let current_input = input("22222222222222222222222222222222");
        let candidate_input = input("33333333333333333333333333333333");
        let rollback_manifest = manifest(&rollback_input, 1);
        let current_manifest = manifest(&current_input, 2);
        let candidate_manifest = manifest(&candidate_input, 3);
        let rollback = RetrievalIndexRollbackRecord {
            manifest: rollback_manifest,
            verified_at_epoch_ms: 2,
        };
        storage
            .publish_retrieval_index_publication(&current_manifest, Some(&rollback))
            .expect("seed current and rollback pointers");
        let prior = storage
            .get_retrieval_index_publication("proj")
            .expect("read prior publication");
        let checks = std::cell::Cell::new(0_u8);

        let error = promote_retrieval_manifest_with_cancel(
            &mut storage,
            &candidate_input,
            &candidate_manifest,
            |_| Ok(candidate_input.clone()),
            |_| Ok(()),
            |_| Ok(()),
            |_| {
                Ok(Some(RetrievalIndexRollbackRecord {
                    manifest: current_manifest.clone(),
                    verified_at_epoch_ms: 3,
                }))
            },
            || {
                let next = checks.get() + 1;
                checks.set(next);
                if next == 3 {
                    bail!("simulated cancellation before SQLite commit");
                }
                Ok(())
            },
        )
        .expect_err("cancellation before commit must reject publication");

        assert!(error.to_string().contains("simulated cancellation"));
        assert_eq!(
            checks.get(),
            3,
            "cancellation did not reach the commit fence"
        );
        assert_eq!(
            storage
                .get_retrieval_index_publication("proj")
                .expect("read publication after cancellation"),
            prior,
            "cancelled transaction changed current or rollback pointers"
        );
    }

    #[test]
    fn partial_zero_dense_vector_candidate_repairs_then_promotes_without_weakening_prior() {
        let _env = crate::test_support::env_lock();
        let project = TempDir::new().expect("project");
        let storage_dir = TempDir::new().expect("storage");
        let cache = TempDir::new().expect("cache");
        let storage_path = storage_dir.path().join("codestory.db");
        let publication = codestory_store::IndexPublicationRecord {
            generation: 1,
            generation_id: "11111111-1111-4111-8111-111111111111".into(),
            run_id: "run-one".into(),
            mode: codestory_store::IndexPublicationMode::Full,
            published_at_epoch_ms: 1,
        };
        let mut storage = Store::open(&storage_path).expect("open storage");
        storage
            .put_index_publication(&publication)
            .expect("publish core identity");
        storage
            .publish_dense_anchor_generation(
                &publication,
                crate::generation::SEMANTIC_POLICY_VERSION,
            )
            .expect("publish empty dense-anchor generation");
        let runtime = crate::config::with_test_cache_root(cache.path(), || {
            SidecarRuntimeConfig::for_project_profile(
                Some(project.path()),
                crate::SidecarProfile::Local,
            )
        });
        let (residency, _active) = crate::embeddings::ProductEmbeddingResidencyLease::test_lease(
            test_embedding_identity("accelerated"),
        );
        let device = crate::embeddings::EmbeddingDeviceReadiness {
            requested_policy: "accelerator_required",
            observed_state: "accelerated",
            observation_source: "per_user_server",
            detected_provider: Some("metal".into()),
            detected_gpu: Some("test accelerator".into()),
            accelerator_requested: true,
            accelerator_request_provider: Some("metal".into()),
            accelerator_request_device: Some("test accelerator".into()),
            cpu_allowed: false,
            full_retrieval_allowed: true,
            degraded_reason: None,
        };
        let producer_compatibility_identity = vector_producer_compatibility_identity(
            &device,
            residency.identity(),
            crate::embeddings::RETRIEVAL_EMBEDDING_DIM as u32,
        )
        .expect("producer compatibility identity");
        let input = |hash: &str| SidecarInputFingerprint {
            hash: hash.into(),
            symbol_doc_count: 0,
            projection_count: 0,
            dense_projection_count: 0,
            semantic_policy_version: Some(crate::generation::SEMANTIC_POLICY_VERSION.into()),
            graph_artifact_hash: format!("graph-{hash}"),
            dense_reason_counts_json: "{}".into(),
            lexical_file_count: 0,
            lexical_hash: format!("lexical-{hash}"),
            lexical_coverage: Default::default(),
        };
        let manifest = |input: &SidecarInputFingerprint, built_at_epoch_ms: i64| {
            let mut manifest = retrieval_manifest_for_sidecar(
                "proj",
                &sidecar_generation_id("proj", &input.hash),
                &crate::generation::sidecar_vector_generation("proj", &input.hash),
                crate::embeddings::PRODUCT_EMBEDDING_RUNTIME_ID,
                crate::embeddings::RETRIEVAL_EMBEDDING_DIM as i32,
                input,
            );
            manifest.built_at_epoch_ms = built_at_epoch_ms;
            manifest
        };
        let rollback_input = input(&"a".repeat(64));
        let current_input = input(&"b".repeat(64));
        let candidate_input = input(&"c".repeat(64));
        let rollback_manifest = manifest(&rollback_input, 1);
        let current_manifest = manifest(&current_input, 2);
        let candidate_manifest = manifest(&candidate_input, 3);
        let publish_empty_vectors = |manifest: &RetrievalIndexManifest,
                                     publish_manifest: bool|
         -> VectorGenerationManifest {
            let evidence = build_vector_producer_evidence(
                &device,
                residency.identity(),
                crate::embeddings::RETRIEVAL_EMBEDDING_DIM as u32,
                EmbeddingVectorPublicationIdentityDto {
                    core_generation_id: publication.generation_id.clone(),
                    core_run_id: publication.run_id.clone(),
                    retrieval_generation: manifest
                        .sidecar_generation
                        .clone()
                        .expect("retrieval generation"),
                    retrieval_input_hash: manifest
                        .sidecar_input_hash
                        .clone()
                        .expect("retrieval input hash"),
                    semantic_generation: manifest.semantic_generation.clone(),
                },
            );
            let contract = VectorEvidenceContract::new(
                manifest
                    .embedding_backend
                    .clone()
                    .expect("embedding backend"),
                usize::try_from(manifest.embedding_dim.expect("embedding dimension"))
                    .expect("positive embedding dimension"),
                crate::embeddings::PRODUCT_EMBEDDING_RUNTIME_ID,
                vector_compatibility_identity(&evidence).expect("compatibility identity"),
            );
            let attestation = EmbeddedVectorIndex::build_attested_with_points(
                &runtime.layout,
                &manifest.semantic_generation,
                manifest
                    .sidecar_generation
                    .as_deref()
                    .expect("retrieval generation"),
                manifest
                    .sidecar_input_hash
                    .as_deref()
                    .expect("retrieval input hash"),
                &contract,
                &[],
                |_visit| Ok(()),
            )
            .expect("publish empty vector database");
            let generation = VectorGenerationManifest::new(evidence, attestation)
                .expect("build vector generation manifest");
            if publish_manifest {
                EmbeddedVectorIndex::publish_generation_manifest(
                    &runtime.layout,
                    &manifest.semantic_generation,
                    &generation,
                )
                .expect("publish vector generation manifest");
            }
            generation
        };
        publish_empty_vectors(&rollback_manifest, true);
        publish_empty_vectors(&current_manifest, true);
        let rollback = RetrievalIndexRollbackRecord {
            manifest: rollback_manifest.clone(),
            verified_at_epoch_ms: 2,
        };
        storage
            .publish_retrieval_index_publication(&current_manifest, Some(&rollback))
            .expect("seed current and rollback pointers");
        let prior = storage
            .get_retrieval_index_publication("proj")
            .expect("read prior publication");

        let candidate_generation = publish_empty_vectors(&candidate_manifest, false);
        EmbeddedVectorIndex::publish_generation_manifest_with_cancel(
            &runtime.layout,
            &candidate_manifest.semantic_generation,
            &candidate_generation,
            || bail!("simulated cancellation after vector database publication"),
        )
        .expect_err("evidence publication must be cancelled");
        assert_eq!(
            storage
                .get_retrieval_index_publication("proj")
                .expect("read publication after evidence cancellation"),
            prior,
            "partial candidate changed current or rollback pointers"
        );
        for prior_manifest in [&current_manifest, &rollback_manifest] {
            crate::embedded_vector::validate_generation_evidence_for_publication(
                &runtime.layout,
                &storage,
                prior_manifest,
                &publication,
                &runtime,
                &device,
                residency.identity(),
            )
            .expect("prior generation remains deeply usable");
        }
        let semantic = SemanticGeneration {
            layout: &runtime.layout,
            collection: &candidate_manifest.semantic_generation,
            generation: candidate_manifest
                .sidecar_generation
                .as_deref()
                .expect("candidate generation"),
            input_hash: candidate_manifest
                .sidecar_input_hash
                .as_deref()
                .expect("candidate input hash"),
            embedding_backend: candidate_manifest
                .embedding_backend
                .as_deref()
                .expect("candidate embedding backend"),
            embedding_dim: candidate_manifest
                .embedding_dim
                .expect("candidate embedding dimension"),
            expected_points: 0,
        };
        let context = GenerationRetentionContext {
            runtime: &runtime,
            layout: &runtime.layout,
            workspace_id: "workspace",
            previous_manifest: Some(&current_manifest),
            embedding_device: &device,
            embedding_residency: residency,
            producer_compatibility_identity,
        };
        let cancelled = AtomicBool::new(false);
        ensure_semantic_index(
            &storage_path,
            "proj",
            &semantic,
            Some(0),
            &context,
            &cancelled,
            &mut |_| {},
        )
        .expect("retry repairs the partial zero-dense candidate");
        crate::embedded_vector::validate_generation_evidence_for_publication(
            &runtime.layout,
            &storage,
            &candidate_manifest,
            &publication,
            &runtime,
            &device,
            context.embedding_residency.identity(),
        )
        .expect("repaired candidate deep-validates");

        promote_retrieval_manifest(
            &mut storage,
            &candidate_input,
            &candidate_manifest,
            |_| Ok(candidate_input.clone()),
            |storage| {
                crate::embedded_vector::validate_generation_evidence_for_publication(
                    &runtime.layout,
                    storage,
                    &candidate_manifest,
                    &publication,
                    &runtime,
                    &device,
                    context.embedding_residency.identity(),
                )
                .map(|_| ())
            },
            |_| Ok(()),
            |_| {
                Ok(Some(RetrievalIndexRollbackRecord {
                    manifest: current_manifest.clone(),
                    verified_at_epoch_ms: 3,
                }))
            },
        )
        .expect("promote repaired candidate");
        assert_eq!(
            storage
                .get_retrieval_index_publication("proj")
                .expect("read repaired publication"),
            Some((
                candidate_manifest,
                Some(RetrievalIndexRollbackRecord {
                    manifest: current_manifest,
                    verified_at_epoch_ms: 3,
                }),
            ))
        );
    }

    #[test]
    fn semantic_projection_excludes_low_value_local_symbols() {
        let row = |kind: NodeKind| SearchSymbolProjectionDetail {
            node_id: codestory_contracts::graph::NodeId(1),
            display_name: "symbol".into(),
            node_kind: Some(kind as i64),
            file_path: Some("src/lib.rs".into()),
            start_line: Some(1),
            end_line: Some(1),
        };

        assert!(semantic_projection_row(&row(NodeKind::FUNCTION)));
        assert!(semantic_projection_row(&row(NodeKind::ENUM_CONSTANT)));
        assert!(!semantic_projection_row(&row(NodeKind::VARIABLE)));
        assert!(!semantic_projection_row(&row(NodeKind::FIELD)));
        assert!(!semantic_projection_row(&row(NodeKind::UNKNOWN)));
    }

    #[test]
    fn sidecar_input_preparation_ignores_empty_and_stale_legacy_symbol_projection() {
        let project = TempDir::new().expect("project");
        std::fs::write(project.path().join("lib.rs"), "pub fn do_work() {}\n")
            .expect("write source");
        let mut storage = Store::new_in_memory().expect("storage");
        insert_matching_semantic_doc(&mut storage, project.path());

        assert_eq!(
            storage
                .get_search_symbol_projection_count()
                .expect("empty legacy projection count"),
            0
        );
        let empty_projection = compute_sidecar_input_fingerprint(
            &storage,
            project.path(),
            "project",
            crate::embeddings::PRODUCT_EMBEDDING_RUNTIME_ID,
            crate::embeddings::RETRIEVAL_EMBEDDING_DIM as i32,
            "producer-compatibility-v1",
        )
        .expect("prepare sidecar input without legacy projection");

        storage
            .upsert_search_symbol_projection_batch(&[SearchSymbolProjection {
                node_id: NodeId(1),
                display_name: "stale_wrong_name".into(),
            }])
            .expect("seed stale legacy projection");
        let stale_projection = compute_sidecar_input_fingerprint(
            &storage,
            project.path(),
            "project",
            crate::embeddings::PRODUCT_EMBEDDING_RUNTIME_ID,
            crate::embeddings::RETRIEVAL_EMBEDDING_DIM as i32,
            "producer-compatibility-v1",
        )
        .expect("prepare sidecar input with stale legacy projection");

        assert_eq!(empty_projection, stale_projection);
    }

    #[test]
    fn canonical_sidecar_generation_is_stable_across_clean_roots_with_same_input() {
        let Some(first_project) = git_project() else {
            return;
        };
        let Some(second_project) = git_project() else {
            return;
        };
        let first_storage_dir = TempDir::new().expect("first storage dir");
        let second_storage_dir = TempDir::new().expect("second storage dir");
        let first_storage_path = first_storage_dir.path().join("codestory.db");
        let second_storage_path = second_storage_dir.path().join("codestory.db");
        let mut first_storage = Store::open(&first_storage_path).expect("first store");
        let mut second_storage = Store::open(&second_storage_path).expect("second store");
        insert_matching_semantic_doc(&mut first_storage, first_project.path());
        insert_matching_semantic_doc(&mut second_storage, second_project.path());

        let first_project_id = sidecar_project_id_for_root(first_project.path());
        let second_project_id = sidecar_project_id_for_root(second_project.path());
        assert_eq!(first_project_id, second_project_id);
        assert_ne!(
            project_id_for_root(first_project.path()),
            project_id_for_root(second_project.path())
        );

        let first_input = compute_sidecar_input_fingerprint(
            &first_storage,
            first_project.path(),
            &first_project_id,
            crate::embeddings::PRODUCT_EMBEDDING_RUNTIME_ID,
            crate::embeddings::RETRIEVAL_EMBEDDING_DIM as i32,
            "producer-compatibility-v1",
        )
        .expect("first input");
        let second_input = compute_sidecar_input_fingerprint(
            &second_storage,
            second_project.path(),
            &second_project_id,
            crate::embeddings::PRODUCT_EMBEDDING_RUNTIME_ID,
            crate::embeddings::RETRIEVAL_EMBEDDING_DIM as i32,
            "producer-compatibility-v1",
        )
        .expect("second input");

        assert_eq!(first_input.hash, second_input.hash);
        assert_eq!(
            sidecar_generation_id(&first_project_id, &first_input.hash),
            sidecar_generation_id(&second_project_id, &second_input.hash)
        );
    }

    #[test]
    fn producer_compatibility_change_selects_a_distinct_immutable_generation() {
        let project = TempDir::new().expect("project");
        std::fs::write(project.path().join("lib.rs"), "pub fn stable() {}\n").expect("source");
        let storage = Store::new_in_memory().expect("storage");
        storage
            .put_index_publication(&codestory_store::IndexPublicationRecord {
                generation: 1,
                generation_id: "core-generation".into(),
                run_id: "core-run".into(),
                mode: codestory_store::IndexPublicationMode::Full,
                published_at_epoch_ms: 1,
            })
            .expect("core publication");
        let project_id = "project";
        let package_a = compute_sidecar_input_fingerprint(
            &storage,
            project.path(),
            project_id,
            crate::embeddings::PRODUCT_EMBEDDING_RUNTIME_ID,
            crate::embeddings::RETRIEVAL_EMBEDDING_DIM as i32,
            "package-runtime-sidecar-evidence-a",
        )
        .expect("package A input");
        let package_b = compute_sidecar_input_fingerprint(
            &storage,
            project.path(),
            project_id,
            crate::embeddings::PRODUCT_EMBEDDING_RUNTIME_ID,
            crate::embeddings::RETRIEVAL_EMBEDDING_DIM as i32,
            "package-runtime-sidecar-evidence-b",
        )
        .expect("package B input");

        assert_ne!(package_a.hash, package_b.hash);
        assert_ne!(
            sidecar_generation_id(project_id, &package_a.hash),
            sidecar_generation_id(project_id, &package_b.hash),
            "a package/runtime producer change must not overwrite the prior sidecar generation"
        );
        assert_ne!(
            crate::generation::sidecar_vector_generation(project_id, &package_a.hash),
            crate::generation::sidecar_vector_generation(project_id, &package_b.hash)
        );
    }

    #[test]
    fn dirty_canonical_repo_falls_back_to_root_sidecar_project_id() {
        let Some(project) = git_project() else {
            return;
        };
        let clean = sidecar_project_id_for_root(project.path());
        std::fs::write(project.path().join("lib.rs"), "pub fn dirty() {}\n").expect("dirty source");
        let dirty = sidecar_project_id_for_root(project.path());

        assert_ne!(clean, dirty);
        assert_eq!(dirty, project_id_for_root(project.path()));
    }

    #[test]
    fn manifest_promotion_rejects_same_count_content_drift_and_preserves_current() {
        let _env = crate::test_support::env_lock();
        let project = TempDir::new().expect("project dir");
        std::fs::write(project.path().join("lib.rs"), "pub fn do_work() {}\n")
            .expect("project file");
        let storage_dir = TempDir::new().expect("storage dir");
        let storage_path = storage_dir.path().join("codestory.db");
        let mut storage = Store::open(&storage_path).expect("open store");
        let runtime = SidecarRuntimeConfig::local();
        let embedding_contract = SidecarEmbeddingContract {
            backend: crate::embeddings::PRODUCT_EMBEDDING_RUNTIME_ID,
            dimension: crate::embeddings::RETRIEVAL_EMBEDDING_DIM as i32,
            producer_compatibility_identity: "producer-compatibility-v1",
        };
        storage
            .insert_nodes_batch(&[Node {
                id: NodeId(1),
                kind: NodeKind::FUNCTION,
                serialized_name: "do_work".into(),
                ..Default::default()
            }])
            .expect("node");
        let mut doc = DenseAnchorInput {
            node_id: NodeId(1),
            file_node_id: None,
            kind: NodeKind::FUNCTION,
            display_name: "do_work".into(),
            qualified_name: Some("pkg::do_work".into()),
            file_path: Some("lib.rs".into()),
            start_line: Some(1),
            end_line: Some(2),
            file_role: FileRole::Source,
            source_provenance: "parser".into(),
            text: "semantic doc".into(),
            document_hash: "hash-one".into(),
            selection_reason: "public_api".into(),
            policy_version: crate::generation::SEMANTIC_POLICY_VERSION.into(),
            source_identity: "core:g1:r1".into(),
            updated_at_epoch_ms: 123,
        };
        storage
            .upsert_dense_anchor_inputs_batch(&[doc.clone()])
            .expect("first doc");
        storage
            .put_index_publication(&codestory_store::IndexPublicationRecord {
                generation: 1,
                generation_id: "g1".into(),
                run_id: "r1".into(),
                mode: codestory_store::IndexPublicationMode::Full,
                published_at_epoch_ms: 1,
            })
            .expect("first core publication");
        let first = compute_sidecar_input_fingerprint(
            &storage,
            project.path(),
            "proj",
            crate::embeddings::PRODUCT_EMBEDDING_RUNTIME_ID,
            crate::embeddings::RETRIEVAL_EMBEDDING_DIM as i32,
            "producer-compatibility-v1",
        )
        .expect("first fingerprint");
        doc.source_identity = "core:g2:r2".into();
        storage
            .upsert_dense_anchor_inputs_batch(&[doc.clone()])
            .expect("rebind unchanged doc");
        storage
            .put_index_publication(&codestory_store::IndexPublicationRecord {
                generation: 2,
                generation_id: "g2".into(),
                run_id: "r2".into(),
                mode: codestory_store::IndexPublicationMode::Full,
                published_at_epoch_ms: 2,
            })
            .expect("second core publication");
        let rebound = compute_sidecar_input_fingerprint(
            &storage,
            project.path(),
            "proj",
            crate::embeddings::PRODUCT_EMBEDDING_RUNTIME_ID,
            crate::embeddings::RETRIEVAL_EMBEDDING_DIM as i32,
            "producer-compatibility-v1",
        )
        .expect("rebound fingerprint");
        assert_ne!(
            first.hash, rebound.hash,
            "publication-bound vector evidence requires a fresh immutable generation"
        );
        let old_manifest = retrieval_manifest_for_sidecar(
            "proj",
            &sidecar_generation_id("proj", &first.hash),
            &crate::generation::sidecar_vector_generation("proj", &first.hash),
            crate::embeddings::PRODUCT_EMBEDDING_RUNTIME_ID,
            crate::embeddings::RETRIEVAL_EMBEDDING_DIM as i32,
            &first,
        );
        storage
            .upsert_retrieval_index_manifest(&old_manifest)
            .expect("old manifest");
        doc.text = "changed semantic doc".into();
        doc.document_hash = "hash-two".into();
        let mut concurrent = Store::open(&storage_path).expect("concurrent store");
        concurrent
            .upsert_dense_anchor_inputs_batch(&[doc])
            .expect("second doc");
        drop(concurrent);
        let second = compute_sidecar_input_fingerprint(
            &storage,
            project.path(),
            "proj",
            crate::embeddings::PRODUCT_EMBEDDING_RUNTIME_ID,
            crate::embeddings::RETRIEVAL_EMBEDDING_DIM as i32,
            "producer-compatibility-v1",
        )
        .expect("second fingerprint");

        assert_eq!(first.projection_count, 1);
        assert_eq!(second.projection_count, 1);
        assert_eq!(first.dense_projection_count, 1);
        assert_eq!(first.dense_reason_counts_json, "{\"public_api\":1}");
        assert_ne!(first.hash, second.hash);

        let mut rejected_manifest = retrieval_manifest_for_sidecar(
            "proj",
            &sidecar_generation_id("proj", &first.hash),
            &crate::generation::sidecar_vector_generation("proj", &first.hash),
            crate::embeddings::PRODUCT_EMBEDDING_RUNTIME_ID,
            crate::embeddings::RETRIEVAL_EMBEDDING_DIM as i32,
            &first,
        );
        rejected_manifest.built_at_epoch_ms += 1;
        let lexical_source = lexical_source_input(project.path()).expect("lexical source");
        let rejected = promote_retrieval_manifest(
            &mut storage,
            &first,
            &rejected_manifest,
            |snapshot| {
                compute_sidecar_input_fingerprint_with_lexical_source(
                    snapshot,
                    project.path(),
                    "proj",
                    &embedding_contract,
                    lexical_source,
                )
            },
            |_| Ok(()),
            |_| Ok(()),
            |_| Ok(None),
        );
        assert!(rejected.is_err());
        assert_eq!(
            storage
                .get_retrieval_index_manifest("proj")
                .expect("current manifest"),
            Some(old_manifest.clone())
        );

        let mut new_manifest = retrieval_manifest_for_sidecar(
            "proj",
            &sidecar_generation_id("proj", &second.hash),
            &crate::generation::sidecar_vector_generation("proj", &second.hash),
            crate::embeddings::PRODUCT_EMBEDDING_RUNTIME_ID,
            crate::embeddings::RETRIEVAL_EMBEDDING_DIM as i32,
            &second,
        );
        new_manifest.scip_revision = Some("graph-test".into());
        let passing = CandidateGenerationEvidence {
            lexical_matches: true,
            scip_revision: new_manifest.scip_revision.clone(),
            scip_graph: true,
            semantic_points: Some(second.projection_count as u64),
            semantic_ready: true,
            semantic_zero_dense_policy: false,
            embedding_device: crate::embeddings::EmbeddingDeviceReadiness {
                requested_policy: "accelerator_required",
                observed_state: "accelerated",
                observation_source: "per_user_server",
                detected_provider: Some("test".into()),
                detected_gpu: Some("test accelerator".into()),
                accelerator_requested: true,
                accelerator_request_provider: Some("test".into()),
                accelerator_request_device: None,
                cpu_allowed: false,
                full_retrieval_allowed: true,
                degraded_reason: None,
            },
            embedding_accelerator_smoke_elapsed_ms: Some(12),
            embedding_identity_before: test_embedding_identity("accelerated"),
            embedding_identity_after: test_embedding_identity("accelerated"),
            retrieval_mode: "full".into(),
            degraded_reason: None,
        };
        let mut zero_dense_input = second.clone();
        zero_dense_input.hash = "zero-dense-candidate".into();
        zero_dense_input.projection_count = 0;
        zero_dense_input.dense_projection_count = 0;
        zero_dense_input.dense_reason_counts_json = "{}".into();
        let mut zero_dense_manifest = retrieval_manifest_for_sidecar(
            "proj",
            &sidecar_generation_id("proj", &zero_dense_input.hash),
            &crate::generation::sidecar_vector_generation("proj", &zero_dense_input.hash),
            crate::embeddings::PRODUCT_EMBEDDING_RUNTIME_ID,
            crate::embeddings::RETRIEVAL_EMBEDDING_DIM as i32,
            &zero_dense_input,
        );
        zero_dense_manifest.scip_revision = Some("graph-test".into());
        let mut zero_dense_evidence = passing.clone();
        zero_dense_evidence.scip_revision = zero_dense_manifest.scip_revision.clone();
        zero_dense_evidence.semantic_points = Some(0);
        zero_dense_evidence.semantic_zero_dense_policy = true;
        validate_candidate_generation_evidence(
            "proj",
            &zero_dense_input,
            &zero_dense_manifest,
            &runtime,
            &zero_dense_evidence,
        )
        .expect("zero-dense policy requires an attested empty vector generation");
        zero_dense_evidence.semantic_points = None;
        validate_candidate_generation_evidence(
            "proj",
            &zero_dense_input,
            &zero_dense_manifest,
            &runtime,
            &zero_dense_evidence,
        )
        .expect_err("a missing zero-dense vector generation must reject promotion");

        let mut missing_nonzero_collection = passing.clone();
        missing_nonzero_collection.semantic_points = None;
        validate_candidate_generation_evidence(
            "proj",
            &second,
            &new_manifest,
            &runtime,
            &missing_nonzero_collection,
        )
        .expect_err("a missing nonzero vector generation must reject promotion");

        let mut cpu_runtime = runtime.clone();
        cpu_runtime.embedding.allow_cpu = true;
        let cpu_input = compute_sidecar_input_fingerprint(
            &storage,
            project.path(),
            "proj",
            crate::embeddings::PRODUCT_EMBEDDING_RUNTIME_ID,
            crate::embeddings::RETRIEVAL_EMBEDDING_DIM as i32,
            "producer-compatibility-v1",
        )
        .expect("cpu-policy fingerprint");
        let mut cpu_manifest = retrieval_manifest_for_sidecar(
            "proj",
            &sidecar_generation_id("proj", &cpu_input.hash),
            &crate::generation::sidecar_vector_generation("proj", &cpu_input.hash),
            crate::embeddings::PRODUCT_EMBEDDING_RUNTIME_ID,
            crate::embeddings::RETRIEVAL_EMBEDDING_DIM as i32,
            &cpu_input,
        );
        cpu_manifest.scip_revision = Some("graph-test".into());
        let mut cpu_evidence = passing.clone();
        cpu_evidence.scip_revision = cpu_manifest.scip_revision.clone();
        cpu_evidence.embedding_device = crate::embeddings::EmbeddingDeviceReadiness {
            requested_policy: "cpu_explicit",
            observed_state: "cpu_explicit",
            observation_source: "per_user_server",
            detected_provider: None,
            detected_gpu: None,
            accelerator_requested: false,
            accelerator_request_provider: None,
            accelerator_request_device: None,
            cpu_allowed: true,
            full_retrieval_allowed: true,
            degraded_reason: None,
        };
        cpu_evidence.embedding_accelerator_smoke_elapsed_ms = None;
        cpu_evidence.embedding_identity_before = test_embedding_identity("cpu_explicit");
        cpu_evidence.embedding_identity_after = test_embedding_identity("cpu_explicit");
        validate_candidate_generation_evidence(
            "proj",
            &cpu_input,
            &cpu_manifest,
            &cpu_runtime,
            &cpu_evidence,
        )
        .expect("explicit CPU policy remains valid");
        let mut rollback_manifest = old_manifest.clone();
        let rollback_hash = "verified-rollback-input";
        rollback_manifest.sidecar_input_hash = Some(rollback_hash.into());
        rollback_manifest.sidecar_generation = Some(sidecar_generation_id("proj", rollback_hash));
        rollback_manifest.semantic_generation =
            crate::generation::sidecar_vector_generation("proj", rollback_hash);
        rollback_manifest.built_at_epoch_ms -= 1;
        let state_file = storage_dir.path().join("state/retrieval-sidecars.json");
        let marker = GenerationRetentionMarker::next(
            "workspace",
            old_manifest.clone(),
            Some(RetrievalIndexRollbackRecord {
                manifest: rollback_manifest,
                verified_at_epoch_ms: 123,
            }),
            123,
        )
        .expect("retention marker");
        write_retention_marker(&state_file, &marker).expect("write retention marker");
        let marker_before = read_retention_marker(&state_file, "workspace")
            .expect("read marker")
            .expect("marker exists");

        for component in [
            "lexical",
            "scip",
            "semantic",
            "embedding_runtime",
            "accelerator_proof",
        ] {
            let mut failing = passing.clone();
            let candidate_input = second.clone();
            match component {
                "lexical" => failing.lexical_matches = false,
                "scip" => failing.scip_revision = Some("wrong-revision".into()),
                "semantic" => failing.semantic_points = None,
                "embedding_runtime" => {
                    failing.embedding_identity_after.instance_id = "inprocess:replaced".into();
                }
                "accelerator_proof" => failing.embedding_accelerator_smoke_elapsed_ms = None,
                _ => unreachable!(),
            }
            let rejected = promote_retrieval_manifest(
                &mut storage,
                &candidate_input,
                &new_manifest,
                |_| Ok(candidate_input.clone()),
                |_| {
                    validate_candidate_generation_evidence(
                        "proj",
                        &candidate_input,
                        &new_manifest,
                        &runtime,
                        &failing,
                    )
                },
                |_| Ok(()),
                |_| Ok(None),
            )
            .expect_err("component failure must reject promotion");
            assert!(
                rejected.to_string().contains(component)
                    || component == "embedding_runtime"
                        && rejected.to_string().contains("does not match")
            );
            assert_eq!(
                storage
                    .get_retrieval_index_manifest("proj")
                    .expect("current manifest"),
                Some(old_manifest.clone()),
                "{component} failure changed current"
            );
            assert_eq!(
                read_retention_marker(&state_file, "workspace")
                    .expect("read marker")
                    .expect("marker exists"),
                marker_before,
                "{component} failure changed rollback"
            );
        }
        let crashed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = promote_retrieval_manifest(
                &mut storage,
                &second,
                &new_manifest,
                |_| Ok(second.clone()),
                |_| {
                    validate_candidate_generation_evidence(
                        "proj",
                        &second,
                        &new_manifest,
                        &runtime,
                        &passing,
                    )?;
                    panic!("simulated crash before candidate promotion")
                },
                |_| Ok(()),
                |_| Ok(None),
            );
        }));
        assert!(crashed.is_err());
        assert_eq!(
            storage
                .get_retrieval_index_manifest("proj")
                .expect("current manifest after crash"),
            Some(old_manifest.clone())
        );
        assert_eq!(
            read_retention_marker(&state_file, "workspace")
                .expect("read marker")
                .expect("marker exists"),
            marker_before
        );

        let source_path = project.path().join("lib.rs");
        let drifted = promote_retrieval_manifest(
            &mut storage,
            &second,
            &new_manifest,
            |snapshot| {
                let lexical_source =
                    lexical_source_input(project.path()).expect("fresh lexical source");
                compute_sidecar_input_fingerprint_with_lexical_source(
                    snapshot,
                    project.path(),
                    "proj",
                    &embedding_contract,
                    lexical_source,
                )
            },
            |_| {
                validate_candidate_generation_evidence(
                    "proj",
                    &second,
                    &new_manifest,
                    &runtime,
                    &passing,
                )?;
                std::fs::write(&source_path, "pub fn changed_during_validation() {}\n")?;
                Ok(())
            },
            |_| Ok(()),
            |_| Ok(None),
        )
        .expect_err("source drift during validation must reject publication");
        assert!(is_sidecar_input_changed(&drifted));
        assert!(drifted.to_string().contains("input changed"));
        assert_eq!(
            storage
                .get_retrieval_index_manifest("proj")
                .expect("current manifest after source drift"),
            Some(old_manifest.clone())
        );
        std::fs::write(&source_path, "pub fn do_work() {}\n").expect("restore source");

        let pointer_fault = promote_retrieval_manifest(
            &mut storage,
            &second,
            &new_manifest,
            |snapshot| {
                let lexical_source =
                    lexical_source_input(project.path()).expect("fresh lexical source");
                compute_sidecar_input_fingerprint_with_lexical_source(
                    snapshot,
                    project.path(),
                    "proj",
                    &embedding_contract,
                    lexical_source,
                )
            },
            |_| {
                validate_candidate_generation_evidence(
                    "proj",
                    &second,
                    &new_manifest,
                    &runtime,
                    &passing,
                )
            },
            |_| Ok(()),
            |_| bail!("simulated SQLite rollback pointer failure"),
        )
        .expect_err("SQLite rollback pointer failure must reject publication");
        assert!(
            pointer_fault
                .to_string()
                .contains("SQLite rollback pointer")
        );
        assert_eq!(
            storage
                .get_retrieval_index_manifest("proj")
                .expect("current manifest after retention pointer fault"),
            Some(old_manifest.clone())
        );
        assert_eq!(
            read_retention_marker(&state_file, "workspace")
                .expect("read marker")
                .expect("marker exists"),
            marker_before,
            "retention pointer failure changed rollback state"
        );

        let rollback_record = RetrievalIndexRollbackRecord {
            manifest: old_manifest.clone(),
            verified_at_epoch_ms: 456,
        };
        promote_retrieval_manifest(
            &mut storage,
            &second,
            &new_manifest,
            |snapshot| {
                let lexical_source =
                    lexical_source_input(project.path()).expect("fresh lexical source");
                compute_sidecar_input_fingerprint_with_lexical_source(
                    snapshot,
                    project.path(),
                    "proj",
                    &embedding_contract,
                    lexical_source,
                )
            },
            |_| {
                validate_candidate_generation_evidence(
                    "proj",
                    &second,
                    &new_manifest,
                    &runtime,
                    &passing,
                )
            },
            |_| Ok(()),
            |_| Ok(Some(rollback_record.clone())),
        )
        .expect("unchanged input promotes manifest");
        assert_eq!(
            storage
                .get_retrieval_index_publication("proj")
                .expect("current publication"),
            Some((new_manifest.clone(), Some(rollback_record.clone())))
        );

        let marker_blocker = storage_dir.path().join("marker-blocker");
        std::fs::write(&marker_blocker, b"not a directory").expect("marker blocker");
        let mut blocked_layout = runtime.layout.clone();
        blocked_layout.state_file = marker_blocker.join("retrieval.state");
        publish_derived_retention_marker(&storage, &blocked_layout, "workspace", "proj")
            .expect_err("derived marker failure happens after SQLite commit");
        assert_eq!(
            storage
                .get_retrieval_index_publication("proj")
                .expect("publication survives marker failure"),
            Some((new_manifest, Some(rollback_record.clone())))
        );
        let mut protection = scan_retention_protection(
            storage_dir.path(),
            Some(&storage_path),
            &blocked_layout.state_file,
        );
        protection
            .errors
            .push("derived marker publication failed".into());
        assert!(
            protection
                .authoritative_rollback
                .contains(&rollback_record.manifest),
            "restart scans must recover rollback authority from SQLite without a marker"
        );
        assert!(
            plan_generation_retention(&runtime.layout, "proj", &protection).pruning_suppressed,
            "marker failure must conservatively suppress pruning"
        );
    }

    #[cfg(feature = "test-support")]
    #[test]
    fn rollback_selection_rejects_corrupt_vector_evidence_and_anchor_publication() {
        use crate::test_support::{env_lock, publish_zero_dense_pinned_query_fixture};
        use codestory_store::{IndexPublicationMode, IndexPublicationRecord};
        use std::io::Write as _;

        let _env = env_lock();
        let project = TempDir::new().expect("project");
        let storage_dir = TempDir::new().expect("storage");
        let cache = TempDir::new().expect("cache");
        let storage_path = storage_dir.path().join("codestory.db");
        let store = Store::open(&storage_path).expect("open storage");
        store
            .put_index_publication(&IndexPublicationRecord {
                generation: 1,
                generation_id: "11111111-1111-4111-8111-111111111111".into(),
                run_id: "run-one".into(),
                mode: IndexPublicationMode::Full,
                published_at_epoch_ms: 1,
            })
            .expect("publish core identity");
        drop(store);
        let runtime = crate::config::with_test_cache_root(cache.path(), || {
            SidecarRuntimeConfig::for_project_profile(
                Some(project.path()),
                crate::SidecarProfile::Local,
            )
        });
        let previous =
            publish_zero_dense_pinned_query_fixture(project.path(), &storage_path, &runtime)
                .expect("publish rollback candidate");
        let active_hash = "b".repeat(64);
        let active =
            crate::test_support::retrieval_manifest_fixture(&previous.project_id, &active_hash);
        let select = |storage: &Store| {
            let residency =
                crate::embeddings::acquire_product_embedding_residency_for_runtime(&runtime)
                    .expect("acquire test residency");
            let device = crate::embeddings::embedding_device_readiness_for_runtime(&runtime);
            let producer_compatibility_identity = vector_producer_compatibility_identity(
                &device,
                residency.identity(),
                crate::embeddings::RETRIEVAL_EMBEDDING_DIM as u32,
            )
            .expect("producer compatibility identity");
            let context = GenerationRetentionContext {
                runtime: &runtime,
                layout: &runtime.layout,
                workspace_id: "workspace",
                previous_manifest: Some(&previous),
                embedding_device: &device,
                embedding_residency: residency,
                producer_compatibility_identity,
            };
            prepare_generation_retention(&context, &previous.project_id, &active, storage)
                .expect("prepare retention")
                .verified_previous
        };

        let storage = Store::open(&storage_path).expect("open candidate storage");
        assert!(
            select(&storage).is_some(),
            "complete rollback must be selected"
        );
        let vector_path =
            crate::embedded_vector::index_path(&runtime.layout, &previous.semantic_generation);
        std::fs::OpenOptions::new()
            .append(true)
            .open(&vector_path)
            .expect("open vector database")
            .write_all(b"corrupt")
            .expect("corrupt vector bytes");
        assert!(
            select(&storage).is_none(),
            "exact vector-byte corruption must disqualify rollback"
        );
        drop(storage);

        publish_zero_dense_pinned_query_fixture(project.path(), &storage_path, &runtime)
            .expect("repair vector fixture");
        let evidence_path =
            crate::embedded_vector::index_path(&runtime.layout, &previous.semantic_generation)
                .parent()
                .expect("vector collection directory")
                .join("vector-generation-manifest.json");
        std::fs::write(&evidence_path, b"{}\n").expect("corrupt producer evidence");
        let storage = Store::open(&storage_path).expect("open evidence-corrupt storage");
        assert!(
            select(&storage).is_none(),
            "producer-evidence corruption must disqualify rollback"
        );
        drop(storage);

        publish_zero_dense_pinned_query_fixture(project.path(), &storage_path, &runtime)
            .expect("repair evidence fixture");
        let storage = Store::open(&storage_path).expect("open anchor-corrupt storage");
        storage
            .get_connection()
            .execute(
                "UPDATE dense_anchor_publication SET anchor_digest = ?1 WHERE id = 1",
                rusqlite::params!["c".repeat(64)],
            )
            .expect("corrupt anchor publication");
        assert!(
            select(&storage).is_none(),
            "anchor-publication corruption must disqualify rollback"
        );
    }

    fn insert_matching_semantic_doc(storage: &mut Store, project_root: &Path) {
        storage
            .insert_nodes_batch(&[Node {
                id: NodeId(1),
                kind: NodeKind::FUNCTION,
                serialized_name: "do_work".into(),
                ..Default::default()
            }])
            .expect("node");
        storage
            .upsert_llm_symbol_docs_batch(&[LlmSymbolDoc {
                node_id: NodeId(1),
                file_node_id: None,
                kind: NodeKind::FUNCTION,
                display_name: "do_work".into(),
                qualified_name: Some("pkg::do_work".into()),
                file_path: Some(project_root.join("lib.rs").display().to_string()),
                start_line: Some(1),
                doc_text: "semantic doc".into(),
                doc_version: 4,
                doc_hash: "hash".into(),
                embedding_profile: Some("coderank-embed".into()),
                embedding_model: "legacy-producer".into(),
                embedding_backend: Some("legacy".into()),
                embedding_dim: crate::embeddings::RETRIEVAL_EMBEDDING_DIM as u32,
                doc_shape: Some("semantic_doc_version=4;scope=durable_symbols".into()),
                semantic_policy_version: Some(crate::generation::SEMANTIC_POLICY_VERSION.into()),
                dense_reason: Some("public_api".into()),
                embedding: vec![0.01; crate::embeddings::RETRIEVAL_EMBEDDING_DIM],
                updated_at_epoch_ms: 123,
            }])
            .expect("semantic doc");
    }

    fn git_project() -> Option<TempDir> {
        if std::process::Command::new("git")
            .arg("--version")
            .output()
            .is_err()
        {
            return None;
        }
        let project = TempDir::new().expect("project");
        git(project.path(), &["init"]);
        git(
            project.path(),
            &["config", "user.email", "codestory@example.invalid"],
        );
        git(project.path(), &["config", "user.name", "CodeStory Test"]);
        git(
            project.path(),
            &[
                "remote",
                "add",
                "origin",
                "https://github.com/TheGreenCedar/CodeStory.git",
            ],
        );
        std::fs::write(project.path().join("lib.rs"), "pub fn do_work() {}\n")
            .expect("write source");
        git(project.path(), &["add", "."]);
        git(project.path(), &["commit", "-m", "init"]);
        Some(project)
    }

    fn git(project: &Path, args: &[&str]) {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(project)
            .args(args)
            .output()
            .expect("run git");
        assert!(
            output.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
