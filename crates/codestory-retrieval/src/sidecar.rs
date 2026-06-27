use crate::config::{
    SidecarImagePins, SidecarLayout, SidecarProfile, SidecarRuntimeConfig,
    default_sidecar_image_pins, sidecar_runtime_auto, sidecar_runtime_for_project,
};
use crate::generation::{
    SIDECAR_SEMANTIC_DOC_CONTRACT_CHANGED, manifest_has_current_sidecar_contract,
    manifest_staleness_reason, manifest_unavailable_reason,
};
use crate::health::{
    EmbeddingLaunchMetadata, RetrievalStatusReport, attach_manifest_contract, attach_repair_hint,
    probe_sidecar_health_with_embedding_device, unavailable_status_report_with_embedding_device,
};
use crate::index::{compute_sidecar_input_fingerprint, sidecar_project_id_for_root};
use anyhow::{Context, Result};
use codestory_contracts::language_support::{
    LanguageSupportMode, language_support_profile_for_ext,
};
use codestory_store::Store;
use codestory_workspace::{RefreshInputs, StoredFileState, WorkspaceManifest};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Runtime state file written by `sidecar_up`.
///
/// The file records local sidecar endpoints and data roots only. It is not a readiness manifest;
/// callers must use `sidecar_status` or `strict_sidecar_status` before trusting retrieval output.
pub struct SidecarStateFile {
    #[serde(default = "default_sidecar_owner")]
    pub owner: String,
    #[serde(default = "default_sidecar_profile")]
    pub profile: String,
    #[serde(default = "default_sidecar_namespace")]
    pub namespace: String,
    #[serde(default = "default_sidecar_namespace")]
    pub compose_project: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    pub zoekt_http_port: u16,
    pub qdrant_http_port: u16,
    pub qdrant_grpc_port: u16,
    #[serde(default = "default_embed_http_port")]
    pub embed_http_port: u16,
    #[serde(default = "default_embed_url")]
    pub embed_url: String,
    #[serde(default = "default_embedding_device_policy")]
    pub embedding_device_policy: String,
    #[serde(default = "default_embedding_device_state")]
    pub embedding_device_state: String,
    #[serde(default = "default_embedding_device_observation_source")]
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
    #[serde(default)]
    pub embedding_cpu_allowed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding_launch: Option<EmbeddingLaunchMetadata>,
    #[serde(default = "default_sidecar_image_pins")]
    pub sidecar_images: SidecarImagePins,
    pub zoekt_data_dir: String,
    pub qdrant_data_dir: String,
    pub scip_artifacts_root: String,
    #[serde(default)]
    pub compose_file: Option<String>,
    #[serde(default)]
    pub cleanup_command: String,
    pub started_at_epoch_ms: i64,
}

pub fn sidecar_up() -> Result<SidecarStateFile> {
    sidecar_up_with_runtime(&SidecarRuntimeConfig::local(), None)
}

pub fn sidecar_up_with_runtime(
    runtime: &SidecarRuntimeConfig,
    compose_file: Option<&Path>,
) -> Result<SidecarStateFile> {
    sidecar_up_with_runtime_and_launch_metadata(runtime, compose_file, None)
}

pub(crate) fn sidecar_up_with_runtime_and_launch_metadata(
    runtime: &SidecarRuntimeConfig,
    compose_file: Option<&Path>,
    embedding_launch: Option<EmbeddingLaunchMetadata>,
) -> Result<SidecarStateFile> {
    let layout = &runtime.layout;
    layout.ensure_data_dirs()?;
    let embedding_device = crate::embeddings::embedding_device_readiness();
    let state = SidecarStateFile {
        owner: "codestory".into(),
        profile: runtime.profile.as_str().into(),
        namespace: runtime.namespace.clone(),
        compose_project: runtime.compose_project.clone(),
        run_id: runtime.run_id.clone(),
        zoekt_http_port: layout.zoekt_http_port,
        qdrant_http_port: layout.qdrant_http_port,
        qdrant_grpc_port: layout.qdrant_grpc_port,
        embed_http_port: runtime.embed_http_port,
        embed_url: SidecarLayout::embed_base_url(runtime.embed_http_port),
        embedding_device_policy: embedding_device.requested_policy.into(),
        embedding_device_state: embedding_device.observed_state.into(),
        embedding_device_observation_source: embedding_device.observation_source.into(),
        embedding_detected_provider: embedding_device.detected_provider,
        embedding_detected_gpu: embedding_device.detected_gpu,
        embedding_accelerator_requested: embedding_device.accelerator_requested,
        embedding_accelerator_request_provider: embedding_device.accelerator_request_provider,
        embedding_accelerator_request_device: embedding_device.accelerator_request_device,
        embedding_cpu_allowed: embedding_device.cpu_allowed,
        embedding_launch,
        sidecar_images: default_sidecar_image_pins(),
        zoekt_data_dir: layout.zoekt_data_dir.display().to_string(),
        qdrant_data_dir: layout.qdrant_data_dir.display().to_string(),
        scip_artifacts_root: layout.scip_artifacts_root.display().to_string(),
        compose_file: compose_file.map(|path| path.display().to_string()),
        cleanup_command: runtime.cleanup_command.clone(),
        started_at_epoch_ms: chrono::Utc::now().timestamp_millis(),
    };
    let json = serde_json::to_string_pretty(&state).context("serialize sidecar state")?;
    std::fs::write(&layout.state_file, json).context("write retrieval-sidecars.json")?;
    Ok(state)
}

