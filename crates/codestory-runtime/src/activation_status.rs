//! Runtime-owned answers for the transport status and diagnostics surfaces.
//!
//! `codestory-cli`'s stdio transport used to reach past the runtime and probe
//! `codestory_retrieval` itself for the sidecar contract version, the active
//! embedding backend identity, the observational engine diagnostics, and this
//! process's start identity. The transport is an adapter: it now asks
//! [`ActivationService`] and only shapes the answers into wire JSON.
//!
//! This module moves ownership, not behavior. Every probe keeps the same
//! arguments, the same order, and the same failure text it had in the
//! transport, so the wire payloads built from these answers are unchanged.

use std::fmt;
use std::path::Path;

use crate::services::ActivationService;
use crate::{ProcessOwnerState, ProcessStartProbe, RetrievalStatusReport, RuntimeRetrievalProfile};

/// Ready-lease evidence retained by the runtime for observational surfaces.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ReadyLeaseEvidence {
    pub ready_lease_present: bool,
    pub ready_lease_admission_basis: String,
    pub ready_lease_observer_epoch_coherence: String,
    pub ready_lease_memo_holds_observations: bool,
}

impl ReadyLeaseEvidence {
    pub(crate) fn absent() -> Self {
        Self {
            ready_lease_present: false,
            ready_lease_admission_basis: "none".to_string(),
            ready_lease_observer_epoch_coherence: "not_applicable".to_string(),
            ready_lease_memo_holds_observations: false,
        }
    }
}

impl Default for ReadyLeaseEvidence {
    fn default() -> Self {
        Self::absent()
    }
}

/// Observe one PID's platform start identity without collapsing uncertainty.
#[must_use]
pub fn process_start_identity(pid: u32) -> ProcessStartProbe {
    codestory_retrieval::probe_process_start_identity(pid)
}

/// Decide whether one PID still owns a persisted optional start identity.
#[must_use]
pub fn process_owner_state(pid: u32, expected_start_identity: Option<&str>) -> ProcessOwnerState {
    let probe = process_start_identity(pid);
    process_owner_state_for_probe(&probe, expected_start_identity)
}

fn process_owner_state_for_probe(
    probe: &ProcessStartProbe,
    expected_start_identity: Option<&str>,
) -> ProcessOwnerState {
    codestory_retrieval::process_owner_state(probe, expected_start_identity)
}

fn collapsed_process_start_identity(probe: ProcessStartProbe) -> Option<String> {
    match probe {
        ProcessStartProbe::Running { start_identity } => Some(start_identity),
        ProcessStartProbe::NotRunning | ProcessStartProbe::Unknown { .. } => None,
    }
}

/// Effective retrieval namespace selected for one status observation.
///
/// The run ID is the retrieval-owned effective value, not merely the adapter's
/// request. In particular, an Agent observation without an explicit run ID
/// carries the shared Agent run ID selected by retrieval configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetrievalStatusSelection {
    profile: RuntimeRetrievalProfile,
    run_id: Option<String>,
}

impl RetrievalStatusSelection {
    fn from_runtime(runtime: &codestory_retrieval::SidecarRuntimeConfig) -> Self {
        Self {
            profile: runtime.profile.into(),
            run_id: runtime.run_id.clone(),
        }
    }

    /// Effective Local or Agent profile.
    #[must_use]
    pub fn profile(&self) -> RuntimeRetrievalProfile {
        self.profile
    }

    /// Effective run ID, including retrieval's default Agent run ID.
    #[must_use]
    pub fn run_id(&self) -> Option<&str> {
        self.run_id.as_deref()
    }
}

/// Strict retrieval status paired with its effective namespace selection.
#[derive(Debug, Clone)]
pub struct RetrievalStatusObservation {
    selection: RetrievalStatusSelection,
    report: RetrievalStatusReport,
    ready_lease: ReadyLeaseEvidence,
}

impl RetrievalStatusObservation {
    /// Effective selection used by the strict probe.
    #[must_use]
    pub fn selection(&self) -> &RetrievalStatusSelection {
        &self.selection
    }

