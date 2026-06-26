use crate::config::{
    SidecarLayout, SidecarRuntimeConfig, ZOEKT_REAL_VERSION_PIN, dir_size_bytes, qdrant_enabled,
    qdrant_semantic_vectors_enabled, zoekt_enabled,
};
use crate::generation::{
    SIDECAR_SCHEMA_VERSION, manifest_has_current_sidecar_contract, manifest_unavailable_reason,
    sidecar_generation_id,
};
use crate::health::probe_sidecar_health_with_embedding_device;
use crate::qdrant_client::{QDRANT_INDEX_UPSERT_BATCH_SIZE, QdrantClient, QdrantUpsertPoint};
use crate::scip_index::{
    SCIP_PRECISE_SEMANTIC_IMPORT_DIR, emit_scip_artifacts_from_store,
    import_precise_semantic_scip_artifact,
};
use crate::zoekt_client::ZoektClient;
use crate::zoekt_index::{build_zoekt_shard, lexical_input_fingerprint};
use anyhow::{Context, Result, bail};
use chrono::Utc;
use codestory_store::{FileRole, LlmSymbolDoc, RetrievalIndexManifest, Store, SymbolSearchDoc};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;
use std::time::Duration;
use tracing::{info, warn};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FinalizeIndexOutcome {
    pub project_id: String,
    pub manifest: RetrievalIndexManifest,
    pub degraded_modes: Vec<String>,
    pub zoekt_stubbed: bool,
    pub qdrant_stubbed: bool,
    pub scip_stubbed: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProjectQdrantRepairOutcome {
    pub project_id: String,
    pub qdrant_collection: String,
    pub collection_existed: bool,
    pub repaired: bool,
    pub points_upserted: usize,
    pub skipped_reason: Option<String>,
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
}

#[derive(Debug, Clone, Copy)]
struct SidecarStubFlags {
    zoekt_stubbed: bool,
    qdrant_stubbed: bool,
    scip_stubbed: bool,
}

const SIDECAR_INPUT_BATCH_SIZE: usize = QDRANT_INDEX_UPSERT_BATCH_SIZE * 8;
const QDRANT_SEMANTIC_SMOKE_RETRY_ATTEMPTS: usize = 2;
const QDRANT_SEMANTIC_SMOKE_RETRY_DELAY: Duration = Duration::from_millis(250);

pub fn project_id_for_root(project_root: &Path) -> String {
    let canonical = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf());
    fnv1a_hex(canonical.to_string_lossy().as_bytes())
}

pub fn sidecar_project_id_for_root(project_root: &Path) -> String {
    codestory_workspace::sidecar_project_identity(project_root, project_id_for_root(project_root))
        .project_id
}

pub fn repair_project_qdrant_collection(
    project_root: &Path,
    storage_path: &Path,
) -> Result<Option<ProjectQdrantRepairOutcome>> {
    if !storage_path.is_file() {
        return Ok(None);
    }

    let project_id = sidecar_project_id_for_root(project_root);
    let storage = Store::open(storage_path).context("open storage for qdrant project repair")?;
    let Some(manifest) = storage
        .get_retrieval_index_manifest(&project_id)
        .context("load retrieval manifest for qdrant project repair")?
    else {
        return Ok(None);
    };
    if let Some(reason) = manifest_unavailable_reason(&project_id, &storage, &manifest) {
        return Ok(Some(ProjectQdrantRepairOutcome {
            project_id,
            qdrant_collection: manifest.qdrant_collection,
            collection_existed: false,
            repaired: false,
            points_upserted: 0,
            skipped_reason: Some(reason),
        }));
    }
    drop(storage);

    if !qdrant_enabled() {
        return Ok(Some(ProjectQdrantRepairOutcome {
            project_id,
            qdrant_collection: manifest.qdrant_collection,
            collection_existed: false,
            repaired: false,
            points_upserted: 0,
            skipped_reason: Some("qdrant_disabled".into()),
        }));
    }

    let layout = SidecarLayout::from_env_for_project(project_root);
    layout.ensure_data_dirs()?;
    let qdrant_client = QdrantClient::new(&layout);
    let probe = qdrant_client.health_probe(&manifest.qdrant_collection);
    if !probe.reachable {
        return Ok(Some(ProjectQdrantRepairOutcome {
            project_id,
            qdrant_collection: manifest.qdrant_collection,
            collection_existed: false,
            repaired: false,
            points_upserted: 0,
            skipped_reason: Some(format!("qdrant_unreachable: {}", probe.detail)),
        }));
    }

    let collection_existed = probe.collection_exists;
    if collection_existed
        && qdrant_client.semantic_search_smoke(&manifest.qdrant_collection)
        && qdrant_ready_point_count(
            &qdrant_client,
            &manifest.qdrant_collection,
            manifest.projection_count.unwrap_or(0),
        )
        .is_some()
    {
        return Ok(Some(ProjectQdrantRepairOutcome {
            project_id,
            qdrant_collection: manifest.qdrant_collection,
            collection_existed,
            repaired: false,
            points_upserted: 0,
            skipped_reason: Some("collection_healthy".into()),
        }));
    }

    let points_upserted = upsert_qdrant_points_from_store(
        storage_path,
        project_root,
        &qdrant_client,
        &manifest.qdrant_collection,
    )
    .context("repair qdrant project collection from indexed symbol projection")?;
    if points_upserted == 0 {
        return Ok(Some(ProjectQdrantRepairOutcome {
            project_id,
            qdrant_collection: manifest.qdrant_collection,
            collection_existed,
            repaired: false,
            points_upserted: 0,
            skipped_reason: Some("no_indexed_symbol_points".into()),
        }));
    }

    Ok(Some(ProjectQdrantRepairOutcome {
        project_id,
        qdrant_collection: manifest.qdrant_collection,
        collection_existed,
        repaired: true,
        points_upserted,
        skipped_reason: None,
    }))
}

