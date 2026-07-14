use crate::config::{SidecarLayout, SidecarRuntimeConfig, dir_size_bytes};
#[cfg(test)]
use crate::generation::manifest_unavailable_reason;
use crate::generation::{
    SIDECAR_SCHEMA_VERSION, manifest_has_current_sidecar_contract,
    manifest_unavailable_reason_for_runtime, sidecar_generation_id,
};
use crate::health::probe_sidecar_health_for_runtime;
use crate::lexical_index::{
    LEXICAL_INDEX_VERSION, LexicalInputFingerprint, build_lexical_shard,
    finish_lexical_input_for_store, lexical_source_input,
};
use crate::qdrant_client::{QDRANT_INDEX_UPSERT_BATCH_SIZE, QdrantClient, QdrantUpsertPoint};
use crate::retention::{
    FsQdrantGenerationRemover, GLOBAL_GENERATION_GC_LOCK_SCOPE, GenerationRetentionApplyReport,
    GenerationRetentionLock, GenerationRetentionMarker, GenerationRetentionPlan,
    VerifiedRollbackManifest, apply_generation_retention, global_generation_gc_state_file,
    plan_generation_retention_with_qdrant_collections, read_retention_marker,
    scan_retention_protection, write_retention_marker,
};
use crate::scip_index::{
    SCIP_PRECISE_SEMANTIC_IMPORT_DIR, emit_scip_artifacts_from_store,
    import_precise_semantic_scip_artifact,
};
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
    pub qdrant_stubbed: bool,
    pub scip_stubbed: bool,
    pub generation_retention_plan: GenerationRetentionPlan,
    pub generation_retention: GenerationRetentionApplyReport,
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
    pub(crate) lexical_coverage: crate::lexical_index::LexicalCoverage,
}

#[derive(Debug, Clone, Copy)]
struct SidecarStubFlags {
    qdrant_stubbed: bool,
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
}

const SIDECAR_INPUT_BATCH_SIZE: usize = QDRANT_INDEX_UPSERT_BATCH_SIZE * 8;
const QDRANT_SEMANTIC_SMOKE_RETRY_ATTEMPTS: usize = 2;
const QDRANT_SEMANTIC_SMOKE_RETRY_DELAY: Duration = Duration::from_millis(250);

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

pub fn repair_project_qdrant_collection(
    project_root: &Path,
    storage_path: &Path,
) -> Result<Option<ProjectQdrantRepairOutcome>> {
    let runtime = SidecarRuntimeConfig::for_project_auto(project_root);
    repair_project_qdrant_collection_for_runtime(project_root, storage_path, &runtime)
}

