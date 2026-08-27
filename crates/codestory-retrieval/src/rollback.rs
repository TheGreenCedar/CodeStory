//! Validated activation of the retained retrieval rollback generation.
//!
//! Retention already keeps one *verified* previous generation beside the
//! current pointer, in the same SQLite row, so a reader observes either the
//! complete old pair or the complete new pair. Until now no code path could
//! select it: a corrupt or unusable current generation could only be repaired
//! by rebuilding, which is minutes to hours of work for a lever the product
//! already paid to retain.
//!
//! Activation re-proves the retained candidate against live state before it
//! moves the pointer, at the same bar retention used to call it verified —
//! current sidecar-generation contract, deep vector-evidence validation
//! against the live complete core publication, and a live-ready health probe.
//! A candidate that no longer proves out is refused with a typed code and the
//! current pointer is left exactly where it was: this lever can only ever
//! narrow what is served, never widen it.

use std::fmt;
use std::path::Path;

use anyhow::{Context, Result};
use codestory_store::{RetrievalIndexManifest, RetrievalIndexRollbackRecord, Store};
use serde::Serialize;
use tracing::warn;

use crate::config::SidecarRuntimeConfig;
use crate::embedding_server_compat::ProductEmbeddingIdentity;
use crate::embeddings::EmbeddingDeviceReadiness;
use crate::generation::manifest_has_current_sidecar_contract;
use crate::health::probe_sidecar_health_for_runtime;
use crate::index::sidecar_project_id_for_runtime;

/// Why validated rollback activation left the current pointer alone.
///
/// Every variant is a refusal to serve something new, never a refusal that
/// removes an existing capability, so callers can report all of them as
/// "nothing changed" without inspecting the code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum RollbackActivationRefusal {
    /// No retrieval publication row exists for this project id.
    NoPublication,
    /// The publication row carries no retained rollback pointer.
    NoRetainedRollback,
    /// The retained rollback manifest no longer satisfies the current
    /// sidecar-generation contract, so nothing downstream would admit it.
    RollbackContractMissing { sidecar_generation: Option<String> },
    /// The core publication is missing or incomplete, so no retrieval
    /// generation can be bound to it.
    CorePublicationIncomplete,
    /// The retained generation's vector bytes, producer evidence, or anchor
    /// coverage no longer validate against the live core publication.
    RollbackEvidenceInvalid { reason: String },
    /// The retained generation does not probe as a live-ready full retrieval
    /// mode with the artifacts currently on disk.
    RollbackNotLiveReady {
        retrieval_mode: String,
        degraded_reason: Option<String>,
    },
    /// The publication pair changed between validation and activation, so the
    /// validated candidate is no longer the one that would be written.
    PublicationRaced,
}

impl RollbackActivationRefusal {
    /// Stable machine code for this refusal.
    pub fn code(&self) -> &'static str {
        match self {
            Self::NoPublication => "no_publication",
            Self::NoRetainedRollback => "no_retained_rollback",
            Self::RollbackContractMissing { .. } => "rollback_contract_missing",
            Self::CorePublicationIncomplete => "core_publication_incomplete",
            Self::RollbackEvidenceInvalid { .. } => "rollback_evidence_invalid",
            Self::RollbackNotLiveReady { .. } => "rollback_not_live_ready",
            Self::PublicationRaced => "publication_raced",
        }
    }
}

impl fmt::Display for RollbackActivationRefusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoPublication => {
                write!(
                    formatter,
                    "{}: this project has published no retrieval generation",
                    self.code()
                )
            }
            Self::NoRetainedRollback => write!(
                formatter,
                "{}: no verified rollback generation is retained beside the current pointer",
                self.code()
            ),
            Self::RollbackContractMissing { sidecar_generation } => write!(
                formatter,
                "{}: retained rollback generation {} does not satisfy the current sidecar generation contract",
                self.code(),
                sidecar_generation.as_deref().unwrap_or("<missing>")
            ),
            Self::CorePublicationIncomplete => write!(
                formatter,
                "{}: no complete core publication is available to bind a retrieval generation to",
                self.code()
            ),
            Self::RollbackEvidenceInvalid { reason } => {
                write!(formatter, "{}: {reason}", self.code())
            }
            Self::RollbackNotLiveReady {
                retrieval_mode,
                degraded_reason,
            } => write!(
                formatter,
                "{}: retained rollback generation probes as {retrieval_mode} ({})",
                self.code(),
                degraded_reason.as_deref().unwrap_or("no degraded reason")
            ),
            Self::PublicationRaced => write!(
                formatter,
                "{}: the retrieval publication changed while the rollback candidate was being validated",
                self.code()
            ),
        }
    }
}