pub fn sidecar_down() -> Result<()> {
    sidecar_down_for_runtime(&SidecarRuntimeConfig::local())
}

pub fn sidecar_down_for_project(project_root: &Path, profile: SidecarProfile) -> Result<()> {
    sidecar_down_for_runtime(&sidecar_runtime_for_project(project_root, profile))
}

pub fn sidecar_down_for_runtime(runtime: &SidecarRuntimeConfig) -> Result<()> {
    let layout = &runtime.layout;
    if layout.state_file.exists() {
        if runtime.profile == SidecarProfile::Agent
            && let Some(state) = std::fs::read_to_string(&layout.state_file)
                .ok()
                .and_then(|contents| serde_json::from_str::<SidecarStateFile>(&contents).ok())
            && state.owner == "codestory"
            && state.namespace == runtime.namespace
        {
            crate::compose::docker_compose_down_for_state(&state)?;
        }
        std::fs::remove_file(&layout.state_file).context("remove retrieval-sidecars.json")?;
    }
    Ok(())
}

/// Probe sidecar health and attach the latest retrieval manifest when storage is available.
///
/// A healthy infrastructure report is still weaker than strict readiness: it may show running
/// services while the manifest is stale for the current worktree.
pub fn sidecar_status(
    project_root: &Path,
    storage_path: Option<&Path>,
) -> Result<RetrievalStatusReport> {
    sidecar_status_inner(project_root, storage_path, false)
}

/// Probe sidecar health and fail stale manifest identity checks.
///
/// This is the status surface to use before serving `retrieval_mode=full` packet/search evidence.
pub fn strict_sidecar_status(
    project_root: &Path,
    storage_path: Option<&Path>,
) -> Result<RetrievalStatusReport> {
    sidecar_status_inner(project_root, storage_path, true)
}

pub fn strict_sidecar_status_for_profile(
    project_root: &Path,
    storage_path: Option<&Path>,
    profile: SidecarProfile,
) -> Result<RetrievalStatusReport> {
    strict_sidecar_status_for_runtime(
        project_root,
        storage_path,
        sidecar_runtime_for_project(project_root, profile),
    )
}

pub fn strict_sidecar_status_for_runtime(
    project_root: &Path,
    storage_path: Option<&Path>,
    runtime: SidecarRuntimeConfig,
) -> Result<RetrievalStatusReport> {
    sidecar_status_inner_with_runtime(project_root, storage_path, true, runtime)
}

fn sidecar_status_inner(
    project_root: &Path,
    storage_path: Option<&Path>,
    strict: bool,
) -> Result<RetrievalStatusReport> {
    let runtime = sidecar_runtime_auto(project_root);
    sidecar_status_inner_with_runtime(project_root, storage_path, strict, runtime)
}