    /// Strict retrieval report.
    #[must_use]
    pub fn report(&self) -> &RetrievalStatusReport {
        &self.report
    }

    /// Evidence retained by the matching ready lease at observation time.
    #[must_use]
    pub fn ready_lease(&self) -> &ReadyLeaseEvidence {
        &self.ready_lease
    }

    /// Consume the observation without losing its effective selection.
    #[must_use]
    pub fn into_parts(self) -> (RetrievalStatusSelection, RetrievalStatusReport) {
        (self.selection, self.report)
    }
}

/// Strict retrieval observation failure with the selection that was probed.
#[derive(Debug)]
pub struct RetrievalStatusObservationError {
    selection: RetrievalStatusSelection,
    ready_lease: ReadyLeaseEvidence,
    source: anyhow::Error,
}

impl RetrievalStatusObservationError {
    /// Effective selection used by the failed strict probe.
    #[must_use]
    pub fn selection(&self) -> &RetrievalStatusSelection {
        &self.selection
    }

    /// Evidence retained by the matching ready lease at observation time.
    #[must_use]
    pub fn ready_lease(&self) -> &ReadyLeaseEvidence {
        &self.ready_lease
    }

    /// Recover the original retrieval error and its complete context chain.
    #[must_use]
    pub fn into_parts(self) -> (RetrievalStatusSelection, anyhow::Error) {
        (self.selection, self.source)
    }
}

impl fmt::Display for RetrievalStatusObservationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.source, formatter)
    }
}

impl std::error::Error for RetrievalStatusObservationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        let source: &(dyn std::error::Error + 'static) = self.source.as_ref();
        Some(source)
    }
}

/// Observational retrieval diagnostics for one project's storage.
///
/// `engine` and `embedding_server` stay serialized: the retrieval crate owns
/// their shapes and the adapter only forwards them without consuming their
/// concrete types.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetrievalEngineDiagnostics {
    /// Strict sidecar retrieval mode for this project.
    pub retrieval_mode: String,
    /// Why the strict status is not fully live, when it is not.
    pub degraded_reason: Option<String>,
    /// Serialized runtime-scoped infrastructure health.
    pub engine: serde_json::Value,
    /// Serialized per-user embedding server snapshot, `null` when no server is
    /// observable, or a typed observation-failure object.
    pub embedding_server: serde_json::Value,
    /// Evidence retained by the matching ready lease.
    pub ready_lease: ReadyLeaseEvidence,
}

/// Which observation failed while building [`RetrievalEngineDiagnostics`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetrievalEngineDiagnosticsStage {
    /// Serializing the runtime-scoped infrastructure health probe.
    EngineHealth,
    /// Serializing an observed per-user embedding server snapshot.
    EmbeddingServerSnapshot,
    /// Reading the strict sidecar retrieval status.
    RetrievalStatus,
}

impl RetrievalEngineDiagnosticsStage {
    /// Stable identifier for logs and assertions.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::EngineHealth => "engine_health",
            Self::EmbeddingServerSnapshot => "embedding_server_snapshot",
            Self::RetrievalStatus => "retrieval_status",
        }
    }
}

/// A typed diagnostics failure.
///
/// `Display` is exactly the underlying failure text the transport used to
/// propagate, so the diagnostics resource error payload is unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetrievalEngineDiagnosticsError {
    stage: RetrievalEngineDiagnosticsStage,
    message: String,
}

impl RetrievalEngineDiagnosticsError {
    fn new(stage: RetrievalEngineDiagnosticsStage, message: String) -> Self {
        Self { stage, message }
    }

    /// The observation that failed.
    #[must_use]
    pub fn stage(&self) -> RetrievalEngineDiagnosticsStage {
        self.stage
    }

    /// The failure text, identical to the pre-move transport error text.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for RetrievalEngineDiagnosticsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for RetrievalEngineDiagnosticsError {}

impl ActivationService {
    /// Sidecar retrieval schema version this runtime speaks.
    #[must_use]
    pub fn retrieval_contract_version(&self) -> i32 {
        codestory_retrieval::SIDECAR_SCHEMA_VERSION
    }

