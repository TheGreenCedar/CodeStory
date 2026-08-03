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

/// Observational retrieval diagnostics for one project's storage.
///
/// `engine` and `embedding_server` stay serialized: the retrieval crate owns
/// their shapes and the adapter only forwards them, so no retrieval type
/// crosses the runtime boundary.
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
        match codestory_retrieval::probe_process_start_identity(std::process::id()) {
            codestory_retrieval::ProcessStartProbe::Running { start_identity } => {
                Some(start_identity)
            }
            codestory_retrieval::ProcessStartProbe::NotRunning
            | codestory_retrieval::ProcessStartProbe::Unknown { .. } => None,
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
        let status = codestory_retrieval::strict_sidecar_status_for_runtime(
            project_root,
            Some(storage_path),
            runtime_config.clone(),
        )
        .map_err(|error| {
            RetrievalEngineDiagnosticsError::new(
                RetrievalEngineDiagnosticsStage::RetrievalStatus,
                error.to_string(),
            )
        })?;
        Ok(RetrievalEngineDiagnostics {
            retrieval_mode: status.retrieval_mode,
            degraded_reason: status.degraded_reason,
            engine,
            embedding_server,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Runtime;
    use std::fs;
    use std::path::PathBuf;

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