fn sidecar_status_inner_with_runtime(
    project_root: &Path,
    storage_path: Option<&Path>,
    strict: bool,
    runtime: SidecarRuntimeConfig,
) -> Result<RetrievalStatusReport> {
    runtime.activate_embed_url_default();
    let layout = runtime.layout.clone();
    let embedding_device = crate::embeddings::embedding_device_readiness_for_runtime(&runtime);
    let project_id = sidecar_project_id_for_root(project_root);
    let manifest = if let Some(path) = storage_path.filter(|path| path.exists()) {
        let storage = Store::open(path).context("open storage for manifest")?;
        let manifest = storage
            .get_retrieval_index_manifest(&project_id)
            .context("load retrieval manifest")?;
        if strict
            && let Some(manifest) = manifest.as_ref()
            && let Some(reason) = strict_readiness_unavailable_reason(
                project_root,
                path,
                &storage,
                &project_id,
                manifest,
            )
            .context("check strict sidecar readiness")?
        {
            return Ok(attach_status_ownership(
                enrich_status_with_semantic_doc_stats(
                    attach_repair_hint(
                        attach_manifest_contract(
                            unavailable_status_report_with_embedding_device(
                                format!("sidecar_manifest_stale: {reason}"),
                                Some(manifest.clone()),
                                &embedding_device,
                            ),
                            project_root,
                        ),
                        project_root,
                        Some(&runtime),
                    ),
                    &storage,
                ),
                &runtime,
            ));
        }
        if let Some(manifest) = manifest.as_ref()
            && let Some(reason) = manifest_unavailable_reason(&project_id, &storage, manifest)
        {
            return Ok(attach_status_ownership(
                enrich_status_with_semantic_doc_stats(
                    attach_repair_hint(
                        attach_manifest_contract(
                            unavailable_status_report_with_embedding_device(
                                reason,
                                Some(manifest.clone()),
                                &embedding_device,
                            ),
                            project_root,
                        ),
                        project_root,
                        Some(&runtime),
                    ),
                    &storage,
                ),
                &runtime,
            ));
        }
        let report = probe_sidecar_health_with_embedding_device(
            &layout,
            &project_id,
            manifest,
            &embedding_device,
        );
        return Ok(attach_status_ownership(
            enrich_status_with_semantic_doc_stats(
                attach_repair_hint(
                    attach_manifest_contract(report, project_root),
                    project_root,
                    Some(&runtime),
                ),
                &storage,
            ),
            &runtime,
        ));
    } else {
        None
    };
    Ok(attach_status_ownership(
        attach_repair_hint(
            attach_manifest_contract(
                probe_sidecar_health_with_embedding_device(
                    &layout,
                    &project_id,
                    manifest,
                    &embedding_device,
                ),
                project_root,
            ),
            project_root,
            Some(&runtime),
        ),
        &runtime,
    ))
}

fn attach_status_ownership(
    mut report: RetrievalStatusReport,
    runtime: &SidecarRuntimeConfig,
) -> RetrievalStatusReport {
    report.ownership = Some(runtime.ownership());
    if let Some(state) = read_sidecar_state(&runtime.layout.state_file) {
        report.embedding_launch = state.embedding_launch.or(report.embedding_launch);
        report.sidecar_images = state.sidecar_images;
    }
    report
}

fn read_sidecar_state(path: &Path) -> Option<SidecarStateFile> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|contents| serde_json::from_str::<SidecarStateFile>(&contents).ok())
}

fn enrich_status_with_semantic_doc_stats(
    mut report: RetrievalStatusReport,
    storage: &Store,
) -> RetrievalStatusReport {
    if let Ok(stats) = storage.get_llm_symbol_doc_stats() {
        report.stored_doc_vector_producer_backend = stats.embedding_backend;
        report.stored_doc_vector_dim = stats.embedding_dim;
        report.stored_doc_vector_mixed_backends = Some(stats.mixed_embedding_backends);
    }
    report
}

pub(crate) fn validate_strict_sidecar_readiness(
    project_root: &Path,
    storage_path: &Path,
    storage: &Store,
) -> Result<()> {
    let project_id = sidecar_project_id_for_root(project_root);
    let Some(manifest) = storage
        .get_retrieval_index_manifest(&project_id)
        .context("load retrieval manifest for strict readiness")?
    else {
        return Ok(());
    };
    if let Some(reason) = strict_readiness_unavailable_reason(
        project_root,
        storage_path,
        storage,
        &project_id,
        &manifest,
    )? {
        anyhow::bail!("sidecar_manifest_stale: {reason}");
    }
    Ok(())
}

