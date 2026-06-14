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
    pub zoekt: ComponentHealth,
    pub qdrant: ComponentHealth,
    pub scip: ComponentHealth,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manifest: Option<codestory_store::RetrievalIndexManifest>,
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
        query_embedding_backend: crate::embeddings::embedding_runtime_id(),
        manifest_vector_embedding_backend,
        manifest_vector_embedding_dim,
        stored_doc_vector_producer_backend: None,
        stored_doc_vector_dim: None,
        stored_doc_vector_mixed_backends: None,
        zoekt: unavailable_component("zoekt", &reason),
        qdrant: unavailable_component("qdrant", &reason),
        scip: unavailable_component("scip", &reason),
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
    project_dir
        .join(crate::scip_index::SCIP_SYMBOLS_FILE)
        .is_file()
        && project_dir
            .join(crate::scip_index::SCIP_INDEX_FILE)
            .is_file()
        && project_dir.join("revision.txt").is_file()
        && !project_dir.join("index.scip.stub").is_file()
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
            status: ComponentStatus::Healthy,
            latency_ms: None,
            detail: "graph_first_v1 selected zero dense anchors; qdrant not required".into(),
            degraded_reason: None,
            capabilities: SidecarCapabilities {
                lexical: false,
                semantic: true,
                graph: false,
            },
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
        query_embedding_backend: current_embedding_backend,
        manifest_vector_embedding_backend: manifest.embedding_backend.clone(),
        manifest_vector_embedding_dim: manifest.embedding_dim,
        stored_doc_vector_producer_backend: None,
        stored_doc_vector_dim: None,
        stored_doc_vector_mixed_backends: None,
        zoekt,
        qdrant,
        scip,
        manifest: Some(manifest),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SidecarLayout;

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
}