/// Typed failure surface for rollback activation.
#[derive(Debug)]
pub enum RollbackActivationError {
    /// Activation completed its checks and declined; nothing was written.
    Refused(RollbackActivationRefusal),
    /// Activation could not complete its checks.
    Failed(anyhow::Error),
}

impl RollbackActivationError {
    /// Stable machine code for this failure.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Refused(refusal) => refusal.code(),
            Self::Failed(_) => "rollback_activation_failed",
        }
    }
}

impl From<RollbackActivationRefusal> for RollbackActivationError {
    fn from(value: RollbackActivationRefusal) -> Self {
        Self::Refused(value)
    }
}

impl fmt::Display for RollbackActivationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Refused(refusal) => write!(formatter, "{refusal}"),
            Self::Failed(error) => write!(formatter, "{error:#}"),
        }
    }
}

impl std::error::Error for RollbackActivationError {}

/// What validated rollback activation observed, and whether it applied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RollbackActivationOutcome {
    pub project_id: String,
    /// `false` for a validation-only run: every check ran, nothing was written.
    pub applied: bool,
    pub previous_generation: Option<String>,
    pub previous_semantic_generation: String,
    pub activated_generation: String,
    pub activated_semantic_generation: String,
    pub activated_built_at_epoch_ms: i64,
    pub rollback_verified_at_epoch_ms: i64,
    /// Activation consumes the retained pointer: the generation it replaced is
    /// the one the operator is stepping away from, so it is not re-armed as a
    /// rollback target.
    pub rollback_pointer_retained: bool,
    pub activated_retrieval_mode: String,
}

/// The retained rollback pointer as an observational surface sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RetainedRollbackObservation {
    pub project_id: String,
    pub current_generation: Option<String>,
    pub rollback_generation: Option<String>,
    pub rollback_verified_at_epoch_ms: i64,
}

/// Observe whether a rollback generation is retained for this project.
///
/// Strictly non-mutating: a status or doctor surface must be able to mention
/// the lever without becoming the thing that migrates, recovers, or activates
/// anything. It deliberately does not validate the candidate — that is
/// [`activate_retained_rollback_generation`]'s job, and doing it here would
/// make an observational call pay for deep artifact validation.
pub fn observe_retained_rollback_generation(
    project_root: &Path,
    storage_path: &Path,
    runtime: &SidecarRuntimeConfig,
) -> Result<Option<RetainedRollbackObservation>> {
    if !storage_path.is_file() {
        return Ok(None);
    }
    let project_id = sidecar_project_id_for_runtime(project_root, runtime)
        .context("resolve project id for retained rollback observation")?;
    let storage = Store::open_observational(storage_path)
        .context("open storage observationally for retained rollback observation")?;
    let Some((current, rollback)) = storage
        .get_retrieval_index_publication(&project_id)
        .context("load the retrieval publication pair")?
    else {
        return Ok(None);
    };
    let Some(rollback) = rollback else {
        return Ok(None);
    };
    Ok(Some(RetainedRollbackObservation {
        project_id,
        current_generation: current.sidecar_generation,
        rollback_generation: rollback.manifest.sidecar_generation,
        rollback_verified_at_epoch_ms: rollback.verified_at_epoch_ms,
    }))
}

/// Validate and (when `apply`) activate the retained rollback generation.
///
/// With `apply == false` every check runs and nothing is written, so an
/// observational surface can report whether the lever would work without
/// becoming a mutating surface itself.
pub fn activate_retained_rollback_generation(
    project_root: &Path,
    storage_path: &Path,
    runtime: &SidecarRuntimeConfig,
    apply: bool,
) -> Result<RollbackActivationOutcome, RollbackActivationError> {
    let snapshot = crate::embeddings::embedding_engine_snapshot_for_runtime(runtime);
    activate_retained_rollback_generation_with_embedding(
        project_root,
        storage_path,
        runtime,
        &snapshot.device,
        snapshot.identity.as_ref(),
        apply,
    )
}