fn strict_readiness_unavailable_reason(
    project_root: &Path,
    storage_path: &Path,
    storage: &Store,
    project_id: &str,
    manifest: &codestory_store::RetrievalIndexManifest,
) -> Result<Option<String>> {
    if !manifest_has_current_sidecar_contract(project_id, manifest) {
        return Ok(None);
    }
    if let Some(reason) = manifest_staleness_reason(storage, manifest)
        && manifest_contract_drift_should_win(&reason)
    {
        return Ok(None);
    }

    let embedding_backend = crate::embeddings::embedding_runtime_id();
    let expected_doc_backend = crate::embeddings::embedding_backend_label();
    if let Ok(stats) = storage.get_llm_symbol_doc_stats() {
        if stats.mixed_embedding_backends {
            return Ok(Some("sidecar_symbol_docs_mixed_embedding_backends".into()));
        }
        if stats
            .embedding_backend
            .as_deref()
            .is_some_and(|backend| backend != expected_doc_backend)
        {
            return Ok(Some(format!(
                "sidecar_symbol_doc_embedding_backend_changed: stored={} current={}",
                stats.embedding_backend.as_deref().unwrap_or("<missing>"),
                expected_doc_backend
            )));
        }
    }
    let embedding_dim = i32::try_from(crate::embeddings::qdrant_vector_dim())
        .unwrap_or(crate::embeddings::RETRIEVAL_EMBEDDING_DIM as i32);
    let current_input = compute_sidecar_input_fingerprint(
        storage,
        storage_path,
        project_root,
        project_id,
        &embedding_backend,
        embedding_dim,
    )
    .context("compute strict sidecar input fingerprint")?;
    if manifest.sidecar_input_hash.as_deref() == Some(current_input.hash.as_str())
        && manifest.projection_count == Some(current_input.projection_count)
        && manifest.symbol_doc_count == Some(current_input.symbol_doc_count)
        && manifest.dense_projection_count == Some(current_input.dense_projection_count)
        && manifest.semantic_policy_version == current_input.semantic_policy_version
        && manifest.graph_artifact_hash.as_deref()
            == Some(current_input.graph_artifact_hash.as_str())
        && manifest.dense_reason_counts_json.as_deref()
            == Some(current_input.dense_reason_counts_json.as_str())
    {
        return Ok(None);
    }

    let workspace = WorkspaceManifest::open(project_root.to_path_buf())
        .context("open workspace manifest for strict sidecar readiness")?;
    let files = storage.files().get_files().context("load indexed files")?;
    let refresh_inputs = RefreshInputs {
        stored_files: files
            .into_iter()
            .map(|file| StoredFileState {
                id: file.id,
                path: file.path,
                modification_time: file.modification_time,
                indexed: file.indexed,
            })
            .collect(),
        inventory: Default::default(),
    };
    let plan = workspace
        .build_execution_plan(&refresh_inputs)
        .context("build strict sidecar freshness plan")?;
    if let Some(path) = plan
        .files_to_index
        .iter()
        .find(|path| graph_indexed_source_path(path))
    {
        return Ok(Some(format!(
            "indexable_file_added_or_changed_after_sidecar_manifest: {}",
            path.display()
        )));
    }
    if let Some(file_id) = plan.files_to_remove.first() {
        return Ok(Some(format!(
            "indexed_file_removed_after_sidecar_manifest: file_id={file_id}"
        )));
    }
    Ok(Some(format!(
        "sidecar_input_hash_changed: manifest={} current={}; symbol_doc_count manifest={} current={}; dense_projection_count manifest={} current={}; projection_count manifest={} current={}",
        manifest
            .sidecar_input_hash
            .as_deref()
            .unwrap_or("<missing>"),
        current_input.hash,
        manifest
            .symbol_doc_count
            .map(|count| count.to_string())
            .unwrap_or_else(|| "<missing>".into()),
        current_input.symbol_doc_count,
        manifest
            .dense_projection_count
            .map(|count| count.to_string())
            .unwrap_or_else(|| "<missing>".into()),
        current_input.dense_projection_count,
        manifest
            .projection_count
            .map(|count| count.to_string())
            .unwrap_or_else(|| "<missing>".into()),
        current_input.projection_count
    )))
}

fn graph_indexed_source_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .and_then(language_support_profile_for_ext)
        .is_some_and(|profile| profile.support_mode == LanguageSupportMode::ParserBackedGraph)
}

fn manifest_contract_drift_should_win(reason: &str) -> bool {
    reason.contains("sidecar_embedding_backend_changed")
        || reason.contains("sidecar_embedding_dim_changed")
        || reason == SIDECAR_SEMANTIC_DOC_CONTRACT_CHANGED
}

fn default_sidecar_owner() -> String {
    "codestory".into()
}

fn default_sidecar_profile() -> String {
    "local".into()
}

fn default_sidecar_namespace() -> String {
    "codestory".into()
}

fn default_embed_http_port() -> u16 {
    crate::config::DEFAULT_EMBED_HTTP_PORT
}

fn default_embed_url() -> String {
    SidecarLayout::embed_base_url(crate::config::DEFAULT_EMBED_HTTP_PORT)
}

fn default_embedding_device_policy() -> String {
    "accelerator_required".into()
}

fn default_embedding_device_state() -> String {
    "unknown".into()
}

