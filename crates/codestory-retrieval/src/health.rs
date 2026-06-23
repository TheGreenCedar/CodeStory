use crate::capabilities::SidecarCapabilities;
use crate::config::{
    QDRANT_HEALTH_BUDGET, SidecarLayout, ZOEKT_HEALTH_BUDGET, qdrant_semantic_vectors_enabled,
};
use crate::embeddings::manifest_embedding_backend_is_product;
use crate::generation::{manifest_has_current_sidecar_contract, manifest_sidecar_generation};
use crate::qdrant_client::QdrantClient;
use crate::scip_client::{ScipAvailability, ScipClient};
use crate::zoekt_client::ZoektClient;
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
pub struct RetrievalRepairHint {
    pub reason: String,
    pub next_step: String,
    pub next_command: String,
    pub full_repair: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetrievalStatusReport {
    pub retrieval_mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub degraded_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repair: Option<RetrievalRepairHint>,
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
    pub zoekt: ComponentHealth,
    pub qdrant: ComponentHealth,
    pub scip: ComponentHealth,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manifest_contract: Option<RetrievalManifestContractReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manifest: Option<codestory_store::RetrievalIndexManifest>,
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

pub fn attach_repair_hint(
    mut report: RetrievalStatusReport,
    project_root: &Path,
) -> RetrievalStatusReport {
    if report.retrieval_mode == "full" {
        return report;
    }
    let reason = repair_reason_code(
        report
            .degraded_reason
            .as_deref()
            .unwrap_or("sidecar_retrieval_not_full"),
    );
    let project = quote_command_path(project_root);
    let full_repair = vec![
        format!("codestory-cli retrieval bootstrap --project {project} --format json"),
        format!("codestory-cli retrieval index --project {project} --refresh full --format json"),
        format!("codestory-cli retrieval status --project {project} --format json"),
    ];
    report.repair = Some(RetrievalRepairHint {
        reason,
        next_step:
            "Prepare sidecars, rebuild retrieval indexes with full refresh, then recheck status."
                .into(),
        next_command: full_repair[0].clone(),
        full_repair,
    });
    report
}

fn repair_reason_code(degraded_reason: &str) -> String {
    if degraded_reason.starts_with("sidecar_manifest_stale:") {
        return "sidecar_manifest_stale".into();
    }
    degraded_reason.to_string()
}

fn quote_command_path(path: &Path) -> String {
    format!("\"{}\"", path.display().to_string().replace('"', "\\\""))
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
            producer: manifest.zoekt_version.clone(),
            provenance: format!("sidecar_generation:{generation}"),
            count: None,
            status: component_status_label(&report.zoekt),
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
            provenance: format!("qdrant_collection:{}", manifest.qdrant_collection),
            count: manifest.dense_projection_count,
            status: component_status_label(&report.qdrant),
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
    pub zoekt_reachable: bool,
    pub qdrant_reachable: bool,
    pub embed_reachable: bool,
    pub zoekt_detail: String,
    pub qdrant_detail: String,
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

pub fn unavailable_status_report(
    reason: impl Into<String>,
    manifest: Option<codestory_store::RetrievalIndexManifest>,
) -> RetrievalStatusReport {
    let reason = reason.into();
    let manifest_vector_embedding_backend = manifest
        .as_ref()
        .and_then(|manifest| manifest.embedding_backend.clone());
    let manifest_vector_embedding_dim = manifest
        .as_ref()
        .and_then(|manifest| manifest.embedding_dim);
    RetrievalStatusReport {
        retrieval_mode: "unavailable".into(),
        degraded_reason: Some(reason.clone()),
        repair: None,
        query_embedding_backend: crate::embeddings::embedding_runtime_id(),
        manifest_vector_embedding_backend,
        manifest_vector_embedding_dim,
        stored_doc_vector_producer_backend: None,
        stored_doc_vector_dim: None,
        stored_doc_vector_mixed_backends: None,
        zoekt: unavailable_component("zoekt", &reason),
        qdrant: unavailable_component("qdrant", &reason),
        scip: unavailable_component("scip", &reason),
        manifest_contract: None,
        manifest,
    }
}

/// Zoekt + Qdrant + embedding reachability without a project collection (used during bootstrap).
pub fn probe_infrastructure_health(layout: &SidecarLayout) -> InfrastructureHealth {
    let zoekt_probe = ZoektClient::new(layout).health_probe();
    let qdrant_client = QdrantClient::new(layout);
    let qdrant_probe = qdrant_client.list_collections_probe();
    let embed_probe = crate::embeddings::probe_product_embedding_runtime();
    InfrastructureHealth {
        zoekt_reachable: zoekt_probe.reachable,
        qdrant_reachable: qdrant_probe.reachable,
        embed_reachable: embed_probe.reachable,
        zoekt_detail: zoekt_probe.detail,
        qdrant_detail: qdrant_probe.detail,
        embed_detail: embed_probe.detail,
    }
}

fn zoekt_capabilities(
    layout: &SidecarLayout,
    sidecar_generation: &str,
    reachable: bool,
    _zoekt_client: &ZoektClient,
) -> SidecarCapabilities {
    let shard_dir = crate::zoekt_index::shard_dir_for(&layout.zoekt_data_dir, sidecar_generation);
    if !crate::zoekt_index::shard_has_lexical_index(&shard_dir) {
        return SidecarCapabilities::NONE;
    }
    let _ = reachable;
    SidecarCapabilities {
        lexical: true,
        semantic: false,
        graph: false,
    }
}

fn qdrant_capabilities(
    layout: &SidecarLayout,
    collection: &str,
    probe: &crate::qdrant_client::QdrantHealthProbe,
    expected_points: Option<u64>,
    product_embedding_backend: bool,
    current_product_embedding_backend: bool,
) -> SidecarCapabilities {
    if !probe.reachable || !probe.collection_exists {
        return SidecarCapabilities::NONE;
    }
    if qdrant_point_count_incomplete(probe, expected_points) {
        return SidecarCapabilities::NONE;
    }
    let client = QdrantClient::new(layout);
    let semantic = !QdrantClient::is_collection_stubbed(&layout.qdrant_data_dir, collection)
        && qdrant_semantic_vectors_enabled()
        && product_embedding_backend
        && current_product_embedding_backend
        && client.semantic_search_smoke(collection);
    SidecarCapabilities {
        lexical: false,
        semantic,
        graph: false,
    }
}

fn qdrant_point_count_incomplete(
    probe: &crate::qdrant_client::QdrantHealthProbe,
    expected_points: Option<u64>,
) -> bool {
    matches!(
        (probe.point_count, expected_points),
        (Some(actual), Some(expected)) if actual < expected
    )
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
    if let Some(manifest) = manifest.as_ref() {
        if !manifest_has_current_sidecar_contract(project_id, manifest) {
            return unavailable_status_report(
                "sidecar_manifest_generation_contract_missing",
                Some(manifest.clone()),
            );
        }
    } else {
        return unavailable_status_report("retrieval_manifest_missing", None);
    }

    let manifest = manifest.expect("manifest validation returned above");
    let zoekt_client = ZoektClient::new(layout);
    let zoekt_probe = zoekt_client.health_probe();
    let sidecar_generation = manifest_sidecar_generation(&manifest);
    let zoekt_capabilities = zoekt_capabilities(
        layout,
        sidecar_generation,
        zoekt_probe.reachable,
        &zoekt_client,
    );
    let zoekt_stub = zoekt_probe.reachable && !zoekt_capabilities.lexical;
    let zoekt = ComponentHealth {
        name: "zoekt".into(),
        status: if !zoekt_probe.reachable {
            ComponentStatus::Unavailable
        } else if zoekt_stub {
            ComponentStatus::Degraded
        } else if zoekt_probe.latency_ms <= ZOEKT_HEALTH_BUDGET.as_millis() as u64 {
            ComponentStatus::Healthy
        } else {
            ComponentStatus::Degraded
        },
        latency_ms: Some(zoekt_probe.latency_ms),
        detail: zoekt_probe.detail,
        degraded_reason: if !zoekt_probe.reachable {
            Some("zoekt_unreachable".into())
        } else if zoekt_stub {
            Some("zoekt_stub".into())
        } else {
            None
        },
        capabilities: zoekt_capabilities,
    };

    let current_embedding_backend = crate::embeddings::embedding_runtime_id();
    let dense_anchor_count = manifest
        .dense_projection_count
        .or(manifest.projection_count)
        .unwrap_or(0);
    let qdrant = if dense_anchor_count == 0 {
        ComponentHealth {
            name: "qdrant".into(),
            status: ComponentStatus::Degraded,
            latency_ms: None,
            detail: "graph_first_v1 selected zero dense anchors; semantic retrieval is unavailable"
                .into(),
            degraded_reason: Some("semantic_dense_projection_count_zero".into()),
            capabilities: SidecarCapabilities::NONE,
        }
    } else {
        let collection = manifest.qdrant_collection.clone();
        let qdrant_probe = QdrantClient::new(layout).health_probe(&collection);
        let expected_qdrant_points = Some(u64::try_from(dense_anchor_count).unwrap_or(u64::MAX));
        let qdrant_point_count_incomplete =
            qdrant_point_count_incomplete(&qdrant_probe, expected_qdrant_points);
        let product_embedding_backend =
            manifest_embedding_backend_is_product(manifest.embedding_backend.as_deref());
        let current_product_embedding_backend =
            manifest_embedding_backend_is_product(Some(current_embedding_backend.as_str()));
        let qdrant_capabilities = qdrant_capabilities(
            layout,
            &collection,
            &qdrant_probe,
            expected_qdrant_points,
            product_embedding_backend,
            current_product_embedding_backend,
        );
        let qdrant_semantic_stub = qdrant_probe.reachable
            && qdrant_probe.collection_exists
            && !qdrant_capabilities.semantic;
        ComponentHealth {
            name: "qdrant".into(),
            status: if !qdrant_probe.reachable {
                ComponentStatus::Unavailable
            } else if !qdrant_probe.collection_exists || qdrant_semantic_stub {
                ComponentStatus::Degraded
            } else if qdrant_probe.latency_ms <= QDRANT_HEALTH_BUDGET.as_millis() as u64 {
                ComponentStatus::Healthy
            } else {
                ComponentStatus::Degraded
            },
            latency_ms: Some(qdrant_probe.latency_ms),
            detail: qdrant_probe.detail,
            degraded_reason: if !qdrant_probe.reachable {
                Some("qdrant_unreachable".into())
            } else if !qdrant_probe.collection_exists {
                Some("qdrant_collection_missing".into())
            } else if qdrant_point_count_incomplete {
                Some("qdrant_point_count_incomplete".into())
            } else if !product_embedding_backend {
                Some("qdrant_non_product_embedding_backend".into())
            } else if !current_product_embedding_backend {
                Some("qdrant_current_embedding_backend_not_product".into())
            } else if qdrant_semantic_stub {
                Some(if qdrant_semantic_vectors_enabled() {
                    "qdrant_semantic_smoke_failed".into()
                } else {
                    "qdrant_hash_vectors_only".into()
                })
            } else {
                None
            },
            capabilities: qdrant_capabilities,
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

    let (mode, degraded_reason) = crate::mode::derive_degraded_mode(&zoekt, &qdrant, &scip);

    RetrievalStatusReport {
        retrieval_mode: mode.as_str().into(),
        degraded_reason,
        repair: None,
        query_embedding_backend: current_embedding_backend,
        manifest_vector_embedding_backend: manifest.embedding_backend.clone(),
        manifest_vector_embedding_dim: manifest.embedding_dim,
        stored_doc_vector_producer_backend: None,
        stored_doc_vector_dim: None,
        stored_doc_vector_mixed_backends: None,
        zoekt,
        qdrant,
        scip,
        manifest_contract: None,
        manifest: Some(manifest),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SidecarLayout;

    #[test]
    fn repair_hint_names_reason_and_full_sidecar_rebuild_sequence() {
        let report = attach_repair_hint(
            unavailable_status_report("retrieval_manifest_missing", None),
            Path::new("C:/repo with spaces"),
        );
        let repair = report.repair.expect("repair hint");

        assert_eq!(repair.reason, "retrieval_manifest_missing");
        assert!(
            repair.next_command.contains("retrieval bootstrap"),
            "repair should start with sidecar bootstrap: {repair:?}"
        );
        assert!(
            repair.full_repair.iter().any(|command| command
                .contains("retrieval index --project \"C:/repo with spaces\" --refresh full")),
            "repair should include full retrieval rebuild with quoted project path: {repair:?}"
        );
        assert!(
            repair
                .full_repair
                .last()
                .is_some_and(|command| command.contains("retrieval status")),
            "repair should end with retrieval status proof: {repair:?}"
        );
    }

    #[test]
    fn repair_hint_keeps_stale_reason_stable_and_degraded_reason_detailed() {
        let detailed = "sidecar_manifest_stale: sidecar_input_hash_mismatch current=abc stored=def path=src/lib.rs";
        let report = attach_repair_hint(
            unavailable_status_report(detailed, None),
            Path::new("C:/repo"),
        );
        let repair = report.repair.expect("repair hint");

        assert_eq!(report.degraded_reason.as_deref(), Some(detailed));
        assert_eq!(repair.reason, "sidecar_manifest_stale");
    }

    #[test]
    fn status_reports_unavailable_when_zoekt_down() {
        let layout = SidecarLayout::from_env();
        let report = probe_sidecar_health(&layout, "testproject", None);
        assert_eq!(report.zoekt.name, "zoekt");
        if report.zoekt.status == ComponentStatus::Unavailable {
            assert_eq!(report.retrieval_mode, "unavailable");
        }
    }

    #[test]
    fn status_reports_unavailable_for_legacy_manifest_without_generation_contract() {
        let layout = SidecarLayout::from_env();
        let manifest = codestory_store::RetrievalIndexManifest {
            project_id: "testproject".into(),
            zoekt_version: "zoekt-real-v1".into(),
            qdrant_collection: QdrantClient::collection_name("testproject"),
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
            Some("sidecar_manifest_generation_contract_missing")
        );
        assert_eq!(report.zoekt.capabilities, SidecarCapabilities::NONE);
        assert_eq!(report.qdrant.capabilities, SidecarCapabilities::NONE);
        assert_eq!(report.scip.capabilities, SidecarCapabilities::NONE);
    }

    #[test]
    fn qdrant_point_count_gap_blocks_semantic_capability() {
        let probe = crate::qdrant_client::QdrantHealthProbe {
            reachable: true,
            latency_ms: 1,
            collection_exists: true,
            point_count: Some(10),
            detail: "http 200 points_count=10".into(),
        };

        assert!(qdrant_point_count_incomplete(&probe, Some(11)));
        assert!(!qdrant_point_count_incomplete(&probe, Some(10)));
        assert!(!qdrant_point_count_incomplete(&probe, None));
    }

    #[test]
    fn zero_dense_manifest_degrades_semantic_lane() {
        let layout = SidecarLayout::from_env();
        let project_id = "testproject";
        let input_hash = "ba5eba11cafebeef";
        let manifest = codestory_store::RetrievalIndexManifest {
            project_id: project_id.into(),
            zoekt_version: "zoekt-real-v1".into(),
            qdrant_collection: crate::generation::sidecar_qdrant_collection(project_id, input_hash),
            scip_revision: Some("graph-test".into()),
            built_at_epoch_ms: 1,
            disk_bytes: None,
            degraded_modes_json: "[]".into(),
            embedding_backend: Some(crate::embeddings::PRODUCT_EMBEDDING_RUNTIME_ID.into()),
            embedding_dim: Some(crate::embeddings::RETRIEVAL_EMBEDDING_DIM as i32),
            sidecar_schema_version: Some(crate::generation::SIDECAR_SCHEMA_VERSION),
            sidecar_input_hash: Some(input_hash.into()),
            sidecar_generation: Some(crate::generation::sidecar_generation_id(
                project_id, input_hash,
            )),
            projection_count: Some(0),
            symbol_doc_count: Some(14741),
            dense_projection_count: Some(0),
            semantic_policy_version: Some(crate::generation::SEMANTIC_POLICY_VERSION.into()),
            graph_artifact_hash: Some("graph-test-hash".into()),
            dense_reason_counts_json: Some("{}".into()),
            precise_semantic_import_status: None,
            precise_semantic_import_reason: None,
            precise_semantic_import_revision: None,
            precise_semantic_import_producer: None,
        };

        let report = probe_sidecar_health(&layout, project_id, Some(manifest));

        assert_eq!(report.qdrant.status, ComponentStatus::Degraded);
        assert_eq!(
            report.qdrant.degraded_reason.as_deref(),
            Some("semantic_dense_projection_count_zero")
        );
        assert!(!report.qdrant.capabilities.semantic);
        assert_ne!(report.retrieval_mode, "full");
    }

    #[test]
    fn qdrant_capability_requires_product_current_backend() {
        let layout = SidecarLayout::from_env();
        let probe = crate::qdrant_client::QdrantHealthProbe {
            reachable: true,
            latency_ms: 1,
            collection_exists: true,
            point_count: Some(10),
            detail: "http 200 points_count=10".into(),
        };

        let capabilities =
            qdrant_capabilities(&layout, "codestory_test", &probe, Some(10), true, false);

        assert_eq!(capabilities, SidecarCapabilities::NONE);
    }

    #[test]
    fn manifest_contract_reports_core_fields_and_lane_provenance() {
        let manifest = codestory_store::RetrievalIndexManifest {
            project_id: "testproject".into(),
            zoekt_version: "zoekt-real-v1".into(),
            qdrant_collection: "codestory_testproject_hash".into(),
            scip_revision: Some("graph-test".into()),
            built_at_epoch_ms: 1,
            disk_bytes: Some(42),
            degraded_modes_json: r#"["qdrant_hash_vectors_only"]"#.into(),
            embedding_backend: Some("llamacpp:bge-base-en-v1.5".into()),
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
            query_embedding_backend: "llamacpp:bge-base-en-v1.5".into(),
            manifest_vector_embedding_backend: manifest.embedding_backend.clone(),
            manifest_vector_embedding_dim: manifest.embedding_dim,
            stored_doc_vector_producer_backend: None,
            stored_doc_vector_dim: None,
            stored_doc_vector_mixed_backends: None,
            zoekt: ComponentHealth {
                name: "zoekt".into(),
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
            qdrant: ComponentHealth {
                name: "qdrant".into(),
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
            repair: None,
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
        assert_eq!(contract.degraded_modes, vec!["qdrant_hash_vectors_only"]);
        assert_eq!(contract.lanes.len(), 5);
        assert!(contract.lanes.iter().any(|lane| {
            lane.lane == "lexical"
                && lane.producer == "zoekt-real-v1"
                && lane.provenance == "sidecar_generation:testproject-input"
        }));
        assert!(contract.lanes.iter().any(|lane| {
            lane.lane == "semantic_dense"
                && lane.producer == "llamacpp:bge-base-en-v1.5"
                && lane.provenance == "qdrant_collection:codestory_testproject_hash"
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
