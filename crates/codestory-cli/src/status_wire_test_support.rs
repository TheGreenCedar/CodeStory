use codestory_retrieval::{
    ComponentHealth, ComponentStatus, RetrievalManifestContractReport,
    RetrievalManifestLaneProvenance, RetrievalStatusReport, SidecarCapabilities,
};
use serde_json::Value;
use std::collections::BTreeSet;

pub(crate) const DOCTOR_GOLDEN: &str =
    include_str!("../tests/fixtures/retrieval_status/doctor.json");
pub(crate) const REPORT_GOLDEN: &str =
    include_str!("../tests/fixtures/retrieval_status/report.json");
pub(crate) const RETRIEVAL_GOLDEN: &str =
    include_str!("../tests/fixtures/retrieval_status/retrieval.json");
pub(crate) const STDIO_GOLDEN: &str =
    include_str!("../tests/fixtures/retrieval_status/stdio-diagnostics.json");

pub(crate) const RAW_STATUS_FIELDS: [&str; 22] = [
    "degraded_reason",
    "embedding_accelerator_request_device",
    "embedding_accelerator_request_provider",
    "embedding_accelerator_requested",
    "embedding_cpu_allowed",
    "embedding_detected_gpu",
    "embedding_detected_provider",
    "embedding_device_observation_source",
    "embedding_device_policy",
    "embedding_device_state",
    "lexical",
    "manifest",
    "manifest_contract",
    "manifest_vector_embedding_backend",
    "manifest_vector_embedding_dim",
    "query_embedding_backend",
    "retrieval_mode",
    "scip",
    "semantic",
    "stored_doc_vector_dim",
    "stored_doc_vector_mixed_backends",
    "stored_doc_vector_producer_backend",
];

pub(crate) const DOCTOR_STATUS_FIELDS: [&str; 19] = [
    "degraded_reason",
    "embedding_accelerator_request_device",
    "embedding_accelerator_request_provider",
    "embedding_accelerator_requested",
    "embedding_cpu_allowed",
    "embedding_detected_gpu",
    "embedding_detected_provider",
    "embedding_device_observation_source",
    "embedding_device_policy",
    "embedding_device_state",
    "manifest_generation",
    "manifest_input_hash",
    "precise_semantic_import_producer",
    "precise_semantic_import_reason",
    "precise_semantic_import_revision",
    "precise_semantic_import_status",
    "profile",
    "retrieval_mode",
    "run_id",
];

pub(crate) const REPORT_STATUS_FIELDS: [&str; 13] = [
    "degraded_reason",
    "embedding_accelerator_request_device",
    "embedding_accelerator_request_provider",
    "embedding_accelerator_requested",
    "embedding_cpu_allowed",
    "embedding_detected_gpu",
    "embedding_detected_provider",
    "embedding_device_observation_source",
    "embedding_device_policy",
    "embedding_device_state",
    "manifest_generation",
    "manifest_input_hash",
    "retrieval_mode",
];

pub(crate) const DIAGNOSTIC_FIELDS: [&str; 4] = [
    "degraded_reason",
    "embedding_server",
    "engine",
    "retrieval_mode",
];