pub fn finalize_index(project_root: &Path, storage_path: &Path) -> Result<FinalizeIndexOutcome> {
    let runtime = crate::config::sidecar_runtime_auto(project_root);
    finalize_index_for_runtime(project_root, storage_path, &runtime)
}

pub fn finalize_index_for_runtime(
    project_root: &Path,
    storage_path: &Path,
    runtime: &SidecarRuntimeConfig,
) -> Result<FinalizeIndexOutcome> {
    runtime.activate_embed_url_default();
    let layout = runtime.layout.clone();
    layout.ensure_data_dirs()?;

    let project_id = sidecar_project_id_for_root(project_root);
    let degraded_modes = Vec::new();
    let zoekt_stubbed = false;
    let qdrant_stubbed = false;
    let scip_stubbed = false;

    let zoekt_client = ZoektClient::new(&layout);
    let zoekt_probe = zoekt_client.health_probe();
    if !zoekt_enabled() {
        bail!("Zoekt sidecar is mandatory; CODESTORY_ZOEKT_ENABLED=false is unsupported");
    }
    if !zoekt_probe.reachable {
        bail!(
            "Zoekt sidecar is mandatory and must be reachable at {} before indexing {project_id}: {}",
            layout.zoekt_base_url(),
            zoekt_probe.detail
        );
    }

    let qdrant_client = QdrantClient::new(&layout);
    let embedding_backend = crate::embeddings::embedding_runtime_id();
    let embedding_dim = i32::try_from(crate::embeddings::qdrant_vector_dim())
        .unwrap_or(crate::embeddings::RETRIEVAL_EMBEDDING_DIM as i32);
    if !qdrant_enabled() {
        bail!("Qdrant sidecar is mandatory; CODESTORY_QDRANT_ENABLED=false is unsupported");
    }
    crate::embeddings::ensure_product_embedding_backend_for_runtime(runtime)?;
    let embedding_device = crate::embeddings::embedding_device_readiness_for_runtime(runtime);

    let mut storage =
        Store::open(storage_path).context("open storage for retrieval sidecar input")?;
    ensure_search_symbol_projection(&mut storage)?;
    let sidecar_input = compute_sidecar_input_fingerprint(
        &storage,
        storage_path,
        project_root,
        &project_id,
        &embedding_backend,
        embedding_dim,
    )?;
    let previous_manifest = storage
        .get_retrieval_index_manifest(&project_id)
        .context("load previous retrieval_index_manifest")?;
    let previous_manifest_unavailable_reason = previous_manifest
        .as_ref()
        .and_then(|manifest| manifest_unavailable_reason(&project_id, &storage, manifest));
    drop(storage);

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
            let status = probe_sidecar_health_with_embedding_device(
                &layout,
                &project_id,
                Some(previous.clone()),
                &embedding_device,
            );
            let qdrant_point_count = qdrant_ready_point_count(
                &qdrant_client,
                &previous.qdrant_collection,
                sidecar_input.projection_count,
            );
            if status.retrieval_mode == "full" && qdrant_point_count.is_some() {
                let mut manifest = previous.clone();
                if let Some(generation) = manifest.sidecar_generation.as_deref() {
                    let scip_dir = layout.scip_project_dir(generation);
                    if update_precise_semantic_import_status(&scip_dir, &mut manifest)? {
                        return persist_finalized_manifest(
                            storage_path,
                            project_id,
                            manifest,
                            degraded_modes,
                            zoekt_stubbed,
                            qdrant_stubbed,
                            scip_stubbed,
                        );
                    }
                }
                info!(
                    project_id = %project_id,
                    sidecar_generation = ?previous.sidecar_generation,
                    projection_count = sidecar_input.projection_count,
                    qdrant_point_count = qdrant_point_count.unwrap_or_default(),
                    lexical_file_count = sidecar_input.lexical_file_count,
                    "retrieval sidecar generation unchanged; reused existing full sidecars"
                );
                return Ok(FinalizeIndexOutcome {
                    project_id,
                    manifest: previous.clone(),
                    degraded_modes,
                    zoekt_stubbed,
                    qdrant_stubbed,
                    scip_stubbed,
                });
            }
            warn!(
                project_id = %project_id,
                retrieval_mode = %status.retrieval_mode,
                degraded_reason = ?status.degraded_reason,
                qdrant_point_count = ?qdrant_point_count,
                "sidecar input unchanged but current generation is not healthy; rebuilding"
            );
        }
    }

    let generation = sidecar_generation_id(&project_id, &sidecar_input.hash);
    let collection = QdrantClient::collection_name_for_generation(&project_id, &sidecar_input.hash);
    let scip_dir = layout.scip_project_dir(&generation);
    let mut manifest = retrieval_manifest_for_sidecar(
        &project_id,
        &generation,
        &collection,
        &embedding_backend,
        embedding_dim,
        &sidecar_input,
    );
    manifest.scip_revision = read_scip_revision(&scip_dir);

    let existing_status = probe_sidecar_health_with_embedding_device(
        &layout,
        &project_id,
        Some(manifest.clone()),
        &embedding_device,
    );
    let zoekt_ready = existing_status.zoekt.capabilities.lexical
        && crate::zoekt_index::shard_matches_lexical_input(
            &layout.zoekt_data_dir,
            &generation,
            sidecar_input.lexical_file_count,
            &sidecar_input.lexical_hash,
        );
    let qdrant_ready_points = if existing_status.qdrant.capabilities.semantic {
        qdrant_ready_point_count(&qdrant_client, &collection, sidecar_input.projection_count)
    } else {
        None
    };
    let scip_ready = existing_status.scip.capabilities.graph;

    if zoekt_ready && qdrant_ready_points.is_some() && scip_ready {
        update_precise_semantic_import_status(&scip_dir, &mut manifest)?;
        manifest.disk_bytes = sidecar_disk_bytes(&layout, &scip_dir);
        let status = probe_sidecar_health_with_embedding_device(
            &layout,
            &project_id,
            Some(manifest.clone()),
            &embedding_device,
        );
        if status.retrieval_mode == "full" {
            info!(
                project_id = %project_id,
                sidecar_generation = %generation,
                qdrant_point_count = qdrant_ready_points.unwrap_or_default(),
                "current generated sidecars already healthy; persisted manifest without rebuild"
            );
            return persist_finalized_manifest(
                storage_path,
                project_id,
                manifest,
                degraded_modes,
                zoekt_stubbed,
                qdrant_stubbed,
                scip_stubbed,
            );
        }
    }

    let zoekt_version = ensure_zoekt_generation(
        project_root,
        storage_path,
        &layout,
        &project_id,
        &generation,
        zoekt_probe.reachable,
        zoekt_ready,
    )?;

    let _qdrant_point_count = ensure_qdrant_collection(
        storage_path,
        project_root,
        &qdrant_client,
        &project_id,
        &collection,
        sidecar_input.projection_count,
        qdrant_ready_points,
    )?;

    ensure_scip_artifacts(
        storage_path,
        &scip_dir,
        &project_id,
        &generation,
        scip_ready,
        &mut manifest,
    )?;
    update_precise_semantic_import_status(&scip_dir, &mut manifest)?;

    manifest.zoekt_version = zoekt_version;
    manifest.scip_revision = read_scip_revision(&scip_dir).or(manifest.scip_revision);
    manifest.disk_bytes = sidecar_disk_bytes(&layout, &scip_dir);

    finalize_manifest_after_health_check(
        storage_path,
        &layout,
        &qdrant_client,
        project_id,
        &collection,
        &sidecar_input,
        manifest,
        degraded_modes,
        SidecarStubFlags {
            zoekt_stubbed,
            qdrant_stubbed,
            scip_stubbed,
        },
        &embedding_device,
    )
}