pub(crate) fn activate_retained_rollback_generation_with_embedding(
    project_root: &Path,
    storage_path: &Path,
    runtime: &SidecarRuntimeConfig,
    embedding_device: &EmbeddingDeviceReadiness,
    live_identity: Option<&ProductEmbeddingIdentity>,
    apply: bool,
) -> Result<RollbackActivationOutcome, RollbackActivationError> {
    let project_id = sidecar_project_id_for_runtime(project_root, runtime)
        .context("resolve project id for rollback activation")
        .map_err(RollbackActivationError::Failed)?;
    let validated = validate_retained_rollback(
        storage_path,
        runtime,
        embedding_device,
        live_identity,
        &project_id,
    )?;
    let mut outcome = RollbackActivationOutcome {
        project_id: project_id.clone(),
        applied: false,
        previous_generation: validated.current.sidecar_generation.clone(),
        previous_semantic_generation: validated.current.semantic_generation.clone(),
        activated_generation: validated
            .rollback
            .manifest
            .sidecar_generation
            .clone()
            .unwrap_or_default(),
        activated_semantic_generation: validated.rollback.manifest.semantic_generation.clone(),
        activated_built_at_epoch_ms: validated.rollback.manifest.built_at_epoch_ms,
        rollback_verified_at_epoch_ms: validated.rollback.verified_at_epoch_ms,
        rollback_pointer_retained: !apply,
        activated_retrieval_mode: validated.retrieval_mode,
    };
    if !apply {
        return Ok(outcome);
    }

    let mut storage = Store::open(storage_path)
        .context("open storage to activate the retained rollback generation")
        .map_err(RollbackActivationError::Failed)?;
    let mut publication = storage
        .write_transaction()
        .context("lock the retrieval publication for rollback activation")
        .map_err(RollbackActivationError::Failed)?;
    let observed = publication
        .storage()
        .get_retrieval_index_publication(&project_id)
        .context("re-read the retrieval publication under the activation transaction")
        .map_err(RollbackActivationError::Failed)?;
    match observed {
        Some((current, Some(rollback)))
            if current == validated.current && rollback == validated.rollback => {}
        _ => return Err(RollbackActivationRefusal::PublicationRaced.into()),
    }
    publication
        .storage_mut()
        .publish_retrieval_index_publication(&validated.rollback.manifest, None)
        .context("write the activated rollback generation as the current pointer")
        .map_err(RollbackActivationError::Failed)?;
    publication
        .finish()
        .context("commit the rollback activation")
        .map_err(RollbackActivationError::Failed)?;
    refresh_retention_marker_after_activation(
        &storage,
        runtime,
        project_root,
        &project_id,
        &validated.rollback.manifest,
    );
    outcome.applied = true;
    outcome.rollback_pointer_retained = false;
    Ok(outcome)
}

struct ValidatedRollback {
    current: RetrievalIndexManifest,
    rollback: RetrievalIndexRollbackRecord,
    retrieval_mode: String,
}