pub(crate) fn healthy_status_report() -> RetrievalStatusReport {
    let mut manifest = codestory_retrieval::test_support::retrieval_manifest_fixture(
        "golden-project",
        "golden-input",
    );
    manifest.built_at_epoch_ms = 1_725_000_000_000;
    manifest.disk_bytes = Some(4_096);
    manifest.degraded_modes_json = "[]".to_string();
    manifest.embedding_backend = Some("manifest-backend".to_string());
    manifest.embedding_dim = Some(768);
    manifest.sidecar_input_hash = Some("golden-input".to_string());
    manifest.sidecar_generation = Some("golden-generation".to_string());
    manifest.projection_count = Some(13);
    manifest.symbol_doc_count = Some(8);
    manifest.dense_projection_count = Some(5);
    manifest.graph_artifact_hash = Some("golden-graph-hash".to_string());
    manifest.dense_reason_counts_json = Some(r#"{"public_api":5}"#.to_string());
    manifest.precise_semantic_import_status = Some("fresh".to_string());
    manifest.precise_semantic_import_reason = Some("golden-import-reason".to_string());
    manifest.precise_semantic_import_revision = Some("golden-import-revision".to_string());
    manifest.precise_semantic_import_producer = Some("golden-import-producer".to_string());

    RetrievalStatusReport {
        retrieval_mode: "full".to_string(),
        degraded_reason: None,
        query_embedding_backend: "query-backend".to_string(),
        manifest_vector_embedding_backend: Some("manifest-backend".to_string()),
        manifest_vector_embedding_dim: Some(768),
        stored_doc_vector_producer_backend: Some("stored-backend".to_string()),
        stored_doc_vector_dim: Some(768),
        stored_doc_vector_mixed_backends: Some(false),
        embedding_device_policy: "accelerator_required".to_string(),
        embedding_device_state: "accelerated".to_string(),
        embedding_device_observation_source: "per_user_server".to_string(),
        embedding_detected_provider: Some("metal".to_string()),
        embedding_detected_gpu: Some("golden-gpu".to_string()),
        embedding_accelerator_requested: true,
        embedding_accelerator_request_provider: Some("metal".to_string()),
        embedding_accelerator_request_device: Some("gpu:0".to_string()),
        embedding_cpu_allowed: false,
        lexical: ComponentHealth {
            name: "lexical".to_string(),
            status: ComponentStatus::Healthy,
            latency_ms: Some(2),
            detail: "lexical ready".to_string(),
            degraded_reason: None,
            capabilities: SidecarCapabilities {
                lexical: true,
                semantic: false,
                graph: false,
            },
        },
        semantic: ComponentHealth {
            name: "semantic".to_string(),
            status: ComponentStatus::Healthy,
            latency_ms: Some(3),
            detail: "semantic ready".to_string(),
            degraded_reason: None,
            capabilities: SidecarCapabilities {
                lexical: false,
                semantic: true,
                graph: false,
            },
        },
        scip: ComponentHealth {
            name: "scip".to_string(),
            status: ComponentStatus::Healthy,
            latency_ms: Some(4),
            detail: "scip ready".to_string(),
            degraded_reason: None,
            capabilities: SidecarCapabilities {
                lexical: false,
                semantic: false,
                graph: true,
            },
        },
        manifest_contract: Some(RetrievalManifestContractReport {
            source_root: "/golden/project".to_string(),
            project_id: "golden-project".to_string(),
            input_hash: Some("golden-input".to_string()),
            generation: Some("golden-generation".to_string()),
            schema_version: Some(7),
            graph_hash: Some("golden-graph-hash".to_string()),
            symbol_doc_count: Some(8),
            dense_anchor_count: Some(5),
            degraded_modes: Vec::new(),
            retrieval_mode: "full".to_string(),
            degraded_reason: None,
            lanes: vec![RetrievalManifestLaneProvenance {
                lane: "semantic".to_string(),
                producer: "golden-producer".to_string(),
                provenance: "golden-provenance".to_string(),
                count: Some(5),
                status: "ready".to_string(),
            }],
        }),
        manifest: Some(manifest),
    }
}

pub(crate) fn degraded_status_report() -> RetrievalStatusReport {
    let mut report = healthy_status_report();
    report.degraded_reason = Some("semantic_store_degraded".to_string());
    report.embedding_device_state = "degraded".to_string();
    report.semantic.status = ComponentStatus::Degraded;
    report.semantic.detail = "semantic degraded".to_string();
    report.semantic.degraded_reason = Some("semantic_store_degraded".to_string());
    report
}

pub(crate) fn unavailable_status_report() -> RetrievalStatusReport {
    RetrievalStatusReport {
        retrieval_mode: "unavailable".to_string(),
        degraded_reason: Some("retrieval_manifest_missing".to_string()),
        query_embedding_backend: "query-backend".to_string(),
        manifest_vector_embedding_backend: None,
        manifest_vector_embedding_dim: None,
        stored_doc_vector_producer_backend: None,
        stored_doc_vector_dim: None,
        stored_doc_vector_mixed_backends: None,
        embedding_device_policy: "accelerator_required".to_string(),
        embedding_device_state: "unknown".to_string(),
        embedding_device_observation_source: "retrieval_unobserved".to_string(),
        embedding_detected_provider: None,
        embedding_detected_gpu: None,
        embedding_accelerator_requested: false,
        embedding_accelerator_request_provider: None,
        embedding_accelerator_request_device: None,
        embedding_cpu_allowed: false,
        lexical: ComponentHealth {
            name: "lexical".to_string(),
            status: ComponentStatus::Unavailable,
            latency_ms: None,
            detail: "lexical unavailable".to_string(),
            degraded_reason: Some("retrieval_manifest_missing".to_string()),
            capabilities: SidecarCapabilities {
                lexical: false,
                semantic: false,
                graph: false,
            },
        },
        semantic: ComponentHealth {
            name: "semantic".to_string(),
            status: ComponentStatus::Unavailable,
            latency_ms: None,
            detail: "semantic unavailable".to_string(),
            degraded_reason: Some("retrieval_manifest_missing".to_string()),
            capabilities: SidecarCapabilities {
                lexical: false,
                semantic: false,
                graph: false,
            },
        },
        scip: ComponentHealth {
            name: "scip".to_string(),
            status: ComponentStatus::Unavailable,
            latency_ms: None,
            detail: "scip unavailable".to_string(),
            degraded_reason: Some("retrieval_manifest_missing".to_string()),
            capabilities: SidecarCapabilities {
                lexical: false,
                semantic: false,
                graph: false,
            },
        },
        manifest_contract: None,
        manifest: None,
    }
}

pub(crate) fn status_runtime_config() -> codestory_retrieval::SidecarRuntimeConfig {
    crate::sidecar_runtime::local().with_profile_and_run_id(
        None,
        codestory_retrieval::SidecarProfile::Agent,
        Some("golden-run"),
    )
}

pub(crate) fn probe_error() -> anyhow::Error {
    anyhow::anyhow!("golden retrieval probe failed")
}

pub(crate) fn assert_exact_fields(value: &Value, expected: &[&str], label: &str) {
    let actual = value
        .as_object()
        .unwrap_or_else(|| panic!("{label} must be an object"))
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let expected = expected.iter().copied().collect::<BTreeSet<_>>();
    assert_eq!(actual, expected, "{label} field set drifted");
}

pub(crate) fn assert_non_null_coverage(cases: &[Value], expected: &[&str], label: &str) {
    for field in expected {
        assert!(
            cases
                .iter()
                .any(|case| case.get(field).is_some_and(|value| !value.is_null())),
            "{label} field `{field}` is null or absent in every golden case"
        );
    }
}

pub(crate) fn assert_json_golden(actual: &Value, expected: &str, label: &str) {
    let expected: Value = serde_json::from_str(expected)
        .unwrap_or_else(|error| panic!("parse {label} golden: {error}"));
    assert_eq!(*actual, expected, "{label} wire behavior drifted");
}