fn ensure_zoekt_generation(
    project_root: &Path,
    storage_path: &Path,
    layout: &SidecarLayout,
    project_id: &str,
    generation: &str,
    zoekt_probe_reachable: bool,
    mut zoekt_ready: bool,
) -> Result<String> {
    if zoekt_ready {
        info!(project_id = %project_id, sidecar_generation = %generation, "Zoekt lexical shard reused");
    } else {
        match build_zoekt_shard(
            project_root,
            Some(storage_path),
            &layout.zoekt_data_dir,
            generation,
            zoekt_probe_reachable,
        ) {
            Ok(true) => {
                zoekt_ready = true;
                info!(project_id = %project_id, sidecar_generation = %generation, "Zoekt lexical shard built");
            }
            Ok(false) => {
                warn!(project_id = %project_id, "Zoekt shard build produced no files");
            }
            Err(error) => bail!("mandatory Zoekt shard build failed for {project_id}: {error}"),
        }
    }
    let shard_dir = crate::zoekt_index::shard_dir_for(&layout.zoekt_data_dir, generation);
    if !zoekt_ready || !crate::zoekt_index::shard_has_lexical_index(&shard_dir) {
        bail!("mandatory Zoekt lexical shard is missing for {project_id}");
    }
    Ok(ZOEKT_REAL_VERSION_PIN.to_string())
}

