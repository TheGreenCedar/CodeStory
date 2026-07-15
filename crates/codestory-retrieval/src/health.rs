use crate::capabilities::SidecarCapabilities;
use crate::config::{SidecarLayout, SidecarRuntimeConfig};
use crate::embedded_vector::EmbeddedVectorIndex;
use crate::embeddings::{EmbeddingDeviceReadiness, manifest_embedding_backend_is_product};
use crate::generation::{manifest_has_current_sidecar_contract, manifest_sidecar_generation};
use crate::scip_client::{ScipAvailability, ScipClient};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComponentStatus {
    Healthy,
    Degraded,
    Unavailable,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentHealth {
    pub name: String,
    pub status: ComponentStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
    pub detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub degraded_reason: Option<String>,
    #[serde(default, skip_serializing_if = "capabilities_are_empty")]
    pub capabilities: SidecarCapabilities,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetrievalManifestLaneProvenance {
    pub lane: String,
    pub producer: String,
    pub provenance: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<i64>,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetrievalManifestContractReport {
    pub source_root: String,
    pub project_id: String,
    pub input_hash: Option<String>,
    pub generation: Option<String>,
    pub schema_version: Option<i32>,
    pub graph_hash: Option<String>,
    pub symbol_doc_count: Option<i64>,
    pub dense_anchor_count: Option<i64>,
    pub degraded_modes: Vec<String>,
    pub retrieval_mode: String,
    pub degraded_reason: Option<String>,
    pub lanes: Vec<RetrievalManifestLaneProvenance>,
}

fn capabilities_are_empty(cap: &SidecarCapabilities) -> bool {
    !cap.lexical && !cap.semantic && !cap.graph
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetrievalStatusReport {
    pub retrieval_mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub degraded_reason: Option<String>,
    pub query_embedding_backend: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manifest_vector_embedding_backend: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manifest_vector_embedding_dim: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stored_doc_vector_producer_backend: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stored_doc_vector_dim: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stored_doc_vector_mixed_backends: Option<bool>,
    pub embedding_device_policy: String,
    pub embedding_device_state: String,
    pub embedding_device_observation_source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding_detected_provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding_detected_gpu: Option<String>,
    #[serde(default)]
    pub embedding_accelerator_requested: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding_accelerator_request_provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding_accelerator_request_device: Option<String>,
    pub embedding_cpu_allowed: bool,
    pub lexical: ComponentHealth,
    pub semantic: ComponentHealth,
    pub scip: ComponentHealth,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manifest_contract: Option<RetrievalManifestContractReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manifest: Option<codestory_store::RetrievalIndexManifest>,
}

impl RetrievalStatusReport {
    /// Whether the persisted full-retrieval classification is also live and usable now.
    pub fn is_live_ready(&self) -> bool {
        self.retrieval_mode == "full" && self.degraded_reason.is_none()
    }
}

pub fn attach_manifest_contract(
    mut report: RetrievalStatusReport,
    source_root: &Path,
) -> RetrievalStatusReport {
    report.manifest_contract = report
        .manifest
        .as_ref()
        .map(|manifest| manifest_contract_report(source_root, &report, manifest));
    report
}

fn manifest_contract_report(
    source_root: &Path,
    report: &RetrievalStatusReport,
    manifest: &codestory_store::RetrievalIndexManifest,
) -> RetrievalManifestContractReport {
    let generation = manifest
        .sidecar_generation
        .clone()
        .unwrap_or_else(|| "generation_missing".into());
    let input_hash = manifest
        .sidecar_input_hash
        .clone()
        .unwrap_or_else(|| "input_hash_missing".into());
    let mut lanes = vec![
        RetrievalManifestLaneProvenance {
            lane: "lexical".into(),
            producer: manifest.lexical_version.clone(),
            provenance: format!("sidecar_generation:{generation}"),
            count: None,
            status: component_status_label(&report.lexical),
        },
        RetrievalManifestLaneProvenance {
            lane: "symbol_docs".into(),
            producer: "codestory-symbol-doc".into(),
            provenance: format!("sidecar_input_hash:{input_hash}"),
            count: manifest.symbol_doc_count,
            status: count_contract_status(manifest.symbol_doc_count),
        },
        RetrievalManifestLaneProvenance {
            lane: "semantic_dense".into(),
            producer: manifest
                .embedding_backend
                .clone()
                .unwrap_or_else(|| "embedding_backend_missing".into()),
            provenance: format!("vector_generation:{}", manifest.semantic_generation),
            count: manifest.dense_projection_count,
            status: component_status_label(&report.semantic),
        },
        RetrievalManifestLaneProvenance {
            lane: "graph".into(),
            producer: manifest
                .scip_revision
                .clone()
                .unwrap_or_else(|| "scip_revision_missing".into()),
            provenance: format!("graph_artifact_hash:{}", graph_hash_label(manifest)),
            count: None,
            status: component_status_label(&report.scip),
        },
    ];
    if let Some(status) = manifest.precise_semantic_import_status.as_ref() {
        lanes.push(RetrievalManifestLaneProvenance {
            lane: "precise_semantic_import".into(),
            producer: manifest
                .precise_semantic_import_producer
                .clone()
                .unwrap_or_else(|| "producer_missing".into()),
            provenance: manifest
                .precise_semantic_import_revision
                .as_ref()
                .map(|revision| format!("precise_semantic_import_revision:{revision}"))
                .or_else(|| {
                    manifest
                        .precise_semantic_import_reason
                        .as_ref()
                        .map(|reason| format!("precise_semantic_import_reason:{reason}"))
                })
                .unwrap_or_else(|| "precise_semantic_import_unconfigured".into()),
            count: None,
            status: status.clone(),
        });
    }
    RetrievalManifestContractReport {
        source_root: source_root.display().to_string(),
        project_id: manifest.project_id.clone(),
        input_hash: manifest.sidecar_input_hash.clone(),
        generation: manifest.sidecar_generation.clone(),
        schema_version: manifest.sidecar_schema_version,
        graph_hash: manifest.graph_artifact_hash.clone(),
        symbol_doc_count: manifest.symbol_doc_count,
        dense_anchor_count: manifest.dense_projection_count,
        degraded_modes: parse_degraded_modes(manifest),
        retrieval_mode: report.retrieval_mode.clone(),
        degraded_reason: report.degraded_reason.clone(),
        lanes,
    }
}

fn parse_degraded_modes(manifest: &codestory_store::RetrievalIndexManifest) -> Vec<String> {
    serde_json::from_str::<Vec<String>>(&manifest.degraded_modes_json)
        .unwrap_or_else(|_| vec!["degraded_modes_json_invalid".into()])
}

fn manifest_classifies_full(manifest: &codestory_store::RetrievalIndexManifest) -> bool {
    manifest_has_current_sidecar_contract(&manifest.project_id, manifest)
        && parse_degraded_modes(manifest).is_empty()
}

fn component_status_label(component: &ComponentHealth) -> String {
    if let Some(reason) = component.degraded_reason.as_ref() {
        return reason.clone();
    }
    match component.status {
        ComponentStatus::Healthy => "ready",
        ComponentStatus::Degraded => "degraded",
        ComponentStatus::Unavailable => "unavailable",
    }
    .into()
}

fn count_contract_status(count: Option<i64>) -> String {
    if count.is_some() {
        "ready".into()
    } else {
        "missing_contract".into()
    }
}

fn graph_hash_label(manifest: &codestory_store::RetrievalIndexManifest) -> String {
    manifest
        .graph_artifact_hash
        .clone()
        .unwrap_or_else(|| "graph_hash_missing".into())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InfrastructureHealth {
    pub embed_reachable: bool,
    pub embedding_device_policy: String,
    pub embedding_device_state: String,
    pub embedding_device_observation_source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding_detected_provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding_detected_gpu: Option<String>,
    #[serde(default)]
    pub embedding_accelerator_requested: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding_accelerator_request_provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding_accelerator_request_device: Option<String>,
    pub embedding_cpu_allowed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding_model_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding_ggml_build_identity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding_backend: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding_adapter: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding_adapter_description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding_execution_devices: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding_policy: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding_engine_instance_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding_model_load_count: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding_smoke_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding_initialization_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding_materialized_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding_materialized_reused: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding_accelerator_execution_verified: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding_adapter_memory_total: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding_adapter_memory_used_by_load: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding_model_layer_count: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding_offloaded_layer_count: Option<u32>,
    pub embed_detail: String,
}

fn unavailable_component(name: &str, reason: &str) -> ComponentHealth {
    ComponentHealth {
        name: name.into(),
        status: ComponentStatus::Unavailable,
        latency_ms: None,
        detail: reason.into(),
        degraded_reason: Some(reason.into()),
        capabilities: SidecarCapabilities::NONE,
    }
}

pub fn unavailable_status_report_with_embedding_device(
    reason: impl Into<String>,
    manifest: Option<codestory_store::RetrievalIndexManifest>,
    embedding_device: &EmbeddingDeviceReadiness,
) -> RetrievalStatusReport {
    let reason = reason.into();
    let retrieval_mode = manifest
        .as_ref()
        .filter(|manifest| manifest_classifies_full(manifest))
        .map_or("unavailable", |_| "full");
    let manifest_vector_embedding_backend = manifest
        .as_ref()
        .and_then(|manifest| manifest.embedding_backend.clone());
    let manifest_vector_embedding_dim = manifest
        .as_ref()
        .and_then(|manifest| manifest.embedding_dim);
    RetrievalStatusReport {
        retrieval_mode: retrieval_mode.into(),
        degraded_reason: Some(reason.clone()),
        query_embedding_backend: crate::embeddings::embedding_runtime_id(),
        manifest_vector_embedding_backend,
        manifest_vector_embedding_dim,
        stored_doc_vector_producer_backend: None,
        stored_doc_vector_dim: None,
        stored_doc_vector_mixed_backends: None,
        embedding_device_policy: embedding_device.requested_policy.into(),
        embedding_device_state: embedding_device.observed_state.into(),
        embedding_device_observation_source: embedding_device.observation_source.into(),
        embedding_detected_provider: embedding_device.detected_provider.clone(),
        embedding_detected_gpu: embedding_device.detected_gpu.clone(),
        embedding_accelerator_requested: embedding_device.accelerator_requested,
        embedding_accelerator_request_provider: embedding_device
            .accelerator_request_provider
            .clone(),
        embedding_accelerator_request_device: embedding_device.accelerator_request_device.clone(),
        embedding_cpu_allowed: embedding_device.cpu_allowed,
        lexical: unavailable_component("lexical", &reason),
        semantic: unavailable_component("semantic", &reason),
        scip: unavailable_component("scip", &reason),
        manifest_contract: None,
        manifest,
    }
}

/// Runtime-scoped lexical storage and embedding reachability before a project generation.
pub fn probe_infrastructure_health(runtime: &SidecarRuntimeConfig) -> InfrastructureHealth {
    let snapshot = crate::embeddings::embedding_engine_snapshot_for_runtime(runtime);
    let embedding_device = &snapshot.device;
    let embed_probe = snapshot.probe;
    let identity = snapshot.identity;
    InfrastructureHealth {
        embed_reachable: embed_probe.reachable,
        embedding_device_policy: embedding_device.requested_policy.into(),
        embedding_device_state: embedding_device.observed_state.into(),
        embedding_device_observation_source: embedding_device.observation_source.into(),
        embedding_detected_provider: embedding_device.detected_provider.clone(),
        embedding_detected_gpu: embedding_device.detected_gpu.clone(),
        embedding_accelerator_requested: embedding_device.accelerator_requested,
        embedding_accelerator_request_provider: embedding_device
            .accelerator_request_provider
            .clone(),
        embedding_accelerator_request_device: embedding_device.accelerator_request_device.clone(),
        embedding_cpu_allowed: embedding_device.cpu_allowed,
        embedding_model_sha256: identity
            .as_ref()
            .map(|identity| identity.model_digest.to_string()),
        embedding_ggml_build_identity: identity
            .as_ref()
            .map(|identity| identity.ggml_build_identity.to_string()),
        embedding_backend: identity.as_ref().map(|identity| identity.backend.clone()),
        embedding_adapter: identity
            .as_ref()
            .map(|identity| identity.adapter_name.clone()),
        embedding_adapter_description: identity
            .as_ref()
            .map(|identity| identity.adapter_description.clone()),
        embedding_execution_devices: identity
            .as_ref()
            .map(|identity| identity.execution_device_names.clone()),
        embedding_policy: identity
            .as_ref()
            .map(|identity| identity.policy.to_string()),
        embedding_engine_instance_id: identity
            .as_ref()
            .map(|identity| identity.instance_id.clone()),
        embedding_model_load_count: identity.as_ref().map(|identity| identity.model_load_count),
        embedding_smoke_ms: identity.as_ref().map(|identity| identity.smoke_ms),
        embedding_initialization_ms: identity.as_ref().map(|identity| identity.initialization_ms),
        embedding_materialized_path: identity
            .as_ref()
            .map(|identity| identity.materialized_path.display().to_string()),
        embedding_materialized_reused: identity
            .as_ref()
            .map(|identity| identity.materialized_reused),
        embedding_accelerator_execution_verified: identity
            .as_ref()
            .map(|identity| identity.accelerator_execution_verified),
        embedding_adapter_memory_total: identity
            .as_ref()
            .map(|identity| identity.adapter_memory_total),
        embedding_adapter_memory_used_by_load: identity
            .as_ref()
            .map(|identity| identity.adapter_memory_used_by_load),
        embedding_model_layer_count: identity.as_ref().map(|identity| identity.model_layer_count),
        embedding_offloaded_layer_count: identity
            .as_ref()
            .map(|identity| identity.offloaded_layer_count),
        embed_detail: embed_probe.detail,
    }
}

fn lexical_capabilities(
    layout: &SidecarLayout,
    sidecar_generation: &str,
    sidecar_input_hash: &str,
) -> SidecarCapabilities {
    let shard_dir =
        crate::lexical_index::shard_dir_for(&layout.lexical_data_dir, sidecar_generation);
    if !crate::lexical_index::shard_has_lexical_index(&shard_dir, sidecar_input_hash) {
        return SidecarCapabilities::NONE;
    }
    SidecarCapabilities {
        lexical: true,
        semantic: false,
        graph: false,
    }
}

fn scip_capabilities(availability: &ScipAvailability, project_dir: &Path) -> SidecarCapabilities {
    match availability {
        ScipAvailability::Ready { revision }
            if revision != "stub-v1" && has_real_scip_artifact(project_dir) =>
        {
            SidecarCapabilities {
                lexical: false,
                semantic: false,
                graph: true,
            }
        }
        ScipAvailability::Ready { .. } => SidecarCapabilities::NONE,
        ScipAvailability::Unavailable { .. } => SidecarCapabilities::NONE,
    }
}

fn has_real_scip_artifact(project_dir: &Path) -> bool {
    let Some(revision) = std::fs::read_to_string(project_dir.join("revision.txt"))
        .ok()
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
    else {
        return false;
    };
    project_dir
        .join(crate::scip_index::SCIP_SYMBOLS_FILE)
        .is_file()
        && project_dir
            .join(crate::scip_index::SCIP_INDEX_FILE)
            .is_file()
        && project_dir.join("revision.txt").is_file()
        && !project_dir.join("index.scip.stub").is_file()
        && crate::scip_index::load_fresh_scip_symbols(project_dir, &revision)
            .ok()
            .flatten()
            .is_some_and(|index| !index.symbols.is_empty())
}

pub fn probe_sidecar_health(
    layout: &SidecarLayout,
    project_id: &str,
    manifest: Option<codestory_store::RetrievalIndexManifest>,
) -> RetrievalStatusReport {
    let embedding_device = crate::embeddings::embedding_device_readiness();
    probe_sidecar_health_with_embedding_device(layout, project_id, manifest, &embedding_device)
}

pub fn probe_sidecar_health_with_embedding_device(
    layout: &SidecarLayout,
    project_id: &str,
    manifest: Option<codestory_store::RetrievalIndexManifest>,
    embedding_device: &EmbeddingDeviceReadiness,
) -> RetrievalStatusReport {
    let runtime = crate::config::SidecarRuntimeConfig::local();
    probe_sidecar_health_for_runtime(layout, project_id, manifest, embedding_device, &runtime)
}

pub fn probe_sidecar_health_for_runtime(
    layout: &SidecarLayout,
    project_id: &str,
    manifest: Option<codestory_store::RetrievalIndexManifest>,
    embedding_device: &EmbeddingDeviceReadiness,
    runtime: &crate::config::SidecarRuntimeConfig,
) -> RetrievalStatusReport {
    if let Some(manifest) = manifest.as_ref() {
        if !manifest_has_current_sidecar_contract(project_id, manifest) {
            return unavailable_status_report_with_embedding_device(
                "retrieval_manifest_generation_contract_missing",
                Some(manifest.clone()),
                embedding_device,
            );
        }
    } else {
        return unavailable_status_report_with_embedding_device(
            "retrieval_manifest_missing",
            None,
            embedding_device,
        );
    }

    let manifest = manifest.expect("manifest validation returned above");
    let sidecar_generation = manifest_sidecar_generation(&manifest);
    let sidecar_input_hash = manifest
        .sidecar_input_hash
        .as_deref()
        .expect("manifest contract validation requires sidecar_input_hash");
    let lexical_started = std::time::Instant::now();
    let lexical_coverage = crate::lexical_index::lexical_shard_coverage(
        &layout.lexical_data_dir,
        sidecar_generation,
        sidecar_input_hash,
    );
    let lexical_capabilities = lexical_capabilities(layout, sidecar_generation, sidecar_input_hash);
    let lexical = match lexical_coverage {
        Ok(coverage)
            if lexical_capabilities.lexical
                && coverage.discovered_files > 0
                && coverage.indexed_files == 0 =>
        {
            ComponentHealth {
                name: "lexical".into(),
                status: ComponentStatus::Degraded,
                latency_ms: Some(lexical_started.elapsed().as_millis() as u64),
                detail: coverage.detail(),
                degraded_reason: Some("lexical_source_coverage_empty".into()),
                capabilities: SidecarCapabilities::NONE,
            }
        }
        Ok(coverage) if lexical_capabilities.lexical => ComponentHealth {
            name: "lexical".into(),
            status: ComponentStatus::Healthy,
            latency_ms: Some(lexical_started.elapsed().as_millis() as u64),
            detail: coverage.detail(),
            degraded_reason: (!coverage.complete())
                .then(|| "lexical_source_coverage_incomplete".into()),
            capabilities: lexical_capabilities,
        },
        Ok(coverage) => ComponentHealth {
            name: "lexical".into(),
            status: ComponentStatus::Degraded,
            latency_ms: Some(lexical_started.elapsed().as_millis() as u64),
            detail: coverage.detail(),
            degraded_reason: Some("lexical_shard_invalid".into()),
            capabilities: SidecarCapabilities::NONE,
        },
        Err(error) => ComponentHealth {
            name: "lexical".into(),
            status: ComponentStatus::Unavailable,
            latency_ms: Some(lexical_started.elapsed().as_millis() as u64),
            detail: error.to_string(),
            degraded_reason: Some("lexical_shard_unavailable".into()),
            capabilities: SidecarCapabilities::NONE,
        },
    };

    let current_embedding_backend = crate::embeddings::embedding_runtime_id_for_runtime(runtime);
    let dense_anchor_count = manifest
        .dense_projection_count
        .or(manifest.projection_count)
        .unwrap_or(0);
    let semantic = if dense_anchor_count == 0 {
        zero_dense_semantic_health(embedding_device)
    } else {
        let collection = manifest.semantic_generation.clone();
        let expected_points = u64::try_from(dense_anchor_count).unwrap_or(u64::MAX);
        let embedded = EmbeddedVectorIndex::health(
            layout,
            &collection,
            sidecar_generation,
            sidecar_input_hash,
            expected_points,
            manifest.embedding_backend.as_deref().unwrap_or_default(),
            usize::try_from(manifest.embedding_dim.unwrap_or_default()).unwrap_or_default(),
        );
        let embedding_reachable = embedding_device.full_retrieval_allowed;
        let product_embedding_backend =
            manifest_embedding_backend_is_product(manifest.embedding_backend.as_deref())
                && manifest_embedding_backend_is_product(Some(current_embedding_backend.as_str()));
        let degraded_reason = if !embedded.ready {
            Some("embedded_vector_index_unavailable".into())
        } else if !product_embedding_backend {
            Some("semantic_embedding_contract_mismatch".into())
        } else if !embedding_reachable {
            Some("embedding_runtime_unavailable".into())
        } else if !embedding_device.full_retrieval_allowed {
            embedding_device.degraded_reason.clone()
        } else {
            None
        };
        ComponentHealth {
            name: "semantic".into(),
            status: if degraded_reason.is_none() {
                ComponentStatus::Healthy
            } else if embedded.ready {
                ComponentStatus::Degraded
            } else {
                ComponentStatus::Unavailable
            },
            latency_ms: Some(embedded.latency_ms),
            detail: format!("{}; in-process embedding engine", embedded.detail),
            degraded_reason,
            capabilities: if embedded.ready
                && product_embedding_backend
                && embedding_reachable
                && embedding_device.full_retrieval_allowed
            {
                SidecarCapabilities {
                    lexical: false,
                    semantic: true,
                    graph: false,
                }
            } else {
                SidecarCapabilities::NONE
            },
        }
    };

    let scip_project_dir = layout.scip_project_dir(sidecar_generation);
    let scip_probe = ScipClient::health_probe(layout, sidecar_generation);
    let scip_capabilities = scip_capabilities(&scip_probe.availability, &scip_project_dir);
    let scip_stub = matches!(&scip_probe.availability, ScipAvailability::Ready { .. })
        && !scip_capabilities.graph;
    let (scip_status, scip_degraded) = match &scip_probe.availability {
        ScipAvailability::Ready { .. } if scip_capabilities.graph => {
            (ComponentStatus::Healthy, None)
        }
        ScipAvailability::Ready { .. } => (ComponentStatus::Degraded, Some("scip_stub".into())),
        ScipAvailability::Unavailable { reason } => {
            (ComponentStatus::Unavailable, Some(reason.clone()))
        }
    };
    let scip = ComponentHealth {
        name: "scip".into(),
        status: scip_status,
        latency_ms: None,
        detail: scip_probe.detail,
        degraded_reason: scip_degraded.or_else(|| scip_stub.then_some("scip_stub".into())),
        capabilities: scip_capabilities,
    };

    let (live_mode, degraded_reason) =
        crate::mode::derive_degraded_mode(&lexical, &semantic, &scip);
    let retrieval_mode = if manifest_classifies_full(&manifest) {
        "full"
    } else {
        live_mode.as_str()
    };

    RetrievalStatusReport {
        retrieval_mode: retrieval_mode.into(),
        degraded_reason,
        query_embedding_backend: current_embedding_backend,
        manifest_vector_embedding_backend: manifest.embedding_backend.clone(),
        manifest_vector_embedding_dim: manifest.embedding_dim,
        stored_doc_vector_producer_backend: None,
        stored_doc_vector_dim: None,
        stored_doc_vector_mixed_backends: None,
        embedding_device_policy: embedding_device.requested_policy.into(),
        embedding_device_state: embedding_device.observed_state.into(),
        embedding_device_observation_source: embedding_device.observation_source.into(),
        embedding_detected_provider: embedding_device.detected_provider.clone(),
        embedding_detected_gpu: embedding_device.detected_gpu.clone(),
        embedding_accelerator_requested: embedding_device.accelerator_requested,
        embedding_accelerator_request_provider: embedding_device
            .accelerator_request_provider
            .clone(),
        embedding_accelerator_request_device: embedding_device.accelerator_request_device.clone(),
        embedding_cpu_allowed: embedding_device.cpu_allowed,
        lexical,
        semantic,
        scip,
        manifest_contract: None,
        manifest: Some(manifest),
    }
}

fn zero_dense_semantic_health(
    embedding_device: &crate::embeddings::EmbeddingDeviceReadiness,
) -> ComponentHealth {
    if !embedding_device.full_retrieval_allowed {
        return ComponentHealth {
            name: "semantic".into(),
            status: ComponentStatus::Degraded,
            latency_ms: None,
            detail: "graph_first_v1 selected zero dense anchors, but embedding device policy is not verified".into(),
            degraded_reason: embedding_device.degraded_reason.clone(),
            capabilities: SidecarCapabilities::NONE,
        };
    }

    ComponentHealth {
        name: "semantic".into(),
        status: ComponentStatus::Healthy,
        latency_ms: None,
        detail: "graph_first_v1 selected zero dense anchors; semantic retrieval skipped by policy"
            .into(),
        degraded_reason: None,
        capabilities: SidecarCapabilities {
            lexical: false,
            semantic: true,
            graph: false,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SidecarLayout;
    use crate::lexical_index::{build_lexical_shard, lexical_input_fingerprint, shard_dir_for};
    use crate::test_support::retrieval_manifest_fixture;
    use tempfile::TempDir;

    #[test]
    fn status_reports_unavailable_when_lexical_down() {
        let layout = SidecarLayout::from_env();
        let report = probe_sidecar_health(&layout, "testproject", None);
        assert_eq!(report.lexical.name, "lexical");
        if report.lexical.status == ComponentStatus::Unavailable {
            assert_eq!(report.retrieval_mode, "unavailable");
        }
    }

    #[test]
    fn status_reports_unavailable_for_legacy_manifest_without_generation_contract() {
        let layout = SidecarLayout::from_env();
        let manifest = codestory_store::RetrievalIndexManifest {
            project_id: "testproject".into(),
            lexical_version: crate::lexical_index::LEXICAL_INDEX_VERSION.into(),
            semantic_generation: "codestory_testproject".into(),
            scip_revision: Some("graph-test".into()),
            built_at_epoch_ms: 1,
            disk_bytes: None,
            degraded_modes_json: "[]".into(),
            embedding_backend: Some("hash-projection:768".into()),
            embedding_dim: Some(768),
            sidecar_schema_version: None,
            sidecar_input_hash: None,
            sidecar_generation: None,
            projection_count: None,
            symbol_doc_count: None,
            dense_projection_count: None,
            semantic_policy_version: None,
            graph_artifact_hash: None,
            dense_reason_counts_json: None,
            precise_semantic_import_status: None,
            precise_semantic_import_reason: None,
            precise_semantic_import_revision: None,
            precise_semantic_import_producer: None,
        };

        let report = probe_sidecar_health(&layout, "testproject", Some(manifest));

        assert_eq!(report.retrieval_mode, "unavailable");
        assert_eq!(
            report.degraded_reason.as_deref(),
            Some("retrieval_manifest_generation_contract_missing")
        );
        assert_eq!(report.lexical.capabilities, SidecarCapabilities::NONE);
        assert_eq!(report.semantic.capabilities, SidecarCapabilities::NONE);
        assert_eq!(report.scip.capabilities, SidecarCapabilities::NONE);
    }

    #[test]
    fn malformed_lexical_shard_cannot_report_full_readiness() {
        let project = TempDir::new().expect("project");
        std::fs::write(project.path().join("lib.rs"), "pub fn alpha() {}").expect("write source");
        let data = TempDir::new().expect("data");
        let mut layout = SidecarLayout::from_env();
        layout.lexical_data_dir = data.path().to_path_buf();
        let manifest = retrieval_manifest_fixture("testproject", "test-input");
        let generation = manifest.sidecar_generation.as_deref().expect("generation");
        let fingerprint = lexical_input_fingerprint(project.path(), None).expect("fingerprint");
        build_lexical_shard(
            project.path(),
            None,
            &layout.lexical_data_dir,
            generation,
            &fingerprint,
            "test-input",
        )
        .expect("build shard");
        let index = shard_dir_for(&layout.lexical_data_dir, generation)
            .join(crate::lexical_index::LEXICAL_INDEX_FILE);
        crate::lexical_index::make_test_file_writable(&index);
        std::fs::write(index, b"not sqlite").expect("corrupt shard");

        let report = probe_sidecar_health(&layout, "testproject", Some(manifest));

        assert!(!report.lexical.capabilities.lexical);
        assert_eq!(report.retrieval_mode, "full");
        assert!(!report.is_live_ready());
    }

    #[test]
    fn partial_lexical_coverage_stays_usable_and_reports_diagnostic() {
        let project = TempDir::new().expect("project");
        std::fs::write(project.path().join("lib.rs"), "pub fn alpha() {}").expect("source");
        std::fs::write(project.path().join("oversized.rs"), vec![b'x'; 1_000_001])
            .expect("oversized");
        let data = TempDir::new().expect("data");
        let mut layout = SidecarLayout::from_env();
        layout.lexical_data_dir = data.path().to_path_buf();
        let manifest = retrieval_manifest_fixture("testproject", "test-input");
        let generation = manifest.sidecar_generation.as_deref().expect("generation");
        let fingerprint = lexical_input_fingerprint(project.path(), None).expect("fingerprint");
        build_lexical_shard(
            project.path(),
            None,
            &layout.lexical_data_dir,
            generation,
            &fingerprint,
            "test-input",
        )
        .expect("build shard");

        let report = probe_sidecar_health(&layout, "testproject", Some(manifest));

        assert_eq!(report.lexical.status, ComponentStatus::Healthy);
        assert!(report.lexical.capabilities.lexical);
        assert_eq!(
            report.lexical.degraded_reason.as_deref(),
            Some("lexical_source_coverage_incomplete")
        );
        assert!(report.lexical.detail.contains("omitted_oversized=1"));
    }

    #[test]
    fn all_omitted_lexical_sources_cannot_report_full_readiness() {
        let project = TempDir::new().expect("project");
        std::fs::write(project.path().join("large.rs"), vec![b'x'; 1_000_001]).expect("oversized");
        std::fs::write(project.path().join("invalid.rs"), [0xff, 0xfe, 0xfd])
            .expect("invalid utf-8");
        let data = TempDir::new().expect("data");
        let mut layout = SidecarLayout::from_env();
        layout.lexical_data_dir = data.path().to_path_buf();
        let manifest = retrieval_manifest_fixture("testproject", "test-input");
        let generation = manifest.sidecar_generation.as_deref().expect("generation");
        let fingerprint = lexical_input_fingerprint(project.path(), None).expect("fingerprint");
        assert_eq!(fingerprint.file_count, 0);
        build_lexical_shard(
            project.path(),
            None,
            &layout.lexical_data_dir,
            generation,
            &fingerprint,
            "test-input",
        )
        .expect("build empty shard");

        let report = probe_sidecar_health(&layout, "testproject", Some(manifest));

        assert_eq!(report.lexical.status, ComponentStatus::Degraded);
        assert!(!report.lexical.capabilities.lexical);
        assert_eq!(
            report.lexical.degraded_reason.as_deref(),
            Some("lexical_source_coverage_empty")
        );
        assert_eq!(report.retrieval_mode, "full");
        assert!(!report.is_live_ready());
    }

    #[test]
    fn zero_dense_manifest_still_requires_verified_embedding_device() {
        let semantic = zero_dense_semantic_health(&crate::embeddings::EmbeddingDeviceReadiness {
            requested_policy: "accelerator_required",
            observed_state: "unknown",
            observation_source: "retrieval_unobserved",
            detected_provider: None,
            detected_gpu: None,
            accelerator_requested: false,
            accelerator_request_provider: None,
            accelerator_request_device: None,
            cpu_allowed: false,
            full_retrieval_allowed: false,
            degraded_reason: Some("embedding_device_unverified".into()),
        });

        assert_eq!(semantic.status, ComponentStatus::Degraded);
        assert_eq!(
            semantic.degraded_reason.as_deref(),
            Some("embedding_device_unverified")
        );
        assert!(!semantic.capabilities.semantic);
    }

    #[test]
    fn unavailable_status_reports_metal_request_without_device() {
        let embedding_device = crate::embeddings::EmbeddingDeviceReadiness {
            requested_policy: "accelerator_required",
            observed_state: "unknown",
            observation_source: "accelerator_request_unobserved",
            detected_provider: None,
            detected_gpu: None,
            accelerator_requested: true,
            accelerator_request_provider: Some("metal".into()),
            accelerator_request_device: None,
            cpu_allowed: false,
            full_retrieval_allowed: false,
            degraded_reason: Some("embedding_device_unverified".into()),
        };

        let report =
            unavailable_status_report_with_embedding_device("missing", None, &embedding_device);

        assert_eq!(
            report.embedding_accelerator_request_provider.as_deref(),
            Some("metal")
        );
        assert_eq!(report.embedding_accelerator_request_device, None);
    }

    #[test]
    fn zero_dense_manifest_allows_explicit_cpu_opt_in() {
        let semantic = zero_dense_semantic_health(&crate::embeddings::EmbeddingDeviceReadiness {
            requested_policy: "cpu_explicit",
            observed_state: "cpu_explicit",
            observation_source: "inprocess_engine",
            detected_provider: None,
            detected_gpu: None,
            accelerator_requested: false,
            accelerator_request_provider: None,
            accelerator_request_device: None,
            cpu_allowed: true,
            full_retrieval_allowed: true,
            degraded_reason: None,
        });

        assert_eq!(semantic.status, ComponentStatus::Healthy);
        assert_eq!(semantic.degraded_reason, None);
        assert!(semantic.capabilities.semantic);
    }

    #[test]
    fn manifest_contract_reports_core_fields_and_lane_provenance() {
        let manifest = codestory_store::RetrievalIndexManifest {
            project_id: "testproject".into(),
            lexical_version: crate::lexical_index::LEXICAL_INDEX_VERSION.into(),
            semantic_generation: "codestory_testproject_hash".into(),
            scip_revision: Some("graph-test".into()),
            built_at_epoch_ms: 1,
            disk_bytes: Some(42),
            degraded_modes_json: r#"["embedded_vector_index_unavailable"]"#.into(),
            embedding_backend: Some(crate::embeddings::PRODUCT_EMBEDDING_RUNTIME_ID.into()),
            embedding_dim: Some(768),
            sidecar_schema_version: Some(crate::generation::SIDECAR_SCHEMA_VERSION),
            sidecar_input_hash: Some("input-hash".into()),
            sidecar_generation: Some("testproject-input".into()),
            projection_count: Some(12),
            symbol_doc_count: Some(9),
            dense_projection_count: Some(3),
            semantic_policy_version: Some("graph_first_v1".into()),
            graph_artifact_hash: Some("graph-hash".into()),
            dense_reason_counts_json: Some(r#"{"public_api":3}"#.into()),
            precise_semantic_import_status: Some("fresh".into()),
            precise_semantic_import_reason: None,
            precise_semantic_import_revision: Some("imported-a".into()),
            precise_semantic_import_producer: Some("scip-fixture".into()),
        };
        let report = RetrievalStatusReport {
            retrieval_mode: "full".into(),
            degraded_reason: None,
            query_embedding_backend: crate::embeddings::PRODUCT_EMBEDDING_RUNTIME_ID.into(),
            manifest_vector_embedding_backend: manifest.embedding_backend.clone(),
            manifest_vector_embedding_dim: manifest.embedding_dim,
            stored_doc_vector_producer_backend: None,
            stored_doc_vector_dim: None,
            stored_doc_vector_mixed_backends: None,
            embedding_device_policy: "accelerator_required".into(),
            embedding_device_state: "accelerated".into(),
            embedding_device_observation_source: "inprocess_engine".into(),
            embedding_detected_provider: None,
            embedding_detected_gpu: None,
            embedding_accelerator_requested: false,
            embedding_accelerator_request_provider: None,
            embedding_accelerator_request_device: None,
            embedding_cpu_allowed: false,
            lexical: ComponentHealth {
                name: "lexical".into(),
                status: ComponentStatus::Healthy,
                latency_ms: Some(1),
                detail: "ok".into(),
                degraded_reason: None,
                capabilities: SidecarCapabilities {
                    lexical: true,
                    semantic: false,
                    graph: false,
                },
            },
            semantic: ComponentHealth {
                name: "semantic".into(),
                status: ComponentStatus::Healthy,
                latency_ms: Some(1),
                detail: "ok".into(),
                degraded_reason: None,
                capabilities: SidecarCapabilities {
                    lexical: false,
                    semantic: true,
                    graph: false,
                },
            },
            scip: ComponentHealth {
                name: "scip".into(),
                status: ComponentStatus::Healthy,
                latency_ms: None,
                detail: "ok".into(),
                degraded_reason: None,
                capabilities: SidecarCapabilities {
                    lexical: false,
                    semantic: false,
                    graph: true,
                },
            },
            manifest_contract: None,
            manifest: Some(manifest),
        };
        let source_root = std::env::current_dir().expect("current dir");

        let report = attach_manifest_contract(report, &source_root);
        let contract = report
            .manifest_contract
            .expect("manifest contract should be derived");

        assert_eq!(contract.source_root, source_root.display().to_string());
        assert_eq!(contract.project_id, "testproject");
        assert_eq!(contract.input_hash.as_deref(), Some("input-hash"));
        assert_eq!(contract.generation.as_deref(), Some("testproject-input"));
        assert_eq!(
            contract.schema_version,
            Some(crate::generation::SIDECAR_SCHEMA_VERSION)
        );
        assert_eq!(contract.graph_hash.as_deref(), Some("graph-hash"));
        assert_eq!(contract.symbol_doc_count, Some(9));
        assert_eq!(contract.dense_anchor_count, Some(3));
        assert_eq!(
            contract.degraded_modes,
            vec!["embedded_vector_index_unavailable"]
        );
        assert_eq!(contract.lanes.len(), 5);
        assert!(contract.lanes.iter().any(|lane| {
            lane.lane == "lexical"
                && lane.producer == crate::lexical_index::LEXICAL_INDEX_VERSION
                && lane.provenance == "sidecar_generation:testproject-input"
        }));
        assert!(contract.lanes.iter().any(|lane| {
            lane.lane == "semantic_dense"
                && lane.producer == crate::embeddings::PRODUCT_EMBEDDING_RUNTIME_ID
                && lane.provenance == "vector_generation:codestory_testproject_hash"
                && lane.count == Some(3)
        }));
        assert!(contract.lanes.iter().any(|lane| {
            lane.lane == "graph"
                && lane.producer == "graph-test"
                && lane.provenance == "graph_artifact_hash:graph-hash"
        }));
        assert!(contract.lanes.iter().any(|lane| {
            lane.lane == "precise_semantic_import"
                && lane.producer == "scip-fixture"
                && lane.provenance == "precise_semantic_import_revision:imported-a"
                && lane.status == "fresh"
        }));
    }
}