pub fn repair_project_qdrant_collection_for_runtime(
    project_root: &Path,
    storage_path: &Path,
    runtime: &SidecarRuntimeConfig,
) -> Result<Option<ProjectQdrantRepairOutcome>> {
    if !storage_path.is_file() {
        return Ok(None);
    }

    let project_id = sidecar_project_id_for_runtime(project_root, runtime)?;
    let storage = Store::open(storage_path).context("open storage for qdrant project repair")?;
    let Some(manifest) = storage
        .get_retrieval_index_manifest(&project_id)
        .context("load retrieval manifest for qdrant project repair")?
    else {
        return Ok(None);
    };
    if let Some(reason) =
        manifest_unavailable_reason_for_runtime(&project_id, &storage, &manifest, runtime)
    {
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

    let layout = &runtime.layout;
    layout.ensure_data_dirs()?;
    let qdrant_client = QdrantClient::for_runtime(runtime)?;
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

pub fn finalize_index_for_runtime_with_progress(
    project_root: &Path,
    storage_path: &Path,
    runtime: &SidecarRuntimeConfig,
    mut progress: impl FnMut(&'static str),
) -> Result<FinalizeIndexOutcome> {
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
    let degraded_modes = Vec::new();
    let qdrant_stubbed = false;
    let scip_stubbed = false;

    let qdrant_client = QdrantClient::for_runtime(runtime)?;
    let embedding_backend = crate::embeddings::embedding_runtime_id_for_runtime(runtime);
    let embedding_dim = i32::try_from(crate::embeddings::qdrant_vector_dim())
        .unwrap_or(crate::embeddings::RETRIEVAL_EMBEDDING_DIM as i32);
    crate::embeddings::ensure_product_embedding_backend_for_runtime(runtime)?;
    let embedding_device = crate::embeddings::embedding_device_readiness_for_runtime(runtime);

    let mut storage =
        Store::open(storage_path).context("open storage for retrieval sidecar input")?;
    ensure_search_symbol_projection(&mut storage)?;
    let lexical_source = lexical_source_input(project_root).context("hash lexical source input")?;
    let input_snapshot = storage
        .read_snapshot()
        .context("open coherent sidecar input snapshot")?;
    let sidecar_input = compute_sidecar_input_fingerprint_with_lexical_source(
        input_snapshot.storage(),
        project_root,
        &project_id,
        &embedding_backend,
        embedding_dim,
        &runtime.embedding,
        lexical_source,
    )?;
    input_snapshot
        .finish()
        .context("finish coherent sidecar input snapshot")?;
    let previous_manifest = storage
        .get_retrieval_index_manifest(&project_id)
        .context("load previous retrieval_index_manifest")?;
    let previous_manifest_unavailable_reason = previous_manifest.as_ref().and_then(|manifest| {
        manifest_unavailable_reason_for_runtime(&project_id, &storage, manifest, runtime)
    });
    drop(storage);
    let retention_context = GenerationRetentionContext {
        runtime,
        layout: &layout,
        workspace_id: &workspace_id,
        previous_manifest: previous_manifest.as_ref(),
        embedding_device: &embedding_device,
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
                            project_root,
                            storage_path,
                            &retention_context,
                            &sidecar_input,
                            project_id,
                            manifest,
                            degraded_modes,
                            SidecarStubFlags {
                                qdrant_stubbed,
                                scip_stubbed,
                            },
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
                return persist_finalized_manifest(
                    project_root,
                    storage_path,
                    &retention_context,
                    &sidecar_input,
                    project_id,
                    previous.clone(),
                    degraded_modes,
                    SidecarStubFlags {
                        qdrant_stubbed,
                        scip_stubbed,
                    },
                );
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
    let qdrant_ready_points = if existing_status.qdrant.capabilities.semantic {
        qdrant_ready_point_count(&qdrant_client, &collection, sidecar_input.projection_count)
    } else {
        None
    };
    let scip_ready = existing_status.scip.capabilities.graph;

    if lexical_ready && qdrant_ready_points.is_some() && scip_ready {
        update_precise_semantic_import_status(&scip_dir, &mut manifest)?;
        manifest.disk_bytes = sidecar_disk_bytes(&layout, &generation, &collection, &scip_dir);
        let status = probe_sidecar_health_for_runtime(
            &layout,
            &project_id,
            Some(manifest.clone()),
            &embedding_device,
            runtime,
        );
        if status.retrieval_mode == "full" {
            info!(
                project_id = %project_id,
                sidecar_generation = %generation,
                qdrant_point_count = qdrant_ready_points.unwrap_or_default(),
                "current generated sidecars already healthy; persisted manifest without rebuild"
            );
            return persist_finalized_manifest(
                project_root,
                storage_path,
                &retention_context,
                &sidecar_input,
                project_id,
                manifest,
                degraded_modes,
                SidecarStubFlags {
                    qdrant_stubbed,
                    scip_stubbed,
                },
            );
        }
    }

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

    let _qdrant_point_count = ensure_qdrant_collection(
        storage_path,
        project_root,
        &qdrant_client,
        &project_id,
        &collection,
        sidecar_input.projection_count,
        qdrant_ready_points,
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
            &sidecar_input,
            project_id,
            manifest,
            degraded_modes,
            SidecarStubFlags {
                qdrant_stubbed,
                scip_stubbed,
            },
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

#[allow(clippy::too_many_arguments)]
fn ensure_qdrant_collection(
    storage_path: &Path,
    project_root: &Path,
    qdrant_client: &QdrantClient,
    project_id: &str,
    collection: &str,
    projection_count: i64,
    mut qdrant_ready_points: Option<u64>,
    progress: &mut impl FnMut(&'static str),
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
        let count = with_finalize_progress(progress, "embeddings", || {
            upsert_qdrant_points_from_store(storage_path, project_root, qdrant_client, collection)
        })?;
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
            real_embeddings = true,
            "Qdrant collection ensured and populated"
        );
    }
    with_finalize_progress(progress, "Qdrant finalize", || {
        ensure_qdrant_semantic_smoke(project_id, collection, qdrant_client)
    })?;
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
        lexical_version: LEXICAL_INDEX_VERSION.to_string(),
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
            &layout.qdrant_data_dir.join("collections").join(collection),
        ))
        .saturating_add(dir_size_bytes(scip_dir)) as i64,
    )
}

fn qdrant_ready_point_count(
    qdrant_client: &QdrantClient,
    collection: &str,
    expected_points: i64,
) -> Option<u64> {
    qdrant_ready_point_count_with(collection, expected_points, || {
        qdrant_client.count_points_exact(collection)
    })
}

fn qdrant_candidate_point_count(
    qdrant_client: &QdrantClient,
    collection: &str,
    expected_points: i64,
) -> Option<u64> {
    qdrant_candidate_point_count_with(expected_points, || {
        qdrant_ready_point_count(qdrant_client, collection, expected_points)
    })
}

fn qdrant_candidate_point_count_with(
    expected_points: i64,
    count_points: impl FnOnce() -> Option<u64>,
) -> Option<u64> {
    (expected_points > 0).then(count_points).flatten()
}

fn qdrant_ready_point_count_with(
    collection: &str,
    expected_points: i64,
    count_points: impl FnOnce() -> Result<u64>,
) -> Option<u64> {
    let expected_points = u64::try_from(expected_points).ok()?;
    match count_points() {
        Ok(actual) if actual == expected_points => Some(actual),
        Ok(actual) => {
            warn!(
                collection = %collection,
                expected_points,
                actual_points = actual,
                "Qdrant generated collection point count does not match its candidate input"
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

#[allow(clippy::too_many_arguments)]
fn persist_finalized_manifest(
    project_root: &Path,
    storage_path: &Path,
    retention_context: &GenerationRetentionContext<'_>,
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
    let embedding_dim = i32::try_from(crate::embeddings::qdrant_vector_dim())
        .unwrap_or(crate::embeddings::RETRIEVAL_EMBEDDING_DIM as i32);
    promote_retrieval_manifest(
        &mut storage,
        sidecar_input,
        &manifest,
        |storage| {
            let lexical_source = lexical_source_input(project_root)
                .context("rescan lexical source at publication fence")?;
            let current_input = compute_sidecar_input_fingerprint_with_lexical_source(
                storage,
                project_root,
                &project_id,
                &embedding_backend,
                embedding_dim,
                &retention_context.runtime.embedding,
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
        || {
            validate_candidate_generation(
                project_root,
                &project_id,
                sidecar_input,
                &manifest,
                retention_context,
            )
        },
    )?;

    let (generation_retention_plan, generation_retention) =
        retain_published_generations(storage_path, retention_context, &project_id, &manifest)?;

    info!(
        project_id = %project_id,
        lexical_version = %manifest.lexical_version,
        qdrant_collection = %manifest.qdrant_collection,
        sidecar_generation = ?manifest.sidecar_generation,
        degraded_modes = ?degraded_modes,
        "retrieval index manifest persisted"
    );

    Ok(FinalizeIndexOutcome {
        project_id,
        manifest,
        degraded_modes,
        qdrant_stubbed: stub_flags.qdrant_stubbed,
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
        bail!("sidecar generation input changed before manifest publication");
    }
    Ok(())
}

fn promote_retrieval_manifest(
    storage: &mut Store,
    expected: &SidecarInputFingerprint,
    manifest: &RetrievalIndexManifest,
    current_input: impl FnOnce(&Store) -> Result<SidecarInputFingerprint>,
    validate_candidate: impl FnOnce() -> Result<()>,
) -> Result<()> {
    validate_candidate()?;
    let mut publication = storage
        .write_transaction()
        .context("lock sidecar input and manifest publication")?;
    let current = current_input(publication.storage())?;
    ensure_sidecar_input_unchanged(expected, &current)?;
    publication
        .storage_mut()
        .upsert_retrieval_index_manifest(manifest)
        .context("persist retrieval_index_manifest")?;
    publication
        .finish()
        .context("commit retrieval manifest publication")
}

#[derive(Clone)]
struct CandidateGenerationEvidence {
    lexical_matches: bool,
    scip_revision: Option<String>,
    scip_graph: bool,
    qdrant_points: Option<u64>,
    qdrant_semantic: bool,
    qdrant_zero_dense_policy: bool,
    embedding_device: crate::embeddings::EmbeddingDeviceReadiness,
    embedding_accelerator_smoke_elapsed_ms: Option<u64>,
    embedding_launch_before: Option<crate::health::EmbeddingLaunchMetadata>,
    embedding_launch_after: Option<crate::health::EmbeddingLaunchMetadata>,
    expected_embedding_launch: Option<crate::health::EmbeddingLaunchMetadata>,
    embedding_container_identity_required: bool,
    embedding_container_identity_before: Option<String>,
    embedding_container_identity_after: Option<String>,
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
    let expected_embedding_dim = i32::try_from(crate::embeddings::qdrant_vector_dim())
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
    let qdrant_valid = if zero_dense_candidate {
        evidence.qdrant_points.is_none()
            && evidence.qdrant_semantic
            && evidence.qdrant_zero_dense_policy
    } else {
        evidence.qdrant_points.is_some()
            && evidence.qdrant_semantic
            && !evidence.qdrant_zero_dense_policy
    };
    if !qdrant_valid {
        bail!("mandatory candidate generation component failed validation: qdrant");
    }
    let container_identity_valid = if evidence.embedding_container_identity_required {
        matches!(
            (
                evidence.embedding_container_identity_before.as_deref(),
                evidence.embedding_container_identity_after.as_deref(),
            ),
            (Some(before), Some(after)) if before == after
        )
    } else {
        evidence.embedding_container_identity_before.is_none()
            && evidence.embedding_container_identity_after.is_none()
    };
    let native_launch_stable = match (
        evidence.embedding_launch_before.as_ref(),
        evidence.embedding_launch_after.as_ref(),
    ) {
        (Some(before), Some(after)) => before == after,
        (None, None) => true,
        _ => false,
    };
    if !crate::embeddings::manifest_embedding_backend_is_product(
        manifest.embedding_backend.as_deref(),
    ) || !embedding_launch_matches_runtime(
        runtime,
        evidence.embedding_launch_after.as_ref(),
        evidence.expected_embedding_launch.as_ref(),
    ) || !native_launch_stable
        || !container_identity_valid
    {
        bail!("mandatory candidate generation component failed validation: embedding_runtime");
    }
    let runtime_cpu_allowed = runtime
        .embedding
        .device_policy
        .eq_ignore_ascii_case("allow_cpu");
    let device_policy_valid = if evidence.embedding_device.cpu_allowed && runtime_cpu_allowed {
        evidence.embedding_device.full_retrieval_allowed
            && evidence.embedding_device.observation_source == "cpu_policy"
            && evidence.embedding_accelerator_smoke_elapsed_ms.is_none()
    } else if !evidence.embedding_device.cpu_allowed && !runtime_cpu_allowed {
        evidence.embedding_accelerator_smoke_elapsed_ms.is_some()
            && evidence.embedding_device.accelerator_requested
            && evidence.embedding_device.observed_state == "accelerated"
            && matches!(
                evidence.embedding_device.observation_source,
                "sidecar_log" | "native_log"
            )
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

fn embedding_launch_matches_runtime(
    runtime: &SidecarRuntimeConfig,
    observed: Option<&crate::health::EmbeddingLaunchMetadata>,
    expected: Option<&crate::health::EmbeddingLaunchMetadata>,
) -> bool {
    let native = runtime
        .embedding
        .server_launch
        .as_deref()
        .is_some_and(|mode| {
            matches!(
                mode.trim().to_ascii_lowercase().as_str(),
                "native" | "native_spawned"
            )
        });
    if !native {
        return true;
    }
    let (Some(observed), Some(expected)) = (observed, expected) else {
        return false;
    };
    let Some(model_path) = observed.model_path.as_deref() else {
        return false;
    };
    let profile = normalize_embedding_model_token(&runtime.embedding.profile);
    let model_file = Path::new(model_path)
        .file_name()
        .and_then(|name| name.to_str())
        .map(normalize_embedding_model_token)
        .unwrap_or_default();
    !profile.is_empty()
        && model_file.contains(&profile)
        && observed.launch_mode == expected.launch_mode
        && observed.endpoint == expected.endpoint
        && observed.launch_fingerprint_sha256 == expected.launch_fingerprint_sha256
        && observed.executable_path == expected.executable_path
        && observed.model_path == expected.model_path
        && observed.requested_device == expected.requested_device
}

fn normalize_embedding_model_token(value: &str) -> String {
    value
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn validate_candidate_generation(
    project_root: &Path,
    project_id: &str,
    sidecar_input: &SidecarInputFingerprint,
    manifest: &RetrievalIndexManifest,
    context: &GenerationRetentionContext<'_>,
) -> Result<()> {
    let generation = manifest
        .sidecar_generation
        .as_deref()
        .context("mandatory sidecar manifest is missing its generation")?;
    let scip_dir = context.layout.scip_project_dir(generation);
    let embedding_server_launch_mode =
        crate::config::embedding_server_launch_mode_for_runtime(context.runtime)?;
    let embedding_container_identity_required = embedding_server_launch_mode
        == crate::config::EmbeddingServerLaunchMode::DockerComposeEmbed;
    let embedding_launch_before =
        crate::sidecar::live_native_embedding_launch_metadata_for_runtime(context.runtime)
            .context("validate native embedding identity before final probes")?;
    let embedding_container_identity_before = embedding_container_identity_required
        .then(|| {
            crate::embeddings::ensure_persisted_running_embedding_container_identity(
                context.runtime,
            )
        })
        .transpose()
        .context("validate candidate embedding container identity before final probes")?;
    let qdrant_client = QdrantClient::for_runtime(context.runtime)?;
    let qdrant_points = qdrant_candidate_point_count(
        &qdrant_client,
        &manifest.qdrant_collection,
        sidecar_input.projection_count,
    );
    if sidecar_input.projection_count > 0 {
        ensure_qdrant_semantic_smoke(project_id, &manifest.qdrant_collection, &qdrant_client)?;
    }
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
    let embedding_container_identity_after = embedding_container_identity_required
        .then(|| {
            crate::embeddings::ensure_persisted_running_embedding_container_identity(
                context.runtime,
            )
        })
        .transpose()
        .context("validate candidate embedding container identity after final probes")?;
    let embedding_launch_after =
        crate::sidecar::live_native_embedding_launch_metadata_for_runtime(context.runtime)
            .context("validate native embedding identity after final probes")?;
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
        qdrant_points,
        qdrant_semantic: status.qdrant.capabilities.semantic,
        qdrant_zero_dense_policy: sidecar_input.projection_count == 0
            && sidecar_input.dense_projection_count == 0
            && status.qdrant.status == crate::health::ComponentStatus::Healthy
            && status.qdrant.degraded_reason.is_none()
            && status.qdrant.capabilities.semantic,
        embedding_device,
        embedding_accelerator_smoke_elapsed_ms: embedding_accelerator_smoke
            .map(|smoke| smoke.elapsed_ms),
        embedding_launch_before,
        embedding_launch_after,
        expected_embedding_launch: crate::compose::expected_native_embedding_launch_metadata(
            project_root,
            context.runtime,
        )?,
        embedding_container_identity_required,
        embedding_container_identity_before,
        embedding_container_identity_after,
        retrieval_mode: status.retrieval_mode,
        degraded_reason: status.degraded_reason,
    };
    validate_candidate_generation_evidence(
        project_id,
        sidecar_input,
        manifest,
        context.runtime,
        &evidence,
    )
}

fn retain_published_generations(
    storage_path: &Path,
    context: &GenerationRetentionContext<'_>,
    project_id: &str,
    active: &RetrievalIndexManifest,
) -> Result<(GenerationRetentionPlan, GenerationRetentionApplyReport)> {
    let now = Utc::now().timestamp_millis();
    let mut errors = Vec::new();
    let existing_marker =
        match read_retention_marker(&context.layout.state_file, context.workspace_id) {
            Ok(marker) => marker,
            Err(error) => {
                errors.push(format!("read generation retention marker: {error:#}"));
                None
            }
        };
    let active_generation = active.sidecar_generation.as_deref();
    let mut candidates = Vec::new();
    if let Some(previous) = context.previous_manifest {
        candidates.push(previous.clone());
    }
    if let Some(marker) = existing_marker.as_ref() {
        candidates.push(marker.active.clone());
        if let Some(rollback) = marker.rollback.as_ref() {
            candidates.push(rollback.manifest.clone());
        }
    }
    candidates.retain(|candidate| {
        candidate.project_id == project_id
            && candidate.sidecar_generation.as_deref() != active_generation
            && manifest_has_current_sidecar_contract(project_id, candidate)
    });
    candidates.sort_by_key(|candidate| candidate.built_at_epoch_ms);
    candidates.dedup_by(|left, right| left.sidecar_generation == right.sidecar_generation);
    let verified_previous = candidates.into_iter().rev().find_map(|candidate| {
        let status = probe_sidecar_health_for_runtime(
            context.layout,
            project_id,
            Some(candidate.clone()),
            context.embedding_device,
            context.runtime,
        );
        (status.retrieval_mode == "full").then_some(VerifiedRollbackManifest {
            manifest: candidate,
            verified_at_epoch_ms: now,
        })
    });

    let marker = GenerationRetentionMarker::next(
        context.workspace_id,
        active.clone(),
        verified_previous.clone(),
        now,
    );
    match marker {
        Ok(marker) => {
            if let Err(error) = write_retention_marker(&context.layout.state_file, &marker) {
                errors.push(format!("write generation retention marker: {error:#}"));
            }
        }
        Err(error) => errors.push(format!("build generation retention marker: {error:#}")),
    }

    let mut protection = scan_retention_protection(
        &crate::config::user_cache_root(),
        Some(storage_path),
        &context.layout.state_file,
    );
    if let Some(rollback) = verified_previous {
        protection
            .authoritative_rollback
            .push(rollback.manifest.clone());
        protection.rollback.push(rollback.manifest);
    }
    protection.errors.extend(errors);
    let live_qdrant_collections = match QdrantClient::new(context.layout).list_collection_names() {
        Ok(collections) => collections,
        Err(error) => {
            protection.errors.push(format!(
                "list live Qdrant collections for retention: {error:#}"
            ));
            Vec::new()
        }
    };
    let plan = plan_generation_retention_with_qdrant_collections(
        context.layout,
        project_id,
        &protection,
        &live_qdrant_collections,
    );
    let mut remover = FsQdrantGenerationRemover::new(context.layout)?;
    let apply = apply_generation_retention(&plan, &mut remover);
    Ok((plan, apply))
}

#[cfg(test)]
pub(crate) fn compute_sidecar_input_fingerprint(
    storage: &Store,
    storage_path: &Path,
    project_root: &Path,
    project_id: &str,
    embedding_backend: &str,
    embedding_dim: i32,
) -> Result<SidecarInputFingerprint> {
    compute_sidecar_input_fingerprint_for_runtime(
        storage,
        storage_path,
        project_root,
        project_id,
        embedding_backend,
        embedding_dim,
        &crate::config::SidecarRuntimeConfig::local().embedding,
    )
}

pub(crate) fn compute_sidecar_input_fingerprint_for_runtime(
    storage: &Store,
    _storage_path: &Path,
    project_root: &Path,
    project_id: &str,
    embedding_backend: &str,
    embedding_dim: i32,
    embedding: &crate::config::EmbeddingRuntimeConfig,
) -> Result<SidecarInputFingerprint> {
    let lexical_source = lexical_source_input(project_root).context("hash lexical source input")?;
    compute_sidecar_input_fingerprint_with_lexical_source(
        storage,
        project_root,
        project_id,
        embedding_backend,
        embedding_dim,
        embedding,
        lexical_source,
    )
}

fn compute_sidecar_input_fingerprint_with_lexical_source(
    storage: &Store,
    project_root: &Path,
    project_id: &str,
    embedding_backend: &str,
    embedding_dim: i32,
    embedding: &crate::config::EmbeddingRuntimeConfig,
    lexical_source: crate::lexical_index::LexicalSourceInput,
) -> Result<SidecarInputFingerprint> {
    let lexical = finish_lexical_input_for_store(lexical_source, project_root, storage)
        .context("hash lexical symbol input")?;
    let mut hasher = Sha256::new();
    let mut graph_hasher = Sha256::new();
    hash_part(&mut hasher, "codestory-sidecar-input-v6");
    hash_part(&mut graph_hasher, "codestory-symbol-search-docs-v1");
    hash_part(&mut hasher, project_id);
    hash_part(&mut hasher, &SIDECAR_SCHEMA_VERSION.to_string());
    hash_part(&mut hasher, LEXICAL_INDEX_VERSION);
    hash_part(&mut hasher, &lexical.file_count.to_string());
    hash_part(&mut hasher, &lexical.hash);
    hash_part(&mut hasher, embedding_backend);
    hash_part(&mut hasher, &embedding_dim.to_string());
    hash_part(&mut hasher, &embedding.profile);
    hash_part(&mut hasher, embedding.model_id.as_deref().unwrap_or(""));
    hash_part(&mut hasher, embedding.pooling.as_deref().unwrap_or(""));
    hash_part(
        &mut hasher,
        &embedding
            .layer_norm
            .map(|value| value.to_string())
            .unwrap_or_default(),
    );
    hash_part(
        &mut hasher,
        &embedding
            .truncate_dim
            .map(|value| value.to_string())
            .unwrap_or_default(),
    );
    hash_part(
        &mut hasher,
        &embedding
            .expected_dim
            .map(|value| value.to_string())
            .unwrap_or_default(),
    );
    hash_part(
        &mut hasher,
        embedding.server_launch.as_deref().unwrap_or(""),
    );
    hash_part(&mut hasher, embedding.query_prefix.as_deref().unwrap_or(""));
    hash_part(
        &mut hasher,
        embedding.document_prefix.as_deref().unwrap_or(""),
    );
    hash_part(&mut hasher, "qdrant-semantic-vectors");
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
            message.contains("mandatory") || message.contains("embedding_device_unverified"),
            "expected a pre-publication retrieval trust-gate error, got {error:#}"
        );
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
    fn qdrant_candidate_point_count_requires_an_exact_verified_collection() {
        let mut zero_count_calls = 0usize;
        assert_eq!(
            qdrant_candidate_point_count_with(0, || {
                zero_count_calls += 1;
                Some(0)
            }),
            None
        );
        assert_eq!(
            zero_count_calls, 0,
            "zero-dense candidates must not query an intentionally absent collection"
        );
        assert_eq!(
            qdrant_candidate_point_count_with(2, || {
                qdrant_ready_point_count_with("exact", 2, || Ok(2))
            }),
            Some(2)
        );
        assert_eq!(
            qdrant_candidate_point_count_with(2, || {
                qdrant_ready_point_count_with("missing", 2, || anyhow::bail!("collection missing"))
            }),
            None
        );
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
    fn project_qdrant_repair_uses_selected_runtime_layout() {
        let _env = crate::test_support::env_lock();
        let project = TempDir::new().expect("project dir");
        let storage_dir = TempDir::new().expect("storage dir");
        let sidecar_dir = TempDir::new().expect("sidecar dir");
        let storage_path = storage_dir.path().join("codestory.db");
        let project_id = sidecar_project_id_for_root(project.path());
        let input = SidecarInputFingerprint {
            hash: "0123456789abcdef0123456789abcdef".into(),
            symbol_doc_count: 0,
            projection_count: 0,
            dense_projection_count: 0,
            semantic_policy_version: Some(crate::generation::SEMANTIC_POLICY_VERSION.into()),
            graph_artifact_hash: "graph-hash".into(),
            dense_reason_counts_json: "{}".into(),
            lexical_file_count: 1,
            lexical_hash: "lexical".into(),
            lexical_coverage: Default::default(),
        };
        let collection = QdrantClient::collection_name_for_generation(&project_id, &input.hash);
        let manifest = retrieval_manifest_for_sidecar(
            &project_id,
            &sidecar_generation_id(&project_id, &input.hash),
            &collection,
            crate::embeddings::PRODUCT_EMBEDDING_RUNTIME_ID,
            768,
            &input,
        );
        {
            let mut storage = Store::open(&storage_path).expect("open db");
            storage
                .upsert_retrieval_index_manifest(&manifest)
                .expect("write manifest");
            assert_eq!(
                manifest_unavailable_reason(&project_id, &storage, &manifest),
                None
            );
        }

        let runtime = SidecarRuntimeConfig {
            project_identity: None,
            layout: SidecarLayout {
                qdrant_http_port: 9,
                qdrant_grpc_port: 10,
                lexical_data_dir: sidecar_dir.path().join("selected-lexical"),
                qdrant_data_dir: sidecar_dir.path().join("selected-qdrant"),
                scip_artifacts_root: sidecar_dir.path().join("selected-scip"),
                state_file: sidecar_dir.path().join("selected-state.json"),
            },
            profile: crate::config::SidecarProfile::Agent,
            run_id: Some("selected-run".into()),
            namespace: "selected-run".into(),
            compose_project: "selected-run".into(),
            embed_http_port: 11,
            cleanup_command: "codestory-cli retrieval down".into(),
            labels: BTreeMap::new(),
            ..SidecarRuntimeConfig::local()
        };

        let repair =
            repair_project_qdrant_collection_for_runtime(project.path(), &storage_path, &runtime)
                .expect("repair")
                .expect("manifest-backed repair");

        assert!(runtime.layout.lexical_data_dir.is_dir());
        assert!(runtime.layout.qdrant_data_dir.is_dir());
        assert!(runtime.layout.scip_artifacts_root.is_dir());
        assert_eq!(repair.qdrant_collection, collection);
        assert!(
            repair
                .skipped_reason
                .as_deref()
                .is_some_and(|reason| reason.starts_with("qdrant_unreachable:")),
            "{repair:?}"
        );
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
    fn manifest_promotion_rejects_same_count_content_drift_and_preserves_current() {
        let _env = crate::test_support::env_lock();
        let project = TempDir::new().expect("project dir");
        std::fs::write(project.path().join("lib.rs"), "pub fn do_work() {}\n")
            .expect("project file");
        let storage_dir = TempDir::new().expect("storage dir");
        let storage_path = storage_dir.path().join("codestory.db");
        let mut storage = Store::open(&storage_path).expect("open store");
        let mut runtime = SidecarRuntimeConfig::local();
        runtime.embedding.server_launch = None;
        runtime.embedding.device_policy = "accelerator_required".into();
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
        let first = compute_sidecar_input_fingerprint_for_runtime(
            &storage,
            &storage_path,
            project.path(),
            "proj",
            crate::embeddings::PRODUCT_EMBEDDING_RUNTIME_ID,
            crate::embeddings::RETRIEVAL_EMBEDDING_DIM as i32,
            &runtime.embedding,
        )
        .expect("first fingerprint");
        let old_manifest = retrieval_manifest_for_sidecar(
            "proj",
            &sidecar_generation_id("proj", &first.hash),
            &crate::generation::sidecar_qdrant_collection("proj", &first.hash),
            crate::embeddings::PRODUCT_EMBEDDING_RUNTIME_ID,
            crate::embeddings::RETRIEVAL_EMBEDDING_DIM as i32,
            &first,
        );
        storage
            .upsert_retrieval_index_manifest(&old_manifest)
            .expect("old manifest");
        doc.embedding[0] = 0.02;
        let mut concurrent = Store::open(&storage_path).expect("concurrent store");
        concurrent
            .upsert_llm_symbol_docs_batch(&[doc])
            .expect("second doc");
        drop(concurrent);
        let second = compute_sidecar_input_fingerprint_for_runtime(
            &storage,
            &storage_path,
            project.path(),
            "proj",
            crate::embeddings::PRODUCT_EMBEDDING_RUNTIME_ID,
            crate::embeddings::RETRIEVAL_EMBEDDING_DIM as i32,
            &runtime.embedding,
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
            &crate::generation::sidecar_qdrant_collection("proj", &first.hash),
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
                    crate::embeddings::PRODUCT_EMBEDDING_RUNTIME_ID,
                    crate::embeddings::RETRIEVAL_EMBEDDING_DIM as i32,
                    &runtime.embedding,
                    lexical_source,
                )
            },
            || Ok(()),
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
            &crate::generation::sidecar_qdrant_collection("proj", &second.hash),
            crate::embeddings::PRODUCT_EMBEDDING_RUNTIME_ID,
            crate::embeddings::RETRIEVAL_EMBEDDING_DIM as i32,
            &second,
        );
        new_manifest.scip_revision = Some("graph-test".into());
        let passing = CandidateGenerationEvidence {
            lexical_matches: true,
            scip_revision: new_manifest.scip_revision.clone(),
            scip_graph: true,
            qdrant_points: Some(second.projection_count as u64),
            qdrant_semantic: true,
            qdrant_zero_dense_policy: false,
            embedding_device: crate::embeddings::EmbeddingDeviceReadiness {
                requested_policy: "accelerator_required",
                observed_state: "accelerated",
                observation_source: "native_log",
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
            embedding_launch_before: None,
            embedding_launch_after: None,
            expected_embedding_launch: None,
            embedding_container_identity_required: false,
            embedding_container_identity_before: None,
            embedding_container_identity_after: None,
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
            &crate::generation::sidecar_qdrant_collection("proj", &zero_dense_input.hash),
            crate::embeddings::PRODUCT_EMBEDDING_RUNTIME_ID,
            crate::embeddings::RETRIEVAL_EMBEDDING_DIM as i32,
            &zero_dense_input,
        );
        zero_dense_manifest.scip_revision = Some("graph-test".into());
        let mut zero_dense_evidence = passing.clone();
        zero_dense_evidence.scip_revision = zero_dense_manifest.scip_revision.clone();
        zero_dense_evidence.qdrant_points = None;
        zero_dense_evidence.qdrant_zero_dense_policy = true;
        validate_candidate_generation_evidence(
            "proj",
            &zero_dense_input,
            &zero_dense_manifest,
            &runtime,
            &zero_dense_evidence,
        )
        .expect("zero-dense policy accepts an intentionally absent Qdrant collection");

        let mut missing_nonzero_collection = passing.clone();
        missing_nonzero_collection.qdrant_points = None;
        validate_candidate_generation_evidence(
            "proj",
            &second,
            &new_manifest,
            &runtime,
            &missing_nonzero_collection,
        )
        .expect_err("a missing nonzero Qdrant collection must reject promotion");

        let mut cpu_runtime = runtime.clone();
        cpu_runtime.embedding.device_policy = "allow_cpu".into();
        let cpu_input = compute_sidecar_input_fingerprint_for_runtime(
            &storage,
            &storage_path,
            project.path(),
            "proj",
            crate::embeddings::PRODUCT_EMBEDDING_RUNTIME_ID,
            crate::embeddings::RETRIEVAL_EMBEDDING_DIM as i32,
            &cpu_runtime.embedding,
        )
        .expect("cpu-policy fingerprint");
        let mut cpu_manifest = retrieval_manifest_for_sidecar(
            "proj",
            &sidecar_generation_id("proj", &cpu_input.hash),
            &crate::generation::sidecar_qdrant_collection("proj", &cpu_input.hash),
            crate::embeddings::PRODUCT_EMBEDDING_RUNTIME_ID,
            crate::embeddings::RETRIEVAL_EMBEDDING_DIM as i32,
            &cpu_input,
        );
        cpu_manifest.scip_revision = Some("graph-test".into());
        let mut cpu_evidence = passing.clone();
        cpu_evidence.scip_revision = cpu_manifest.scip_revision.clone();
        cpu_evidence.embedding_device = crate::embeddings::EmbeddingDeviceReadiness {
            requested_policy: "cpu_allowed",
            observed_state: "cpu",
            observation_source: "cpu_policy",
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
        validate_candidate_generation_evidence(
            "proj",
            &cpu_input,
            &cpu_manifest,
            &cpu_runtime,
            &cpu_evidence,
        )
        .expect("explicit CPU policy remains valid");
        let mut replaced_container = passing.clone();
        replaced_container.embedding_container_identity_required = true;
        replaced_container.embedding_container_identity_before =
            Some("container-a|start-a|true".into());
        replaced_container.embedding_container_identity_after =
            Some("container-b|start-b|true".into());
        let replaced_error = validate_candidate_generation_evidence(
            "proj",
            &second,
            &new_manifest,
            &runtime,
            &replaced_container,
        )
        .expect_err("container replacement during final probes must reject promotion");
        assert!(replaced_error.to_string().contains("embedding_runtime"));
        let mut stable_container = replaced_container;
        stable_container.embedding_container_identity_after =
            stable_container.embedding_container_identity_before.clone();
        validate_candidate_generation_evidence(
            "proj",
            &second,
            &new_manifest,
            &runtime,
            &stable_container,
        )
        .expect("one persisted running container identity remains valid across final probes");
        let mut native_runtime = runtime.clone();
        native_runtime.embedding.server_launch = Some("native_spawned".into());
        let native_input = compute_sidecar_input_fingerprint_for_runtime(
            &storage,
            &storage_path,
            project.path(),
            "proj",
            crate::embeddings::PRODUCT_EMBEDDING_RUNTIME_ID,
            crate::embeddings::RETRIEVAL_EMBEDDING_DIM as i32,
            &native_runtime.embedding,
        )
        .expect("native fingerprint");
        let mut native_manifest = retrieval_manifest_for_sidecar(
            "proj",
            &sidecar_generation_id("proj", &native_input.hash),
            &crate::generation::sidecar_qdrant_collection("proj", &native_input.hash),
            crate::embeddings::PRODUCT_EMBEDDING_RUNTIME_ID,
            crate::embeddings::RETRIEVAL_EMBEDDING_DIM as i32,
            &native_input,
        );
        native_manifest.scip_revision = Some("graph-test".into());
        let mut wrong_native_model = passing.clone();
        wrong_native_model.scip_revision = native_manifest.scip_revision.clone();
        let expected_native_launch = crate::health::EmbeddingLaunchMetadata {
            provider: "test".into(),
            launch_mode: "native_spawned".into(),
            endpoint: crate::config::SidecarLayout::embed_base_url(native_runtime.embed_http_port),
            pid: None,
            spawned_at_epoch_ms: None,
            process_start_identity: None,
            spawn_protocol: None,
            launch_args: Vec::new(),
            launch_fingerprint_sha256: Some("expected-fingerprint".into()),
            executable_source: None,
            executable_path: Some("llama-server".into()),
            model_path: Some("bge-base-en-v1.5.gguf".into()),
            model_sha256: None,
            log_path: Some("llama-server-native.log".into()),
            requested_device: None,
        };
        let mut stale_native_launch = expected_native_launch.clone();
        stale_native_launch.launch_fingerprint_sha256 = Some("stale-fingerprint".into());
        wrong_native_model.embedding_launch_before = Some(stale_native_launch.clone());
        wrong_native_model.embedding_launch_after = Some(stale_native_launch);
        wrong_native_model.expected_embedding_launch = Some(expected_native_launch.clone());
        let native_error = validate_candidate_generation_evidence(
            "proj",
            &native_input,
            &native_manifest,
            &native_runtime,
            &wrong_native_model,
        )
        .expect_err("wrong native model must fail");
        assert!(native_error.to_string().contains("embedding_runtime"));
        let mut replaced_native = passing.clone();
        replaced_native.scip_revision = native_manifest.scip_revision.clone();
        replaced_native.embedding_launch_before = Some(expected_native_launch.clone());
        let mut replacement_launch = expected_native_launch.clone();
        replacement_launch.pid = Some(4242);
        replacement_launch.spawned_at_epoch_ms = Some(456);
        replaced_native.embedding_launch_after = Some(replacement_launch);
        replaced_native.expected_embedding_launch = Some(expected_native_launch);
        let replacement_error = validate_candidate_generation_evidence(
            "proj",
            &native_input,
            &native_manifest,
            &native_runtime,
            &replaced_native,
        )
        .expect_err("native launch replacement across final probes must fail");
        assert!(replacement_error.to_string().contains("embedding_runtime"));
        let mut rollback_manifest = old_manifest.clone();
        let rollback_hash = "verified-rollback-input";
        rollback_manifest.sidecar_input_hash = Some(rollback_hash.into());
        rollback_manifest.sidecar_generation = Some(sidecar_generation_id("proj", rollback_hash));
        rollback_manifest.qdrant_collection =
            crate::generation::sidecar_qdrant_collection("proj", rollback_hash);
        rollback_manifest.built_at_epoch_ms -= 1;
        let state_file = storage_dir.path().join("state/retrieval-sidecars.json");
        let marker = GenerationRetentionMarker::next(
            "workspace",
            old_manifest.clone(),
            Some(VerifiedRollbackManifest {
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
            "qdrant",
            "embedding_runtime",
            "accelerator_proof",
        ] {
            let mut failing = passing.clone();
            let mut candidate_input = second.clone();
            let mut candidate_runtime = runtime.clone();
            match component {
                "lexical" => failing.lexical_matches = false,
                "scip" => failing.scip_revision = Some("wrong-revision".into()),
                "qdrant" => failing.qdrant_points = None,
                "embedding_runtime" => {
                    candidate_runtime.embedding.model_id = Some("different-768d-model".into());
                    candidate_input = compute_sidecar_input_fingerprint_for_runtime(
                        &storage,
                        &storage_path,
                        project.path(),
                        "proj",
                        crate::embeddings::PRODUCT_EMBEDDING_RUNTIME_ID,
                        crate::embeddings::RETRIEVAL_EMBEDDING_DIM as i32,
                        &candidate_runtime.embedding,
                    )
                    .expect("mismatched model fingerprint");
                    assert_ne!(candidate_input.hash, second.hash);
                }
                "accelerator_proof" => failing.embedding_accelerator_smoke_elapsed_ms = None,
                _ => unreachable!(),
            }
            let rejected = promote_retrieval_manifest(
                &mut storage,
                &candidate_input,
                &new_manifest,
                |_| Ok(candidate_input.clone()),
                || {
                    validate_candidate_generation_evidence(
                        "proj",
                        &candidate_input,
                        &new_manifest,
                        &candidate_runtime,
                        &failing,
                    )
                },
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
                || {
                    validate_candidate_generation_evidence(
                        "proj",
                        &second,
                        &new_manifest,
                        &runtime,
                        &passing,
                    )?;
                    panic!("simulated crash before candidate promotion")
                },
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
                    crate::embeddings::PRODUCT_EMBEDDING_RUNTIME_ID,
                    crate::embeddings::RETRIEVAL_EMBEDDING_DIM as i32,
                    &runtime.embedding,
                    lexical_source,
                )
            },
            || {
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
        )
        .expect_err("source drift during validation must reject publication");
        assert!(drifted.to_string().contains("input changed"));
        assert_eq!(
            storage
                .get_retrieval_index_manifest("proj")
                .expect("current manifest after source drift"),
            Some(old_manifest.clone())
        );
        std::fs::write(&source_path, "pub fn do_work() {}\n").expect("restore source");

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
                    crate::embeddings::PRODUCT_EMBEDDING_RUNTIME_ID,
                    crate::embeddings::RETRIEVAL_EMBEDDING_DIM as i32,
                    &runtime.embedding,
                    lexical_source,
                )
            },
            || {
                validate_candidate_generation_evidence(
                    "proj",
                    &second,
                    &new_manifest,
                    &runtime,
                    &passing,
                )
            },
        )
        .expect("unchanged input promotes manifest");
        assert_eq!(
            storage
                .get_retrieval_index_manifest("proj")
                .expect("current manifest"),
            Some(new_manifest)
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