fn ensure_qdrant_collection(
    storage_path: &Path,
    project_root: &Path,
    qdrant_client: &QdrantClient,
    project_id: &str,
    collection: &str,
    projection_count: i64,
    mut qdrant_ready_points: Option<u64>,
) -> Result<u64> {
    if projection_count == 0 {
        info!(
            project_id = %project_id,
            collection = %collection,
            "Qdrant collection skipped because graph_first_v1 selected zero dense anchors"
        );
        return Ok(0);
    }
    let qdrant_probe = qdrant_client.health_probe(collection);
    if !qdrant_probe.reachable {
        bail!(
            "qdrant sidecar is mandatory but unreachable while finalizing retrieval index for {project_id}: {}",
            qdrant_probe.detail
        );
    }
    if let Some(point_count) = qdrant_ready_points {
        info!(
            project_id = %project_id,
            collection = %collection,
            qdrant_point_count = point_count,
            "Qdrant generated collection reused"
        );
    } else {
        let count =
            upsert_qdrant_points_from_store(storage_path, project_root, qdrant_client, collection)?;
        if count == 0 {
            bail!(
                "mandatory Qdrant semantic collection has no indexed symbol points for {project_id}"
            );
        }
        qdrant_ready_points = qdrant_ready_point_count(qdrant_client, collection, projection_count);
        if qdrant_ready_points.is_none() {
            let actual = qdrant_client
                .count_points_exact(collection)
                .unwrap_or_default();
            bail!(
                "mandatory Qdrant semantic collection incomplete for {project_id}: expected at least {} points, found {actual}",
                projection_count
            );
        }
        info!(
            project_id = %project_id,
            collection = %collection,
            points = count,
            qdrant_point_count = qdrant_ready_points.unwrap_or_default(),
            real_embeddings = qdrant_semantic_vectors_enabled(),
            "Qdrant collection ensured and populated"
        );
    }
    ensure_qdrant_semantic_smoke(project_id, collection, qdrant_client)?;
    Ok(qdrant_ready_points.unwrap_or_default())
}

fn ensure_qdrant_semantic_smoke(
    project_id: &str,
    collection: &str,
    qdrant_client: &QdrantClient,
) -> Result<()> {
    ensure_qdrant_semantic_smoke_with(
        project_id,
        collection,
        || qdrant_client.semantic_search_smoke_result(collection),
        QDRANT_SEMANTIC_SMOKE_RETRY_ATTEMPTS,
        QDRANT_SEMANTIC_SMOKE_RETRY_DELAY,
        std::thread::sleep,
    )
}