    /// Identity of the embedding backend this runtime configuration activates.
    #[must_use]
    pub fn active_embedding_backend_id(&self) -> String {
        codestory_retrieval::embedding_runtime_id_for_runtime(&self.controller.runtime_config)
    }

    /// Start identity of the calling process, or `None` when the platform
    /// cannot prove it.
    ///
    /// `Unknown` collapses to `None` exactly as the transport did: an
    /// unavailable probe is reported as absent evidence, never as a synthetic
    /// identity.
    #[must_use]
    pub fn host_process_start_identity() -> Option<String> {
        collapsed_process_start_identity(process_start_identity(std::process::id()))
    }

    /// Observe strict retrieval status using this service's pinned runtime
    /// configuration. This starts no server, loads no model, and mutates no
    /// retrieval state.
    pub fn retrieval_status(
        &self,
        project_root: &Path,
        storage_path: &Path,
    ) -> Result<RetrievalStatusObservation, RetrievalStatusObservationError> {
        self.retrieval_status_with_runtime(
            project_root,
            storage_path,
            self.controller.runtime_config.as_ref().clone(),
        )
    }

    /// Observe strict retrieval status for an explicit Local or Agent lane.
    ///
    /// Profile and run-ID selection happens inside runtime so adapters never
    /// reproduce retrieval namespace defaults. The observation remains
    /// read-only.
    pub fn retrieval_status_for_profile(
        &self,
        project_root: &Path,
        storage_path: &Path,
        profile: RuntimeRetrievalProfile,
        run_id: Option<&str>,
    ) -> Result<RetrievalStatusObservation, RetrievalStatusObservationError> {
        let runtime = self.controller.runtime_config.with_profile_and_run_id(
            Some(project_root),
            profile.into(),
            run_id,
        );
        self.retrieval_status_with_runtime(project_root, storage_path, runtime)
    }

    fn retrieval_status_with_runtime(
        &self,
        project_root: &Path,
        storage_path: &Path,
        runtime: codestory_retrieval::SidecarRuntimeConfig,
    ) -> Result<RetrievalStatusObservation, RetrievalStatusObservationError> {
        let selection = RetrievalStatusSelection::from_runtime(&runtime);
        let ready_lease = self.ready_lease_evidence(project_root, storage_path);
        match codestory_retrieval::strict_sidecar_status_for_runtime(
            project_root,
            Some(storage_path),
            runtime,
        ) {
            Ok(report) => Ok(RetrievalStatusObservation {
                selection,
                report,
                ready_lease,
            }),
            Err(source) => Err(RetrievalStatusObservationError {
                selection,
                ready_lease,
                source,
            }),
        }
    }