fn validate_retained_rollback(
    storage_path: &Path,
    runtime: &SidecarRuntimeConfig,
    embedding_device: &EmbeddingDeviceReadiness,
    live_identity: Option<&ProductEmbeddingIdentity>,
    project_id: &str,
) -> Result<ValidatedRollback, RollbackActivationError> {
    // Validation is strictly observational: an operator inspecting the lever
    // must never up-migrate or recover a database as a side effect.
    let storage = Store::open_observational(storage_path)
        .context("open storage observationally for rollback validation")
        .map_err(RollbackActivationError::Failed)?;
    let publication = storage
        .get_retrieval_index_publication(project_id)
        .context("load the retrieval publication pair")
        .map_err(RollbackActivationError::Failed)?;
    let Some((current, rollback)) = publication else {
        return Err(RollbackActivationRefusal::NoPublication.into());
    };
    let Some(rollback) = rollback else {
        return Err(RollbackActivationRefusal::NoRetainedRollback.into());
    };
    let candidate = rollback.manifest.clone();
    if !manifest_has_current_sidecar_contract(project_id, &candidate) {
        return Err(RollbackActivationRefusal::RollbackContractMissing {
            sidecar_generation: candidate.sidecar_generation.clone(),
        }
        .into());
    }
    let core_publication = storage
        .get_complete_index_publication()
        .context("load the complete core publication for rollback validation")
        .map_err(RollbackActivationError::Failed)?;
    let Some(core_publication) = core_publication else {
        return Err(RollbackActivationRefusal::CorePublicationIncomplete.into());
    };
    crate::embedded_vector::validate_generation_evidence_for_publication(
        &runtime.layout,
        &storage,
        &candidate,
        &core_publication,
        runtime,
        embedding_device,
        live_identity,
    )
    .map_err(|error| RollbackActivationRefusal::RollbackEvidenceInvalid {
        reason: format!("{error:#}"),
    })?;
    let status = probe_sidecar_health_for_runtime(
        &runtime.layout,
        project_id,
        Some(candidate.clone()),
        embedding_device,
        runtime,
    );
    if !status.is_live_ready() {
        return Err(RollbackActivationRefusal::RollbackNotLiveReady {
            retrieval_mode: status.retrieval_mode,
            degraded_reason: status.degraded_reason,
        }
        .into());
    }
    Ok(ValidatedRollback {
        current,
        rollback,
        retrieval_mode: status.retrieval_mode,
    })
}

/// Re-derive the generation retention marker from the committed publication.
///
/// A stale marker only ever over-protects generations, so a failure here is a
/// warning rather than a failed activation: the pointer swap is already
/// durable and correct.
fn refresh_retention_marker_after_activation(
    storage: &Store,
    runtime: &SidecarRuntimeConfig,
    project_root: &Path,
    project_id: &str,
    activated: &RetrievalIndexManifest,
) {
    let Some(workspace_id) = runtime
        .project_identity
        .as_ref()
        .map(|identity| identity.workspace_id.as_str())
    else {
        return;
    };
    if let Err(error) = crate::index::publish_derived_retention_marker(
        storage,
        &runtime.layout,
        workspace_id,
        project_root,
        project_id,
    ) {
        warn!(
            project_id = %project_id,
            sidecar_generation = ?activated.sidecar_generation,
            error = %format!("{error:#}"),
            "rollback activation committed but the retention marker was not refreshed"
        );
    }
}

// The fixtures below need a device the product would admit for full
// retrieval, which only exists under `test-support`; the per-PR lane runs this
// module with that feature explicitly.
#[cfg(all(test, feature = "test-support"))]
mod tests {
    use super::*;
    use crate::test_support::{env_lock, publish_zero_dense_pinned_query_fixture};
    use codestory_store::{IndexPublicationMode, IndexPublicationRecord};
    use std::io::Write as _;
    use tempfile::TempDir;

    struct Fixture {
        _project: TempDir,
        _storage_dir: TempDir,
        _cache: TempDir,
        project_root: std::path::PathBuf,
        storage_path: std::path::PathBuf,
        runtime: SidecarRuntimeConfig,
        current: RetrievalIndexManifest,
        rollback: RetrievalIndexRollbackRecord,
    }