fn ensure_qdrant_semantic_smoke_with<F, S>(
    project_id: &str,
    collection: &str,
    mut smoke: F,
    retry_attempts: usize,
    retry_delay: Duration,
    mut sleep: S,
) -> Result<()>
where
    F: FnMut() -> Result<()>,
    S: FnMut(Duration),
{
    let total_attempts = retry_attempts + 1;
    for attempt in 1..=total_attempts {
        match smoke() {
            Ok(()) => return Ok(()),
            Err(error) if attempt < total_attempts => {
                warn!(
                    project_id = %project_id,
                    collection = %collection,
                    attempt,
                    total_attempts,
                    error = %error,
                    "Qdrant semantic smoke failed; retrying before finalize"
                );
                sleep(retry_delay);
            }
            Err(error) => {
                bail!(
                    "mandatory Qdrant semantic smoke failed for {project_id} collection {collection} after {attempt} attempt(s): {error:#}"
                );
            }
        }
    }
    Ok(())
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

#[allow(clippy::too_many_arguments)]
fn finalize_manifest_after_health_check(
    storage_path: &Path,
    layout: &SidecarLayout,
    qdrant_client: &QdrantClient,
    project_id: String,
    collection: &str,
    sidecar_input: &SidecarInputFingerprint,
    manifest: RetrievalIndexManifest,
    degraded_modes: Vec<String>,
    stub_flags: SidecarStubFlags,
    embedding_device: &crate::embeddings::EmbeddingDeviceReadiness,
) -> Result<FinalizeIndexOutcome> {
    let status = probe_sidecar_health_with_embedding_device(
        layout,
        &project_id,
        Some(manifest.clone()),
        embedding_device,
    );
    if status.retrieval_mode != "full" {
        bail!(
            "mandatory sidecar generation did not reach full mode for {project_id}: {} {:?}",
            status.retrieval_mode,
            status.degraded_reason
        );
    }
    if qdrant_ready_point_count(qdrant_client, collection, sidecar_input.projection_count).is_none()
    {
        bail!(
            "mandatory Qdrant semantic collection incomplete for {project_id}: expected at least {} generated points",
            sidecar_input.projection_count
        );
    }

    persist_finalized_manifest(
        storage_path,
        project_id,
        manifest,
        degraded_modes,
        stub_flags.zoekt_stubbed,
        stub_flags.qdrant_stubbed,
        stub_flags.scip_stubbed,
    )
}

#[allow(dead_code)] // Phase 2 cache keys
pub fn query_fingerprint(query: &str) -> String {
    let digest = Sha256::digest(query.as_bytes());
    format!("{:x}", digest)[..16].to_string()
}

fn ensure_search_symbol_projection(storage: &mut Store) -> Result<()> {
    if storage.get_search_symbol_projection_count().unwrap_or(0) == 0 {
        storage
            .rebuild_search_symbol_projection_from_node_table()
            .context("rebuild search_symbol_projection")?;
    }
    Ok(())
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
        zoekt_version: ZOEKT_REAL_VERSION_PIN.to_string(),
        qdrant_collection: collection.to_string(),
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

fn sidecar_disk_bytes(layout: &SidecarLayout, scip_dir: &Path) -> Option<i64> {
    Some(
        dir_size_bytes(&layout.zoekt_data_dir)
            .saturating_add(dir_size_bytes(&layout.qdrant_data_dir))
            .saturating_add(dir_size_bytes(scip_dir)) as i64,
    )
}

fn qdrant_ready_point_count(
    qdrant_client: &QdrantClient,
    collection: &str,
    expected_points: i64,
) -> Option<u64> {
    let expected_points = u64::try_from(expected_points).ok()?;
    if expected_points == 0 {
        return Some(0);
    }
    match qdrant_client.count_points_exact(collection) {
        Ok(actual) if actual >= expected_points => Some(actual),
        Ok(actual) => {
            warn!(
                collection = %collection,
                expected_points,
                actual_points = actual,
                "Qdrant generated collection is incomplete"
            );
            None
        }
        Err(error) => {
            warn!(
                collection = %collection,
                error = %error,
                "Qdrant generated collection point count unavailable"
            );
            None
        }
    }
}

fn persist_finalized_manifest(
    storage_path: &Path,
    project_id: String,
    mut manifest: RetrievalIndexManifest,
    degraded_modes: Vec<String>,
    zoekt_stubbed: bool,
    qdrant_stubbed: bool,
    scip_stubbed: bool,
) -> Result<FinalizeIndexOutcome> {
    manifest.built_at_epoch_ms = Utc::now().timestamp_millis();
    manifest.degraded_modes_json =
        serde_json::to_string(&degraded_modes).unwrap_or_else(|_| "[]".into());
    let mut storage = Store::open(storage_path).context("open storage for retrieval manifest")?;
    if let Some(reason) = manifest_unavailable_reason(&project_id, &storage, &manifest) {
        bail!(
            "mandatory retrieval sidecar manifest would be unavailable immediately for {project_id}: {reason}"
        );
    }
    storage
        .upsert_retrieval_index_manifest(&manifest)
        .context("persist retrieval_index_manifest")?;

    info!(
        project_id = %project_id,
        zoekt_version = %manifest.zoekt_version,
        qdrant_collection = %manifest.qdrant_collection,
        sidecar_generation = ?manifest.sidecar_generation,
        degraded_modes = ?degraded_modes,
        "retrieval index manifest persisted"
    );

    Ok(FinalizeIndexOutcome {
        project_id,
        manifest,
        degraded_modes,
        zoekt_stubbed,
        qdrant_stubbed,
        scip_stubbed,
    })
}

pub(crate) fn compute_sidecar_input_fingerprint(
    storage: &Store,
    storage_path: &Path,
    project_root: &Path,
    project_id: &str,
    embedding_backend: &str,
    embedding_dim: i32,
) -> Result<SidecarInputFingerprint> {
    let lexical = lexical_input_fingerprint(project_root, Some(storage_path))
        .context("hash lexical sidecar input")?;
    let mut hasher = Sha256::new();
    let mut graph_hasher = Sha256::new();
    hash_part(&mut hasher, "codestory-sidecar-input-v5");
    hash_part(&mut graph_hasher, "codestory-symbol-search-docs-v1");
    hash_part(&mut hasher, project_id);
    hash_part(&mut hasher, &SIDECAR_SCHEMA_VERSION.to_string());
    hash_part(&mut hasher, ZOEKT_REAL_VERSION_PIN);
    hash_part(&mut hasher, &lexical.file_count.to_string());
    hash_part(&mut hasher, &lexical.hash);
    hash_part(&mut hasher, embedding_backend);
    hash_part(&mut hasher, &embedding_dim.to_string());
    hash_part(
        &mut hasher,
        std::env::var("CODESTORY_EMBED_QUERY_PREFIX")
            .as_deref()
            .unwrap_or(""),
    );
    hash_part(
        &mut hasher,
        std::env::var("CODESTORY_EMBED_DOCUMENT_PREFIX")
            .as_deref()
            .unwrap_or(""),
    );
    hash_part(
        &mut hasher,
        if qdrant_semantic_vectors_enabled() {
            "qdrant-semantic-vectors"
        } else {
            "qdrant-hash-vectors"
        },
    );
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
            .get_llm_symbol_docs_batch_after(after, SIDECAR_INPUT_BATCH_SIZE)
            .context("load stored semantic docs for sidecar hash")?;
        if batch.is_empty() {
            break;
        }
        after = batch.last().map(|doc| doc.node_id);
        let batch = batch
            .into_iter()
            .filter(qdrant_semantic_doc_row)
            .collect::<Vec<_>>();
        dense_projection_count += i64::try_from(batch.len()).unwrap_or(i64::MAX);
        for doc in batch {
            observe_policy_version(&mut policy_versions, doc.semantic_policy_version.as_deref());
            let reason = doc.dense_reason.as_deref().unwrap_or("unknown").to_string();
            *dense_reason_counts.entry(reason).or_insert(0) += 1;
            hash_semantic_doc_detail(&mut hasher, project_root, &doc);
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

fn hash_semantic_doc_detail(hasher: &mut Sha256, project_root: &Path, doc: &LlmSymbolDoc) {
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
    hash_part(hasher, &doc.doc_version.to_string());
    hash_part(hasher, &doc.doc_hash);
    hash_part(hasher, doc.semantic_policy_version.as_deref().unwrap_or(""));
    hash_part(hasher, doc.dense_reason.as_deref().unwrap_or(""));
    hash_part(hasher, doc.embedding_profile.as_deref().unwrap_or(""));
    hash_part(hasher, &doc.embedding_model);
    hash_part(hasher, doc.embedding_backend.as_deref().unwrap_or(""));
    hash_part(hasher, &doc.embedding_dim.to_string());
    hash_part(hasher, doc.doc_shape.as_deref().unwrap_or(""));
    hash_part(hasher, &doc.embedding.len().to_string());
    hash_embedding_vector(hasher, &doc.embedding);
}

#[cfg(test)]
fn qdrant_semantic_projection_row(row: &codestory_store::SearchSymbolProjectionDetail) -> bool {
    let Some(kind) = row
        .node_kind
        .and_then(|kind| i32::try_from(kind).ok())
        .and_then(|kind| codestory_contracts::graph::NodeKind::try_from(kind).ok())
    else {
        return false;
    };
    crate::generation::sidecar_semantic_node_kind(kind)
}

fn qdrant_semantic_doc_row(doc: &LlmSymbolDoc) -> bool {
    crate::generation::sidecar_semantic_doc_is_product_eligible(doc)
}

fn hash_part(hasher: &mut Sha256, value: &str) {
    hasher.update(value.len().to_le_bytes());
    hasher.update(value.as_bytes());
}

fn hash_embedding_vector(hasher: &mut Sha256, embedding: &[f32]) {
    hasher.update(embedding.len().to_le_bytes());
    for value in embedding {
        hasher.update(value.to_bits().to_le_bytes());
    }
}

fn upsert_qdrant_points_from_store(
    storage_path: &Path,
    project_root: &Path,
    qdrant_client: &QdrantClient,
    collection: &str,
) -> Result<usize> {
    let mut storage = Store::open(storage_path).context("open storage for qdrant upsert")?;
    ensure_search_symbol_projection(&mut storage)?;
    let file_roles = storage
        .get_files()
        .map(|files| sidecar_file_role_map(files, project_root))
        .unwrap_or_default();
    let mut total = 0usize;
    let mut after = None;
    loop {
        let batch = storage
            .get_llm_symbol_docs_batch_after(after, SIDECAR_INPUT_BATCH_SIZE)
            .context("load stored semantic docs for qdrant")?;
        if batch.is_empty() {
            break;
        }
        after = batch.last().map(|doc| doc.node_id);
        let points = batch
            .into_iter()
            .filter(qdrant_semantic_doc_row)
            .map(|doc| {
                let id = qdrant_point_id_for_node_id(doc.node_id.0);
                let display_name = doc.qualified_name.clone().unwrap_or(doc.display_name);
                let file_path = doc
                    .file_path
                    .as_deref()
                    .and_then(|path| normalize_sidecar_file_path(path, project_root).ok());
                let file_role = file_path
                    .as_deref()
                    .and_then(|path| file_roles.get(path).copied())
                    .or_else(|| {
                        file_path
                            .as_deref()
                            .map(|path| FileRole::classify_path(Path::new(path)))
                    });
                QdrantUpsertPoint {
                    id,
                    display_name,
                    node_id: doc.node_id.0.to_string(),
                    file_path,
                    file_role,
                    dense_reason: doc.dense_reason.clone(),
                    vector: Some(doc.embedding),
                }
            })
            .collect::<Vec<_>>();
        if points.is_empty() {
            continue;
        }
        total += qdrant_client
            .upsert_points(collection, &points)
            .context("qdrant collection upsert")?;
    }
    Ok(total)
}

fn sidecar_file_role_map(
    files: Vec<codestory_store::FileInfo>,
    project_root: &Path,
) -> HashMap<String, FileRole> {
    files
        .into_iter()
        .filter_map(|file| {
            let path = file.path.to_string_lossy().to_string();
            normalize_sidecar_file_path(&path, project_root)
                .ok()
                .map(|path| (path, file.file_role))
        })
        .collect()
}

fn qdrant_point_id_for_node_id(node_id: i64) -> u64 {
    u64::from_le_bytes(node_id.to_le_bytes())
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

fn fnv1a_hex(bytes: &[u8]) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use codestory_contracts::graph::{Node, NodeId, NodeKind};
    use codestory_store::SearchSymbolProjectionDetail;
    use tempfile::TempDir;

    #[test]
    fn qdrant_point_ids_preserve_negative_node_ids() {
        assert_eq!(qdrant_point_id_for_node_id(42), 42);
        assert_ne!(
            qdrant_point_id_for_node_id(-1),
            qdrant_point_id_for_node_id(0)
        );
        assert_ne!(
            qdrant_point_id_for_node_id(-2),
            qdrant_point_id_for_node_id(-1)
        );
    }

    #[test]
    fn finalize_index_fails_without_mandatory_sidecar_artifacts() {
        let project = TempDir::new().expect("project dir");
        let storage_dir = TempDir::new().expect("storage dir");
        let storage_path = storage_dir.path().join("codestory.db");
        {
            let storage = Store::open(&storage_path).expect("open empty db");
            drop(storage);
        }
        let error = finalize_index(project.path(), &storage_path)
            .expect_err("empty stores cannot satisfy mandatory sidecar indexing");
        assert!(
            error.to_string().contains("mandatory"),
            "expected mandatory-sidecar error, got {error:#}"
        );
    }

    #[test]
    fn qdrant_semantic_smoke_gate_succeeds_first_attempt() {
        let mut calls = 0usize;
        ensure_qdrant_semantic_smoke_with(
            "proj",
            "collection",
            || {
                calls += 1;
                Ok(())
            },
            2,
            Duration::from_millis(1),
            |_| panic!("sleep should not run after first-pass success"),
        )
        .expect("smoke succeeds");

        assert_eq!(calls, 1);
    }

    #[test]
    fn qdrant_semantic_smoke_gate_retries_transient_failure() {
        let mut calls = 0usize;
        let mut sleeps = 0usize;
        ensure_qdrant_semantic_smoke_with(
            "proj",
            "collection",
            || {
                calls += 1;
                if calls == 1 {
                    anyhow::bail!("qdrant semantic smoke search failed: qdrant query warming");
                }
                Ok(())
            },
            2,
            Duration::from_millis(1),
            |_| sleeps += 1,
        )
        .expect("retry succeeds");

        assert_eq!(calls, 2);
        assert_eq!(sleeps, 1);
    }

    #[test]
    fn qdrant_semantic_smoke_gate_fails_closed_with_sublayer_detail() {
        let mut calls = 0usize;
        let error = ensure_qdrant_semantic_smoke_with(
            "proj",
            "collection",
            || {
                calls += 1;
                anyhow::bail!("qdrant semantic smoke search failed: embedding endpoint refused")
            },
            2,
            Duration::from_millis(1),
            |_| {},
        )
        .expect_err("hard smoke failure must fail finalize");

        let message = format!("{error:#}");
        assert_eq!(calls, 3);
        assert!(message.contains("mandatory Qdrant semantic smoke failed"));
        assert!(message.contains("embedding endpoint refused"));
    }

    #[test]
    fn project_qdrant_repair_noops_without_manifest() {
        let project = TempDir::new().expect("project dir");
        let storage_dir = TempDir::new().expect("storage dir");
        let storage_path = storage_dir.path().join("codestory.db");
        {
            let storage = Store::open(&storage_path).expect("open empty db");
            drop(storage);
        }

        let repair =
            repair_project_qdrant_collection(project.path(), &storage_path).expect("repair");
        assert!(repair.is_none());
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
        };
        let generation = sidecar_generation_id(project_id, &input.hash);
        let collection = QdrantClient::collection_name_for_generation(project_id, &input.hash);

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
        assert_eq!(manifest.qdrant_collection, collection);
    }

    #[test]
    fn qdrant_semantic_projection_excludes_low_value_local_symbols() {
        let row = |kind: NodeKind| SearchSymbolProjectionDetail {
            node_id: codestory_contracts::graph::NodeId(1),
            display_name: "symbol".into(),
            node_kind: Some(kind as i64),
            file_path: Some("src/lib.rs".into()),
            start_line: Some(1),
            end_line: Some(1),
        };

        assert!(qdrant_semantic_projection_row(&row(NodeKind::FUNCTION)));
        assert!(qdrant_semantic_projection_row(&row(
            NodeKind::ENUM_CONSTANT
        )));
        assert!(!qdrant_semantic_projection_row(&row(NodeKind::VARIABLE)));
        assert!(!qdrant_semantic_projection_row(&row(NodeKind::FIELD)));
        assert!(!qdrant_semantic_projection_row(&row(NodeKind::UNKNOWN)));
    }

    #[test]
    fn qdrant_semantic_doc_requires_product_stored_embedding() {
        let doc = |kind: NodeKind, backend: Option<&str>, dim: u32| LlmSymbolDoc {
            node_id: codestory_contracts::graph::NodeId(1),
            file_node_id: None,
            kind,
            display_name: "do_work".into(),
            qualified_name: Some("pkg::do_work".into()),
            file_path: Some("src/lib.rs".into()),
            start_line: Some(1),
            doc_text: "semantic doc".into(),
            doc_version: 4,
            doc_hash: "hash".into(),
            embedding_profile: Some("bge-base-en-v1.5".into()),
            embedding_model: "BAAI/bge-base-en-v1.5-local|backend=onnx".into(),
            embedding_backend: backend.map(str::to_string),
            embedding_dim: dim,
            doc_shape: Some("semantic_doc_version=4;scope=durable_symbols".into()),
            semantic_policy_version: Some(crate::generation::SEMANTIC_POLICY_VERSION.into()),
            dense_reason: Some("public_api".into()),
            embedding: vec![0.01; dim as usize],
            updated_at_epoch_ms: 123,
        };

        assert!(!qdrant_semantic_doc_row(&doc(
            NodeKind::FUNCTION,
            Some("onnx"),
            768
        )));
        assert!(qdrant_semantic_doc_row(&doc(
            NodeKind::METHOD,
            Some("llamacpp"),
            768
        )));
        assert!(!qdrant_semantic_doc_row(&doc(
            NodeKind::VARIABLE,
            Some("onnx"),
            768
        )));
        assert!(!qdrant_semantic_doc_row(&doc(
            NodeKind::FUNCTION,
            Some("hash"),
            768
        )));
        assert!(!qdrant_semantic_doc_row(&doc(
            NodeKind::FUNCTION,
            Some("onnx"),
            384
        )));
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
            &first_storage_path,
            first_project.path(),
            &first_project_id,
            crate::embeddings::PRODUCT_EMBEDDING_RUNTIME_ID,
            crate::embeddings::RETRIEVAL_EMBEDDING_DIM as i32,
        )
        .expect("first input");
        let second_input = compute_sidecar_input_fingerprint(
            &second_storage,
            &second_storage_path,
            second_project.path(),
            &second_project_id,
            crate::embeddings::PRODUCT_EMBEDDING_RUNTIME_ID,
            crate::embeddings::RETRIEVAL_EMBEDDING_DIM as i32,
        )
        .expect("second input");

        assert_eq!(first_input.hash, second_input.hash);
        assert_eq!(
            sidecar_generation_id(&first_project_id, &first_input.hash),
            sidecar_generation_id(&second_project_id, &second_input.hash)
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
    fn sidecar_input_hash_changes_when_embedding_values_change() {
        let project = TempDir::new().expect("project dir");
        std::fs::write(project.path().join("lib.rs"), "pub fn do_work() {}\n")
            .expect("project file");
        let storage_dir = TempDir::new().expect("storage dir");
        let storage_path = storage_dir.path().join("codestory.db");
        let mut storage = Store::open(&storage_path).expect("open store");
        storage
            .insert_nodes_batch(&[Node {
                id: NodeId(1),
                kind: NodeKind::FUNCTION,
                serialized_name: "do_work".into(),
                ..Default::default()
            }])
            .expect("node");
        let mut doc = LlmSymbolDoc {
            node_id: NodeId(1),
            file_node_id: None,
            kind: NodeKind::FUNCTION,
            display_name: "do_work".into(),
            qualified_name: Some("pkg::do_work".into()),
            file_path: Some("lib.rs".into()),
            start_line: Some(1),
            doc_text: "semantic doc".into(),
            doc_version: 4,
            doc_hash: "hash".into(),
            embedding_profile: Some("bge-base-en-v1.5".into()),
            embedding_model: crate::embeddings::PRODUCT_EMBEDDING_RUNTIME_ID.into(),
            embedding_backend: Some("llamacpp".into()),
            embedding_dim: crate::embeddings::RETRIEVAL_EMBEDDING_DIM as u32,
            doc_shape: Some("semantic_doc_version=4;scope=durable_symbols".into()),
            semantic_policy_version: Some(crate::generation::SEMANTIC_POLICY_VERSION.into()),
            dense_reason: Some("public_api".into()),
            embedding: vec![0.01; crate::embeddings::RETRIEVAL_EMBEDDING_DIM],
            updated_at_epoch_ms: 123,
        };
        storage
            .upsert_llm_symbol_docs_batch(&[doc.clone()])
            .expect("first doc");
        let first = compute_sidecar_input_fingerprint(
            &storage,
            &storage_path,
            project.path(),
            "proj",
            crate::embeddings::PRODUCT_EMBEDDING_RUNTIME_ID,
            crate::embeddings::RETRIEVAL_EMBEDDING_DIM as i32,
        )
        .expect("first fingerprint");
        doc.embedding[0] = 0.02;
        storage
            .upsert_llm_symbol_docs_batch(&[doc])
            .expect("second doc");
        let second = compute_sidecar_input_fingerprint(
            &storage,
            &storage_path,
            project.path(),
            "proj",
            crate::embeddings::PRODUCT_EMBEDDING_RUNTIME_ID,
            crate::embeddings::RETRIEVAL_EMBEDDING_DIM as i32,
        )
        .expect("second fingerprint");

        assert_eq!(first.projection_count, 1);
        assert_eq!(second.projection_count, 1);
        assert_eq!(first.dense_projection_count, 1);
        assert_eq!(first.dense_reason_counts_json, "{\"public_api\":1}");
        assert_ne!(first.hash, second.hash);
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
                embedding_profile: Some("bge-base-en-v1.5".into()),
                embedding_model: "BAAI/bge-base-en-v1.5-local|backend=onnx".into(),
                embedding_backend: Some("onnx".into()),
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