    /// Observe engine health, the per-user embedding server, and the strict
    /// sidecar retrieval status for one project.
    ///
    /// Purely observational: it starts no server, loads no model, and
    /// finalizes no generation.
    pub fn retrieval_engine_diagnostics(
        &self,
        project_root: &Path,
        storage_path: &Path,
    ) -> Result<RetrievalEngineDiagnostics, RetrievalEngineDiagnosticsError> {
        let runtime_config = self.controller.runtime_config.as_ref();
        let infrastructure = codestory_retrieval::probe_infrastructure_health(runtime_config);
        let engine = serde_json::to_value(&infrastructure).map_err(|error| {
            RetrievalEngineDiagnosticsError::new(
                RetrievalEngineDiagnosticsStage::EngineHealth,
                format!("serialize retrieval infrastructure health: {error}"),
            )
        })?;
        let embedding_server = match codestory_retrieval::PerUserEmbeddingClient::for_runtime(
            runtime_config,
        )
        .and_then(|client| client.observe())
        {
            Ok(Some(snapshot)) => serde_json::to_value(snapshot).map_err(|_| {
                RetrievalEngineDiagnosticsError::new(
                    RetrievalEngineDiagnosticsStage::EmbeddingServerSnapshot,
                    "serialize observational embedding server snapshot".to_string(),
                )
            })?,
            Ok(None) => serde_json::Value::Null,
            Err(error) => serde_json::json!({
                "schema_version": codestory_retrieval::PER_USER_EMBEDDING_SERVER_SNAPSHOT_SCHEMA_VERSION,
                "lifecycle": "unavailable",
                "failure": {
                    "code": "embedding_server_observation_failed",
                    "retry_class": "after_server_change",
                    "retry_after_ms": 0,
                    "retry_condition": "the per-user server lifetime authority changes",
                    "message": error.to_string(),
                }
            }),
        };
        let status = self
            .retrieval_status(project_root, storage_path)
            .map_err(|error| {
                RetrievalEngineDiagnosticsError::new(
                    RetrievalEngineDiagnosticsStage::RetrievalStatus,
                    error.to_string(),
                )
            })?;
        Ok(RetrievalEngineDiagnostics {
            retrieval_mode: status.report.retrieval_mode,
            degraded_reason: status.report.degraded_reason,
            engine,
            embedding_server,
            ready_lease: status.ready_lease,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Runtime;
    use std::fs;
    use std::path::PathBuf;

    #[test]
    fn process_owner_state_matches_retrieval_for_every_probe_and_expected_identity() {
        let probes = [
            (
                "running",
                ProcessStartProbe::Running {
                    start_identity: "start-a".to_string(),
                },
                [
                    ProcessOwnerState::Matching,
                    ProcessOwnerState::GoneOrReused,
                    ProcessOwnerState::Matching,
                ],
            ),
            (
                "unknown",
                ProcessStartProbe::Unknown {
                    reason: "probe failed".to_string(),
                },
                [
                    ProcessOwnerState::Unknown,
                    ProcessOwnerState::Unknown,
                    ProcessOwnerState::Unknown,
                ],
            ),
            (
                "not_running",
                ProcessStartProbe::NotRunning,
                [
                    ProcessOwnerState::GoneOrReused,
                    ProcessOwnerState::GoneOrReused,
                    ProcessOwnerState::GoneOrReused,
                ],
            ),
        ];
        let expected_identities = [
            ("matching", Some("start-a")),
            ("different", Some("start-b")),
            ("absent", None),
        ];
        let mut compared_cells = 0;

        for (probe_name, probe, expected_states) in &probes {
            for ((identity_name, expected_identity), expected_state) in
                expected_identities.iter().zip(expected_states)
            {
                let runtime_state = process_owner_state_for_probe(probe, *expected_identity);
                let retrieval_state =
                    codestory_retrieval::process_owner_state(probe, *expected_identity);
                assert_eq!(
                    runtime_state, retrieval_state,
                    "probe={probe_name} expected={identity_name}"
                );
                assert_eq!(
                    runtime_state, *expected_state,
                    "probe={probe_name} expected={identity_name}"
                );
                compared_cells += 1;
            }
        }

        assert_eq!(compared_cells, 9, "the differential table must stay 3x3");
    }

    #[test]
    fn host_process_start_identity_keeps_collapsed_diagnostic_behavior() {
        assert_eq!(
            collapsed_process_start_identity(ProcessStartProbe::Running {
                start_identity: "start-a".to_string(),
            }),
            Some("start-a".to_string())
        );
        assert_eq!(
            collapsed_process_start_identity(ProcessStartProbe::NotRunning),
            None
        );
        assert_eq!(
            collapsed_process_start_identity(ProcessStartProbe::Unknown {
                reason: "probe failed".to_string(),
            }),
            None
        );
    }

    struct DiagnosticsFixture {
        _project: tempfile::TempDir,
        _cache: tempfile::TempDir,
        project_root: PathBuf,
        storage_path: PathBuf,
        sidecar: codestory_retrieval::SidecarRuntimeConfig,
        runtime: Runtime,
    }

    /// A runtime bound to throwaway project, cache, and sidecar roots.
    ///
    /// Nothing in this crate's test binary installs a per-user embedding client
    /// transport, so `PerUserEmbeddingClient::for_runtime` always fails here:
    /// the observation-failure branch is deterministic, not incidental.
    fn diagnostics_fixture() -> DiagnosticsFixture {
        let project = tempfile::tempdir().expect("project");
        let cache = tempfile::tempdir().expect("cache");
        let sidecar_cache = cache.path().join("sidecar");
        fs::create_dir_all(&sidecar_cache).expect("create sidecar cache");
        let sidecar = codestory_retrieval::with_test_cache_root(&sidecar_cache, || {
            codestory_retrieval::SidecarRuntimeConfig::for_project_profile(
                Some(project.path()),
                codestory_retrieval::SidecarProfile::Agent,
            )
        });
        DiagnosticsFixture {
            project_root: project.path().to_path_buf(),
            storage_path: cache.path().join("codestory.db"),
            runtime: Runtime::new_with_config(sidecar.clone()),
            sidecar,
            _project: project,
            _cache: cache,
        }
    }

    #[test]
    fn diagnostics_observe_the_service_owned_runtime_configuration() {
        let fixture = diagnostics_fixture();

        let diagnostics = fixture
            .runtime
            .activation_service()
            .retrieval_engine_diagnostics(&fixture.project_root, &fixture.storage_path)
            .expect("observational diagnostics");

        // The service must probe the runtime configuration it was built with,
        // not some other default: compare against a direct probe of that exact
        // configuration.
        let expected_engine = serde_json::to_value(
            codestory_retrieval::probe_infrastructure_health(&fixture.sidecar),
        )
        .expect("serialize infrastructure health");
        let expected_status = codestory_retrieval::strict_sidecar_status_for_runtime(
            &fixture.project_root,
            Some(&fixture.storage_path),
            fixture.sidecar.clone(),
        )
        .expect("strict sidecar status");
        assert_eq!(diagnostics.engine, expected_engine);
        assert_eq!(diagnostics.retrieval_mode, expected_status.retrieval_mode);
        assert_eq!(diagnostics.degraded_reason, expected_status.degraded_reason);
    }

    #[test]
    fn status_observation_preserves_default_and_profile_selected_runtime_identity() {
        let fixture = diagnostics_fixture();
        let service = fixture.runtime.activation_service();

        let default = service
            .retrieval_status(&fixture.project_root, &fixture.storage_path)
            .expect("default status observation");
        assert_eq!(
            default.selection(),
            &RetrievalStatusSelection::from_runtime(&fixture.sidecar)
        );
        assert_eq!(
            default.selection().run_id(),
            Some(codestory_retrieval::DEFAULT_AGENT_RUN_ID),
            "an omitted Agent run ID must retain retrieval's effective shared run ID"
        );
        let direct_default = codestory_retrieval::strict_sidecar_status_for_runtime(
            &fixture.project_root,
            Some(&fixture.storage_path),
            fixture.sidecar.clone(),
        )
        .expect("direct default status");
        assert_eq!(
            serde_json::to_value(default.report()).expect("serialize default service status"),
            serde_json::to_value(direct_default).expect("serialize default direct status")
        );

        for (profile, run_id) in [
            (RuntimeRetrievalProfile::Local, None),
            (RuntimeRetrievalProfile::Local, Some("ignored-local-run")),
            (RuntimeRetrievalProfile::Agent, None),
            (RuntimeRetrievalProfile::Agent, Some("explicit-agent-run")),
        ] {
            let selected = fixture.sidecar.with_profile_and_run_id(
                Some(&fixture.project_root),
                profile.into(),
                run_id,
            );
            let observed = service
                .retrieval_status_for_profile(
                    &fixture.project_root,
                    &fixture.storage_path,
                    profile,
                    run_id,
                )
                .expect("profile status observation");
            assert_eq!(
                observed.selection(),
                &RetrievalStatusSelection::from_runtime(&selected),
                "profile={profile:?} run_id={run_id:?} selected the wrong namespace"
            );
            let expected_run_id = match (profile, run_id) {
                (RuntimeRetrievalProfile::Local, _) => None,
                (RuntimeRetrievalProfile::Agent, None) => {
                    Some(codestory_retrieval::DEFAULT_AGENT_RUN_ID)
                }
                (RuntimeRetrievalProfile::Agent, Some(run_id)) => Some(run_id),
            };
            assert_eq!(
                observed.selection().run_id(),
                expected_run_id,
                "profile={profile:?} run_id={run_id:?} changed the effective run ID"
            );
            let direct = codestory_retrieval::strict_sidecar_status_for_runtime(
                &fixture.project_root,
                Some(&fixture.storage_path),
                selected,
            )
            .expect("direct profile status");
            assert_eq!(
                serde_json::to_value(observed.report()).expect("serialize service status"),
                serde_json::to_value(direct).expect("serialize direct status"),
                "profile={profile:?} run_id={run_id:?} changed the strict report"
            );
        }
    }

    #[test]
    fn status_observation_failure_preserves_effective_selection_and_error_chain() {
        let fixture = diagnostics_fixture();
        fs::write(&fixture.storage_path, b"not a sqlite database").expect("write hostile storage");
        let profile = RuntimeRetrievalProfile::Agent;
        let selected = fixture.sidecar.with_profile_and_run_id(
            Some(&fixture.project_root),
            profile.into(),
            None,
        );
        let direct = codestory_retrieval::strict_sidecar_status_for_runtime(
            &fixture.project_root,
            Some(&fixture.storage_path),
            selected.clone(),
        )
        .expect_err("hostile storage must refuse the direct probe");
        let error = fixture
            .runtime
            .activation_service()
            .retrieval_status_for_profile(
                &fixture.project_root,
                &fixture.storage_path,
                profile,
                None,
            )
            .expect_err("hostile storage must refuse the service probe");

        assert_eq!(
            error.selection(),
            &RetrievalStatusSelection::from_runtime(&selected)
        );
        assert_eq!(error.to_string(), direct.to_string());
        assert_eq!(
            std::error::Error::source(&error).map(ToString::to_string),
            Some(direct.to_string()),
            "the standard Error chain must start with the underlying retrieval failure"
        );
        let (_, source) = error.into_parts();
        assert_eq!(
            source.chain().map(ToString::to_string).collect::<Vec<_>>(),
            direct.chain().map(ToString::to_string).collect::<Vec<_>>(),
            "runtime must preserve the retrieval error chain byte for byte"
        );
    }

    #[test]
    fn unobservable_embedding_server_keeps_its_typed_unavailable_object() {
        let fixture = diagnostics_fixture();

        let diagnostics = fixture
            .runtime
            .activation_service()
            .retrieval_engine_diagnostics(&fixture.project_root, &fixture.storage_path)
            .expect("observational diagnostics");

        assert_eq!(
            diagnostics.embedding_server,
            serde_json::json!({
                "schema_version": codestory_retrieval::PER_USER_EMBEDDING_SERVER_SNAPSHOT_SCHEMA_VERSION,
                "lifecycle": "unavailable",
                "failure": {
                    "code": "embedding_server_observation_failed",
                    "retry_class": "after_server_change",
                    "retry_after_ms": 0,
                    "retry_condition": "the per-user server lifetime authority changes",
                    "message": "embedding_server_transport_unavailable",
                }
            }),
            "an unobservable per-user server must stay a typed unavailable snapshot"
        );
    }

    #[test]
    fn strict_status_failures_reach_the_wire_as_their_bare_observation_text() {
        let fixture = diagnostics_fixture();
        fs::write(&fixture.storage_path, b"not a sqlite database").expect("write hostile storage");

        let error = fixture
            .runtime
            .activation_service()
            .retrieval_engine_diagnostics(&fixture.project_root, &fixture.storage_path)
            .expect_err("hostile storage must refuse");
        let direct = codestory_retrieval::strict_sidecar_status_for_runtime(
            &fixture.project_root,
            Some(&fixture.storage_path),
            fixture.sidecar.clone(),
        )
        .expect_err("hostile storage must refuse the direct probe too");

        assert_eq!(
            error.stage(),
            RetrievalEngineDiagnosticsStage::RetrievalStatus
        );
        // The transport renders this failure with `error.to_string()`. Any
        // wrapping (an error code prefix, a recovery block) would change the
        // diagnostics resource payload.
        assert_eq!(error.to_string(), direct.to_string());
        assert_eq!(anyhow::Error::new(error).to_string(), direct.to_string());
    }
}