    /// Publish two distinct live-ready retrieval generations against one core
    /// publication and arm the older one as the retained rollback pointer.
    fn publish_current_over_rollback() -> Fixture {
        let project = TempDir::new().expect("project");
        let storage_dir = TempDir::new().expect("storage");
        let cache = TempDir::new().expect("cache");
        let storage_path = storage_dir.path().join("codestory.db");
        std::fs::write(project.path().join("lib.rs"), "pub fn alpha() {}\n").expect("seed source");
        let mut store = Store::open(&storage_path).expect("open storage");
        let publication = IndexPublicationRecord {
            generation: 1,
            generation_id: "11111111-1111-4111-8111-111111111111".into(),
            run_id: "run-one".into(),
            mode: IndexPublicationMode::Full,
            published_at_epoch_ms: 1,
        };
        crate::test_support::publish_complete_core_fixture(
            &mut store,
            project.path(),
            &publication,
        )
        .expect("publish complete core fixture");
        drop(store);
        let runtime = crate::config::with_test_cache_root(cache.path(), || {
            SidecarRuntimeConfig::for_project_profile(
                Some(project.path()),
                crate::SidecarProfile::Local,
            )
        });
        let older =
            publish_zero_dense_pinned_query_fixture(project.path(), &storage_path, &runtime)
                .expect("publish rollback candidate generation");
        // A second lexical input over the same core publication produces a
        // distinct sidecar generation, which is exactly the shape retention
        // retains: two generations bound to one core publication.
        std::fs::write(project.path().join("beta.rs"), "pub fn beta() {}\n")
            .expect("second source");
        let newer =
            publish_zero_dense_pinned_query_fixture(project.path(), &storage_path, &runtime)
                .expect("publish current generation");
        assert_ne!(
            older.sidecar_generation, newer.sidecar_generation,
            "fixture must produce two distinct generations"
        );
        let rollback = RetrievalIndexRollbackRecord {
            manifest: older,
            verified_at_epoch_ms: 4_242,
        };
        let mut store = Store::open(&storage_path).expect("open storage for pointer pair");
        store
            .publish_retrieval_index_publication(&newer, Some(&rollback))
            .expect("arm the retained rollback pointer");
        drop(store);
        Fixture {
            project_root: project.path().to_path_buf(),
            storage_path,
            runtime,
            current: newer,
            rollback,
            _project: project,
            _storage_dir: storage_dir,
            _cache: cache,
        }
    }

    fn activate(
        fixture: &Fixture,
        apply: bool,
    ) -> Result<RollbackActivationOutcome, RollbackActivationError> {
        let device = crate::embeddings::embedding_device_readiness_for_runtime(&fixture.runtime);
        activate_retained_rollback_generation_with_embedding(
            &fixture.project_root,
            &fixture.storage_path,
            &fixture.runtime,
            &device,
            None,
            apply,
        )
    }

    fn stored_publication(
        fixture: &Fixture,
    ) -> (RetrievalIndexManifest, Option<RetrievalIndexRollbackRecord>) {
        let store = Store::open_observational(&fixture.storage_path).expect("open storage");
        store
            .get_retrieval_index_publication(&fixture.current.project_id)
            .expect("read publication")
            .expect("publication exists")
    }

    #[test]
    fn activation_promotes_the_verified_rollback_generation_and_consumes_the_pointer() {
        let _env = env_lock();
        let fixture = publish_current_over_rollback();

        let outcome = activate(&fixture, true).expect("verified rollback must activate");

        assert!(outcome.applied, "activation must report that it wrote");
        assert_eq!(
            outcome.activated_generation,
            fixture
                .rollback
                .manifest
                .sidecar_generation
                .clone()
                .expect("rollback generation")
        );
        assert_eq!(
            outcome.previous_generation,
            fixture.current.sidecar_generation
        );
        assert_eq!(outcome.activated_retrieval_mode, "full");
        let (current, rollback) = stored_publication(&fixture);
        assert_eq!(
            current, fixture.rollback.manifest,
            "the retained rollback generation must become the current pointer"
        );
        assert_eq!(
            rollback, None,
            "activation must consume the retained rollback pointer"
        );
    }

    #[test]
    fn validation_only_activation_never_writes_the_publication() {
        let _env = env_lock();
        let fixture = publish_current_over_rollback();

        let outcome = activate(&fixture, false).expect("validation-only run must succeed");

        assert!(
            !outcome.applied,
            "validation-only run must not report a write"
        );
        assert!(
            outcome.rollback_pointer_retained,
            "validation-only run must leave the retained pointer armed"
        );
        let (current, rollback) = stored_publication(&fixture);
        assert_eq!(
            current, fixture.current,
            "validation-only run must leave the current pointer alone"
        );
        assert_eq!(
            rollback,
            Some(fixture.rollback.clone()),
            "validation-only run must leave the rollback pointer alone"
        );
    }