fn default_embedding_device_observation_source() -> String {
    "sidecar_unobserved".into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generation::{
        SIDECAR_SCHEMA_VERSION, sidecar_generation_id, sidecar_qdrant_collection,
    };
    use crate::index::{compute_sidecar_input_fingerprint, project_id_for_root};
    use crate::test_support::retrieval_manifest_fixture;
    use codestory_contracts::graph::{Node, NodeId, NodeKind};
    use codestory_store::{FileInfo, FileRole, LlmSymbolDoc};
    use std::collections::BTreeMap;
    use std::ffi::OsString;
    use tempfile::TempDir;

    fn test_runtime(root: &TempDir) -> SidecarRuntimeConfig {
        SidecarRuntimeConfig {
            layout: SidecarLayout {
                zoekt_http_port: 16070,
                qdrant_http_port: 16333,
                qdrant_grpc_port: 16334,
                zoekt_data_dir: root.path().join("zoekt"),
                qdrant_data_dir: root.path().join("qdrant"),
                scip_artifacts_root: root.path().join("scip"),
                state_file: root.path().join("retrieval-sidecars.json"),
            },
            profile: SidecarProfile::Local,
            run_id: None,
            namespace: "test".to_string(),
            compose_project: "test".to_string(),
            embed_http_port: 18080,
            cleanup_command: "codestory-cli retrieval down".to_string(),
            labels: BTreeMap::new(),
        }
    }

    fn semantic_doc_with_backend(backend: &str) -> LlmSymbolDoc {
        LlmSymbolDoc {
            node_id: NodeId(1),
            file_node_id: None,
            kind: NodeKind::FUNCTION,
            display_name: "do_work".into(),
            qualified_name: Some("pkg::do_work".into()),
            file_path: Some("src/lib.rs".into()),
            start_line: Some(1),
            doc_text: "semantic doc".into(),
            doc_version: 5,
            doc_hash: "doc-hash".into(),
            embedding_profile: Some("bge-base-en-v1.5".into()),
            embedding_model: format!("BAAI/bge-base-en-v1.5-local|backend={backend}"),
            embedding_backend: Some(backend.into()),
            embedding_dim: crate::embeddings::RETRIEVAL_EMBEDDING_DIM as u32,
            doc_shape: Some("semantic_doc_version=5;scope=durable_symbols".into()),
            semantic_policy_version: Some(crate::generation::SEMANTIC_POLICY_VERSION.into()),
            dense_reason: Some("public_api".into()),
            embedding: vec![0.01; crate::embeddings::RETRIEVAL_EMBEDDING_DIM],
            updated_at_epoch_ms: 123,
        }
    }

    #[test]
    fn status_attaches_embedding_launch_metadata_from_state_file() {
        let root = TempDir::new().expect("root");
        let runtime = test_runtime(&root);
        let launch = EmbeddingLaunchMetadata {
            provider: "llamacpp".to_string(),
            launch_mode: "native_spawned".to_string(),
            endpoint: "http://127.0.0.1:18080/v1/embeddings".to_string(),
            executable_source: Some("managed_cache".to_string()),
            executable_path: Some("C:/cache/llama-server.exe".to_string()),
            model_path: Some("C:/cache/bge-base-en-v1.5.Q8_0.gguf".to_string()),
            requested_device: Some("Vulkan0".to_string()),
        };
        let state =
            sidecar_up_with_runtime_and_launch_metadata(&runtime, None, Some(launch.clone()))
                .expect("write state");
        assert_eq!(state.embedding_launch, Some(launch.clone()));

        let report = unavailable_status_report_with_embedding_device(
            "missing",
            None,
            &crate::embeddings::embedding_device_readiness(),
        );
        let report = attach_status_ownership(report, &runtime);

        assert_eq!(report.embedding_launch, Some(launch));
    }

    #[test]
    fn status_rejects_stale_manifest_before_component_probes() {
        let project = TempDir::new().expect("project");
        let storage_dir = TempDir::new().expect("storage");
        let storage_path = storage_dir.path().join("codestory.db");
        let project_id = project_id_for_root(project.path());
        let hash = "deadbeefcafebabe";
        {
            let mut storage = Store::open(&storage_path).expect("open db");
            let mut manifest = retrieval_manifest_fixture(&project_id, hash);
            manifest.projection_count = Some(10);
            manifest.symbol_doc_count = Some(10);
            manifest.dense_projection_count = Some(10);
            manifest.dense_reason_counts_json = Some("{\"public_api\":10}".into());
            storage
                .upsert_retrieval_index_manifest(&manifest)
                .expect("manifest");
        }

        let report = strict_sidecar_status(project.path(), Some(&storage_path))
            .expect("sidecar status report");

        assert_eq!(report.retrieval_mode, "unavailable");
        assert!(
            report
                .degraded_reason
                .as_deref()
                .unwrap_or_default()
                .contains("sidecar_manifest_stale")
        );
    }

    #[test]
    fn strict_readiness_rejects_stored_doc_backend_mismatch() {
        let _lock = crate::test_support::env_lock();
        let _backend = EnvGuard::set("CODESTORY_EMBED_BACKEND", "llamacpp");
        let project = TempDir::new().expect("project");
        let storage_dir = TempDir::new().expect("storage");
        let storage_path = storage_dir.path().join("codestory.db");
        let project_id = project_id_for_root(project.path());
        let hash = "badc0ffee0ddf00d";
        let mut manifest = retrieval_manifest_fixture(&project_id, hash);
        manifest.projection_count = Some(1);
        manifest.dense_projection_count = Some(1);
        manifest.dense_reason_counts_json = Some("{\"public_api\":1}".into());

        let mut storage = Store::open(&storage_path).expect("open db");
        storage
            .insert_nodes_batch(&[Node {
                id: NodeId(1),
                kind: NodeKind::FUNCTION,
                serialized_name: "do_work".into(),
                ..Default::default()
            }])
            .expect("node");
        storage
            .upsert_llm_symbol_docs_batch(&[semantic_doc_with_backend("onnx")])
            .expect("semantic doc");

        let reason = strict_readiness_unavailable_reason(
            project.path(),
            &storage_path,
            &storage,
            &project_id,
            &manifest,
        )
        .expect("strict readiness")
        .expect("backend mismatch should degrade");

        assert!(
            reason.contains("sidecar_symbol_doc_embedding_backend_changed"),
            "unexpected reason: {reason}"
        );
        assert!(
            reason.contains("stored=onnx current=llamacpp"),
            "unexpected reason: {reason}"
        );
    }

    #[test]
    fn status_rejects_manifest_when_live_indexed_file_changes_or_is_removed() {
        let _lock = crate::test_support::env_lock();
        let _backend = EnvGuard::set("CODESTORY_EMBED_BACKEND", "llamacpp");
        let project = TempDir::new().expect("project");
        let storage_dir = TempDir::new().expect("storage");
        let storage_path = storage_dir.path().join("codestory.db");
        let source_path = project.path().join("src").join("lib.rs");
        std::fs::create_dir_all(source_path.parent().expect("source parent"))
            .expect("create source parent");
        std::fs::write(&source_path, "pub fn indexed() {}\n").expect("write source");
        let indexed_mtime = live_mtime_millis(&source_path);
        let project_id = project_id_for_root(project.path());
        let hash = "feedfacecafebeef";
        {
            let mut storage = Store::open(&storage_path).expect("open db");
            storage
                .insert_file(&FileInfo {
                    id: 1,
                    path: source_path.clone(),
                    language: "rust".into(),
                    modification_time: indexed_mtime,
                    indexed: true,
                    complete: true,
                    line_count: 1,
                    file_role: FileRole::Source,
                })
                .expect("insert indexed file");
            let mut manifest = retrieval_manifest_fixture(&project_id, hash);
            manifest.built_at_epoch_ms = indexed_mtime;
            storage
                .upsert_retrieval_index_manifest(&manifest)
                .expect("manifest");
        }

        std::thread::sleep(std::time::Duration::from_millis(5));
        std::fs::write(&source_path, "pub fn indexed() -> usize { 1 }\n").expect("mutate source");
        let changed = strict_sidecar_status(project.path(), Some(&storage_path))
            .expect("changed sidecar status");
        assert_eq!(changed.retrieval_mode, "unavailable");
        assert!(
            changed
                .degraded_reason
                .as_deref()
                .unwrap_or_default()
                .contains("indexable_file_added_or_changed_after_sidecar_manifest"),
            "changed indexed file should make sidecar status fail closed: {changed:?}"
        );

        std::fs::remove_file(&source_path).expect("remove source");
        let removed = strict_sidecar_status(project.path(), Some(&storage_path))
            .expect("removed sidecar status");
        assert_eq!(removed.retrieval_mode, "unavailable");
        assert!(
            removed
                .degraded_reason
                .as_deref()
                .unwrap_or_default()
                .contains("indexed_file_removed_after_sidecar_manifest"),
            "removed indexed file should make sidecar status fail closed: {removed:?}"
        );
    }

    #[test]
    fn lightweight_status_does_not_scan_live_indexable_inventory() {
        let _lock = crate::test_support::env_lock();
        let _backend = EnvGuard::set("CODESTORY_EMBED_BACKEND", "llamacpp");
        let project = TempDir::new().expect("project");
        let storage_dir = TempDir::new().expect("storage");
        let storage_path = storage_dir.path().join("codestory.db");
        let source_path = project.path().join("src").join("lib.rs");
        std::fs::create_dir_all(source_path.parent().expect("source parent"))
            .expect("create source parent");
        std::fs::write(&source_path, "pub fn indexed() {}\n").expect("write source");
        let indexed_mtime = live_mtime_millis(&source_path);
        let project_id = project_id_for_root(project.path());
        let hash = "1ead1e55cafebeef";
        {
            let mut storage = Store::open(&storage_path).expect("open db");
            storage
                .insert_file(&FileInfo {
                    id: 1,
                    path: source_path.clone(),
                    language: "rust".into(),
                    modification_time: indexed_mtime,
                    indexed: true,
                    complete: true,
                    line_count: 1,
                    file_role: FileRole::Source,
                })
                .expect("insert indexed file");
            let mut manifest = retrieval_manifest_fixture(&project_id, hash);
            manifest.built_at_epoch_ms = indexed_mtime;
            storage
                .upsert_retrieval_index_manifest(&manifest)
                .expect("manifest");
        }

        std::fs::write(
            project.path().join("src").join("new_module.rs"),
            "pub fn newly_added() {}\n",
        )
        .expect("write new source");

        let lightweight =
            sidecar_status(project.path(), Some(&storage_path)).expect("lightweight status");
        let strict =
            strict_sidecar_status(project.path(), Some(&storage_path)).expect("strict status");

        assert!(
            !lightweight
                .degraded_reason
                .as_deref()
                .unwrap_or_default()
                .contains("indexable_file_added_or_changed_after_sidecar_manifest"),
            "lightweight status should leave live inventory scans to strict callers: {lightweight:?}"
        );
        assert!(
            strict
                .degraded_reason
                .as_deref()
                .unwrap_or_default()
                .contains("sidecar_manifest_stale"),
            "strict status should fail closed on new indexable files: {strict:?}"
        );
    }

    #[test]
    fn strict_status_rejects_manifest_when_new_indexable_file_is_added() {
        let _lock = crate::test_support::env_lock();
        let _backend = EnvGuard::set("CODESTORY_EMBED_BACKEND", "llamacpp");
        let project = TempDir::new().expect("project");
        let storage_dir = TempDir::new().expect("storage");
        let storage_path = storage_dir.path().join("codestory.db");
        let source_path = project.path().join("src").join("lib.rs");
        std::fs::create_dir_all(source_path.parent().expect("source parent"))
            .expect("create source parent");
        std::fs::write(&source_path, "pub fn indexed() {}\n").expect("write source");
        let indexed_mtime = live_mtime_millis(&source_path);
        let project_id = project_id_for_root(project.path());
        let hash = "ba5eba11cafebeef";
        {
            let mut storage = Store::open(&storage_path).expect("open db");
            storage
                .insert_file(&FileInfo {
                    id: 1,
                    path: source_path.clone(),
                    language: "rust".into(),
                    modification_time: indexed_mtime,
                    indexed: true,
                    complete: true,
                    line_count: 1,
                    file_role: FileRole::Source,
                })
                .expect("insert indexed file");
            let mut manifest = retrieval_manifest_fixture(&project_id, hash);
            manifest.built_at_epoch_ms = indexed_mtime;
            storage
                .upsert_retrieval_index_manifest(&manifest)
                .expect("manifest");
        }

        std::fs::write(
            project.path().join("src").join("new_module.rs"),
            "pub fn newly_added() {}\n",
        )
        .expect("write new source");

        let report =
            strict_sidecar_status(project.path(), Some(&storage_path)).expect("strict status");

        assert_eq!(report.retrieval_mode, "unavailable");
        assert!(
            report
                .degraded_reason
                .as_deref()
                .unwrap_or_default()
                .contains("indexable_file_added_or_changed_after_sidecar_manifest"),
            "new indexable file should make strict status fail closed: {report:?}"
        );
    }

    #[test]
    fn strict_status_rejects_manifest_when_new_parser_backed_language_file_is_added() {
        let _lock = crate::test_support::env_lock();
        let _backend = EnvGuard::set("CODESTORY_EMBED_BACKEND", "llamacpp");
        let project = TempDir::new().expect("project");
        let storage_dir = TempDir::new().expect("storage");
        let storage_path = storage_dir.path().join("codestory.db");
        let source_path = project.path().join("src").join("lib.rs");
        std::fs::create_dir_all(source_path.parent().expect("source parent"))
            .expect("create source parent");
        std::fs::write(&source_path, "pub fn indexed() {}\n").expect("write source");
        let indexed_mtime = live_mtime_millis(&source_path);
        let project_id = project_id_for_root(project.path());
        let hash = "ba5eba11feedface";
        {
            let mut storage = Store::open(&storage_path).expect("open db");
            storage
                .insert_file(&FileInfo {
                    id: 1,
                    path: source_path.clone(),
                    language: "rust".into(),
                    modification_time: indexed_mtime,
                    indexed: true,
                    complete: true,
                    line_count: 1,
                    file_role: FileRole::Source,
                })
                .expect("insert indexed file");
            let mut manifest = retrieval_manifest_fixture(&project_id, hash);
            manifest.built_at_epoch_ms = indexed_mtime;
            storage
                .upsert_retrieval_index_manifest(&manifest)
                .expect("manifest");
        }

        std::fs::write(
            project.path().join("src").join("Routes.kt"),
            "fun routeUsers() = Unit\n",
        )
        .expect("write kotlin source");

        let report =
            strict_sidecar_status(project.path(), Some(&storage_path)).expect("strict status");

        assert_eq!(report.retrieval_mode, "unavailable");
        assert!(
            report
                .degraded_reason
                .as_deref()
                .unwrap_or_default()
                .contains("indexable_file_added_or_changed_after_sidecar_manifest"),
            "new registry-backed parser file should make strict status fail closed: {report:?}"
        );
    }

    #[test]
    fn strict_readiness_accepts_markdown_covered_by_sidecar_fingerprint() {
        let _lock = crate::test_support::env_lock();
        let _backend = EnvGuard::set("CODESTORY_EMBED_BACKEND", "llamacpp");
        let project = TempDir::new().expect("project");
        let storage_dir = TempDir::new().expect("storage");
        let storage_path = storage_dir.path().join("codestory.db");
        let source_path = project.path().join("src").join("lib.rs");
        std::fs::create_dir_all(source_path.parent().expect("source parent"))
            .expect("create source parent");
        std::fs::write(&source_path, "pub fn indexed() {}\n").expect("write source");
        std::fs::write(project.path().join("AGENTS.md"), "# Agent guidance\n")
            .expect("write markdown");
        let indexed_mtime = live_mtime_millis(&source_path);
        let project_id = project_id_for_root(project.path());

        let mut storage = Store::open(&storage_path).expect("open db");
        storage
            .insert_file(&FileInfo {
                id: 1,
                path: source_path.clone(),
                language: "rust".into(),
                modification_time: indexed_mtime,
                indexed: true,
                complete: true,
                line_count: 1,
                file_role: FileRole::Source,
            })
            .expect("insert indexed file");
        let input = compute_sidecar_input_fingerprint(
            &storage,
            &storage_path,
            project.path(),
            &project_id,
            crate::embeddings::PRODUCT_EMBEDDING_RUNTIME_ID,
            crate::embeddings::RETRIEVAL_EMBEDDING_DIM as i32,
        )
        .expect("sidecar input");
        storage
            .upsert_retrieval_index_manifest(&codestory_store::RetrievalIndexManifest {
                project_id: project_id.clone(),
                zoekt_version: "zoekt-real-v1".into(),
                qdrant_collection: sidecar_qdrant_collection(&project_id, &input.hash),
                scip_revision: Some("graph-test".into()),
                built_at_epoch_ms: indexed_mtime,
                disk_bytes: None,
                degraded_modes_json: "[]".into(),
                embedding_backend: Some(crate::embeddings::PRODUCT_EMBEDDING_RUNTIME_ID.into()),
                embedding_dim: Some(768),
                sidecar_schema_version: Some(SIDECAR_SCHEMA_VERSION),
                sidecar_input_hash: Some(input.hash.clone()),
                sidecar_generation: Some(sidecar_generation_id(&project_id, &input.hash)),
                projection_count: Some(input.projection_count),
                symbol_doc_count: Some(input.symbol_doc_count),
                dense_projection_count: Some(input.dense_projection_count),
                semantic_policy_version: input.semantic_policy_version.clone(),
                graph_artifact_hash: Some(input.graph_artifact_hash.clone()),
                dense_reason_counts_json: Some(input.dense_reason_counts_json.clone()),
                precise_semantic_import_status: None,
                precise_semantic_import_reason: None,
                precise_semantic_import_revision: None,
                precise_semantic_import_producer: None,
            })
            .expect("manifest");

        validate_strict_sidecar_readiness(project.path(), &storage_path, &storage)
            .expect("markdown already covered by sidecar input should not look stale");

        std::fs::write(project.path().join("README.md"), "# New docs\n").expect("write new docs");
        let stale = validate_strict_sidecar_readiness(project.path(), &storage_path, &storage)
            .expect_err("new sidecar-only docs should stale the manifest");
        assert!(
            stale.to_string().contains("sidecar_input_hash_changed"),
            "docs-only sidecar drift should report input-hash drift, got: {stale:?}"
        );
    }

    fn live_mtime_millis(path: &Path) -> i64 {
        std::fs::metadata(path)
            .expect("metadata")
            .modified()
            .expect("modified")
            .duration_since(std::time::UNIX_EPOCH)
            .expect("mtime since epoch")
            .as_millis()
            .min(i64::MAX as u128) as i64
    }

    struct EnvGuard {
        key: &'static str,
        previous: Option<OsString>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let previous = std::env::var_os(key);
            // SAFETY: tests that mutate process environment hold crate::test_support::env_lock().
            unsafe {
                std::env::set_var(key, value);
            }
            Self { key, previous }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            // SAFETY: tests that mutate process environment hold crate::test_support::env_lock().
            unsafe {
                if let Some(previous) = self.previous.take() {
                    std::env::set_var(self.key, previous);
                } else {
                    std::env::remove_var(self.key);
                }
            }
        }
    }
}