    #[test]
    fn corrupt_rollback_vector_bytes_refuse_activation_and_keep_the_current_generation() {
        let _env = env_lock();
        let fixture = publish_current_over_rollback();
        let vector_path = crate::embedded_vector::index_path(
            &fixture.runtime.layout,
            &fixture.rollback.manifest.semantic_generation,
        );
        crate::copy_on_write::make_file_owner_writable(&vector_path)
            .expect("make retained rollback vector database writable for corruption");
        std::fs::OpenOptions::new()
            .append(true)
            .open(&vector_path)
            .expect("open retained rollback vector database")
            .write_all(b"corrupt")
            .expect("corrupt retained rollback vector bytes");

        let error = activate(&fixture, true).expect_err("corrupt rollback must be refused");

        assert_eq!(error.code(), "rollback_evidence_invalid", "{error}");
        let (current, rollback) = stored_publication(&fixture);
        assert_eq!(
            current, fixture.current,
            "a refused activation must leave the current pointer alone"
        );
        assert_eq!(
            rollback,
            Some(fixture.rollback.clone()),
            "a refused activation must leave the retained pointer alone"
        );
    }

    #[test]
    fn missing_rollback_lexical_shard_refuses_activation_as_not_live_ready() {
        let _env = env_lock();
        let fixture = publish_current_over_rollback();
        let shard = crate::lexical_index::shard_dir_for(
            &fixture.runtime.layout.lexical_data_dir,
            fixture
                .rollback
                .manifest
                .sidecar_generation
                .as_deref()
                .expect("rollback generation"),
        );
        let index = shard.join(crate::lexical_index::LEXICAL_INDEX_FILE);
        crate::lexical_index::make_test_file_writable(&index);
        std::fs::remove_file(&index).expect("remove retained rollback lexical shard");

        let error = activate(&fixture, true).expect_err("unusable rollback must be refused");

        assert_eq!(error.code(), "rollback_not_live_ready", "{error}");
        let (current, rollback) = stored_publication(&fixture);
        assert_eq!(current, fixture.current);
        assert_eq!(rollback, Some(fixture.rollback.clone()));
    }

    #[test]
    fn observation_reports_the_retained_pointer_and_leaves_the_cache_untouched() {
        let _env = env_lock();
        let fixture = publish_current_over_rollback();
        let before = std::fs::read(&fixture.storage_path).expect("read cache before observing");

        let observed = observe_retained_rollback_generation(
            &fixture.project_root,
            &fixture.storage_path,
            &fixture.runtime,
        )
        .expect("observation must succeed")
        .expect("a retained rollback must be observed");

        assert_eq!(observed.project_id, fixture.current.project_id);
        assert_eq!(
            observed.current_generation,
            fixture.current.sidecar_generation
        );
        assert_eq!(
            observed.rollback_generation,
            fixture.rollback.manifest.sidecar_generation
        );
        assert_eq!(observed.rollback_verified_at_epoch_ms, 4_242);
        assert_eq!(
            std::fs::read(&fixture.storage_path).expect("read cache after observing"),
            before,
            "an observational read must not change a byte of the cache"
        );

        let mut store = Store::open(&fixture.storage_path).expect("open storage");
        store
            .publish_retrieval_index_publication(&fixture.current, None)
            .expect("clear the retained rollback pointer");
        drop(store);
        assert_eq!(
            observe_retained_rollback_generation(
                &fixture.project_root,
                &fixture.storage_path,
                &fixture.runtime,
            )
            .expect("observation must succeed"),
            None,
            "with no retained pointer there is no lever to report"
        );
    }

    #[test]
    fn a_publication_without_a_retained_rollback_refuses_activation() {
        let _env = env_lock();
        let fixture = publish_current_over_rollback();
        let mut store = Store::open(&fixture.storage_path).expect("open storage");
        store
            .publish_retrieval_index_publication(&fixture.current, None)
            .expect("clear the retained rollback pointer");
        drop(store);

        let error = activate(&fixture, true).expect_err("missing rollback must be refused");

        assert_eq!(error.code(), "no_retained_rollback", "{error}");
        let (current, rollback) = stored_publication(&fixture);
        assert_eq!(current, fixture.current);
        assert_eq!(rollback, None);
    }
}
