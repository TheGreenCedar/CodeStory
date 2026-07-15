use crate::config::{SidecarLayout, SidecarRuntimeConfig, dir_size_bytes};
use crate::embedded_vector::{EmbeddedVectorIndex, SemanticPoint};
use crate::generation::{
    SIDECAR_SCHEMA_VERSION, manifest_has_current_sidecar_contract,
    manifest_unavailable_reason_for_runtime, sidecar_generation_id,
};
use crate::health::probe_sidecar_health_for_runtime;
use crate::lexical_index::{
    LEXICAL_INDEX_VERSION, LexicalInputFingerprint, build_lexical_shard,
    finish_lexical_input_for_store, lexical_source_input,
};
use crate::retention::{
    FsGenerationRemover, GLOBAL_GENERATION_GC_LOCK_SCOPE, GenerationRetentionApplyReport,
    GenerationRetentionLock, GenerationRetentionMarker, GenerationRetentionPlan,
    VerifiedRollbackManifest, apply_generation_retention, global_generation_gc_state_file,
    plan_generation_retention, read_retention_marker, scan_retention_protection,
    write_retention_marker,
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
use tracing::{info, warn};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FinalizeIndexOutcome {
    pub project_id: String,
    pub manifest: RetrievalIndexManifest,
    pub degraded_modes: Vec<String>,
    pub scip_stubbed: bool,
    pub generation_retention_plan: GenerationRetentionPlan,
    pub generation_retention: GenerationRetentionApplyReport,
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

struct SidecarEmbeddingContract<'a> {
    backend: &'a str,
    dimension: i32,
}

struct SemanticGeneration<'a> {
    layout: &'a SidecarLayout,
    collection: &'a str,
    generation: &'a str,
    input_hash: &'a str,
    embedding_backend: &'a str,
    embedding_dim: i32,
    expected_points: i64,
}

#[derive(Debug, Clone, Copy)]
struct SidecarStubFlags {
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
    embedding_residency: crate::embeddings::ProductEmbeddingResidencyLease,
}

const SIDECAR_INPUT_BATCH_SIZE: usize = 4096;

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
    let embedding_residency =
        crate::embeddings::acquire_product_embedding_residency_for_runtime(runtime)
            .context("mandatory retrieval embedding runtime could not be pinned")?;
    let degraded_modes = Vec::new();
    let scip_stubbed = false;

    let embedding_backend = crate::embeddings::embedding_runtime_id_for_runtime(runtime);
    let embedding_dim = i32::try_from(crate::embeddings::semantic_vector_dim())
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
    let embedding_contract = SidecarEmbeddingContract {
        backend: &embedding_backend,
        dimension: embedding_dim,
    };
    let sidecar_input = compute_sidecar_input_fingerprint_with_lexical_source(
        input_snapshot.storage(),
        project_root,
        &project_id,
        &embedding_contract,
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
        embedding_residency,
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
            let previous_semantic = SemanticGeneration {
                layout: &layout,
                collection: &previous.semantic_generation,
                generation: previous.sidecar_generation.as_deref().unwrap_or_default(),
                input_hash: previous.sidecar_input_hash.as_deref().unwrap_or_default(),
                embedding_backend: &embedding_backend,
                embedding_dim,
                expected_points: sidecar_input.projection_count,
            };
            let semantic_point_count = semantic_ready_point_count(&previous_semantic);
            if status.retrieval_mode == "full" && semantic_point_count.is_some() {
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
                            SidecarStubFlags { scip_stubbed },
                        );
                    }
                }
                info!(
                    project_id = %project_id,
                    sidecar_generation = ?previous.sidecar_generation,
                    projection_count = sidecar_input.projection_count,
                    semantic_point_count = semantic_point_count.unwrap_or_default(),
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
                    SidecarStubFlags { scip_stubbed },
                );
            }
            warn!(
                project_id = %project_id,
                retrieval_mode = %status.retrieval_mode,
                degraded_reason = ?status.degraded_reason,
                semantic_point_count = ?semantic_point_count,
                "sidecar input unchanged but current generation is not healthy; rebuilding"
            );
        }
    }

    let generation = sidecar_generation_id(&project_id, &sidecar_input.hash);
    let collection = crate::generation::sidecar_vector_generation(&project_id, &sidecar_input.hash);
    let scip_dir = layout.scip_project_dir(&generation);
    let semantic_generation = SemanticGeneration {
        layout: &layout,
        collection: &collection,
        generation: &generation,
        input_hash: &sidecar_input.hash,
        embedding_backend: &embedding_backend,
        embedding_dim,
        expected_points: sidecar_input.projection_count,
    };
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
    let semantic_ready_points = if existing_status.semantic.capabilities.semantic {
        semantic_ready_point_count(&semantic_generation)
    } else {
        None
    };
    let scip_ready = existing_status.scip.capabilities.graph;

    if lexical_ready && semantic_ready_points.is_some() && scip_ready {
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
                semantic_point_count = semantic_ready_points.unwrap_or_default(),
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
                SidecarStubFlags { scip_stubbed },
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

    let _semantic_point_count = ensure_semantic_index(
        storage_path,
        project_root,
        &project_id,
        &semantic_generation,
        semantic_ready_points,
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
            SidecarStubFlags { scip_stubbed },
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

fn ensure_semantic_index(
    storage_path: &Path,
    project_root: &Path,
    project_id: &str,
    semantic: &SemanticGeneration<'_>,
    semantic_ready_points: Option<u64>,
    progress: &mut impl FnMut(&'static str),
) -> Result<u64> {
    if semantic.expected_points == 0 {
        info!(
            project_id = %project_id,
            collection = %semantic.collection,
            "dense vector index skipped because graph_first_v1 selected zero dense anchors"
        );
        return Ok(0);
    }
    if let Some(point_count) = semantic_ready_points {
        info!(
            project_id = %project_id,
            sidecar_generation = %semantic.generation,
            point_count,
            "embedded vector generation reused"
        );
        return Ok(point_count);
    }
    let point_count = with_finalize_progress(progress, "embedded vectors", || {
        EmbeddedVectorIndex::build_with_points(
            semantic.layout,
            semantic.collection,
            semantic.generation,
            semantic.input_hash,
            semantic.embedding_backend,
            usize::try_from(semantic.embedding_dim).context("negative embedding dimension")?,
            |visit| visit_semantic_points_from_store(storage_path, project_root, visit),
        )
    })?;
    if point_count != u64::try_from(semantic.expected_points).unwrap_or(u64::MAX) {
        bail!(
            "embedded vector generation incomplete for {project_id}: expected {} points, found {point_count}",
            semantic.expected_points
        );
    }
    info!(
        project_id = %project_id,
        sidecar_generation = %semantic.generation,
        point_count,
        "embedded SQLite vector generation published"
    );
    Ok(point_count)
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
        semantic_generation: collection.to_string(),
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
            &layout
                .semantic_data_dir
                .join("collections")
                .join(collection),
        ))
        .saturating_add(dir_size_bytes(scip_dir)) as i64,
    )
}

fn semantic_ready_point_count(semantic: &SemanticGeneration<'_>) -> Option<u64> {
    let expected = u64::try_from(semantic.expected_points).ok()?;
    if expected == 0 {
        return Some(0);
    }
    let health = EmbeddedVectorIndex::health(
        semantic.layout,
        semantic.collection,
        semantic.generation,
        semantic.input_hash,
        expected,
        semantic.embedding_backend,
        usize::try_from(semantic.embedding_dim).ok()?,
    );
    health.ready.then_some(health.point_count)
}

fn with_embedding_publication_residency<T>(
    residency: &crate::embeddings::ProductEmbeddingResidencyLease,
    publish: impl FnOnce() -> Result<T>,
) -> Result<T> {
    if residency.identity().is_none() && !cfg!(feature = "test-support") {
        bail!("embedding publication fence is missing its residency lease identity");
    }
    publish()
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
    let embedding_dim = i32::try_from(crate::embeddings::semantic_vector_dim())
        .unwrap_or(crate::embeddings::RETRIEVAL_EMBEDDING_DIM as i32);
    with_embedding_publication_residency(&retention_context.embedding_residency, || {
        promote_retrieval_manifest(
            &mut storage,
            sidecar_input,
            &manifest,
            |storage| {
                let lexical_source = lexical_source_input(project_root)
                    .context("rescan lexical source at publication fence")?;
                let embedding_contract = SidecarEmbeddingContract {
                    backend: &embedding_backend,
                    dimension: embedding_dim,
                };
                let current_input = compute_sidecar_input_fingerprint_with_lexical_source(
                    storage,
                    project_root,
                    &project_id,
                    &embedding_contract,
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
                    &project_id,
                    sidecar_input,
                    &manifest,
                    retention_context,
                )
            },
        )
    })?;

    let (generation_retention_plan, generation_retention) =
        retain_published_generations(storage_path, retention_context, &project_id, &manifest)?;

    info!(
        project_id = %project_id,
        lexical_version = %manifest.lexical_version,
        semantic_generation = %manifest.semantic_generation,
        sidecar_generation = ?manifest.sidecar_generation,
        degraded_modes = ?degraded_modes,
        "retrieval index manifest persisted"
    );

    Ok(FinalizeIndexOutcome {
        project_id,
        manifest,
        degraded_modes,
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
    semantic_points: Option<u64>,
    semantic_ready: bool,
    semantic_zero_dense_policy: bool,
    embedding_device: crate::embeddings::EmbeddingDeviceReadiness,
    embedding_accelerator_smoke_elapsed_ms: Option<u64>,
    embedding_identity_before: crate::in_process_embedding::ProcessEmbeddingIdentity,
    embedding_identity_after: crate::in_process_embedding::ProcessEmbeddingIdentity,
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
    let expected_embedding_dim = i32::try_from(crate::embeddings::semantic_vector_dim())
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
    let semantic_valid = if zero_dense_candidate {
        evidence.semantic_points.is_none()
            && evidence.semantic_ready
            && evidence.semantic_zero_dense_policy
    } else {
        evidence.semantic_points.is_some()
            && evidence.semantic_ready
            && !evidence.semantic_zero_dense_policy
    };
    if !semantic_valid {
        bail!("mandatory candidate generation component failed validation: semantic");
    }
    if !crate::embeddings::manifest_embedding_backend_is_product(
        manifest.embedding_backend.as_deref(),
    ) || !embedding_identity_matches(
        &evidence.embedding_identity_before,
        &evidence.embedding_identity_after,
    ) {
        bail!("mandatory candidate generation component failed validation: embedding_runtime");
    }
    let runtime_cpu_allowed = runtime.embedding.allow_cpu;
    let device_policy_valid = if evidence.embedding_device.cpu_allowed && runtime_cpu_allowed {
        evidence.embedding_device.full_retrieval_allowed
            && evidence.embedding_device.observed_state == "cpu_explicit"
            && evidence.embedding_device.observation_source == "inprocess_engine"
            && evidence.embedding_accelerator_smoke_elapsed_ms.is_none()
    } else if !evidence.embedding_device.cpu_allowed && !runtime_cpu_allowed {
        evidence.embedding_accelerator_smoke_elapsed_ms.is_some()
            && evidence.embedding_device.accelerator_requested
            && evidence.embedding_device.observed_state == "accelerated"
            && evidence.embedding_device.observation_source == "inprocess_engine"
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

fn embedding_identity_matches(
    before: &crate::in_process_embedding::ProcessEmbeddingIdentity,
    after: &crate::in_process_embedding::ProcessEmbeddingIdentity,
) -> bool {
    before.instance_id == after.instance_id
        && before.load_generation == after.load_generation
        && before.model_load_count == after.model_load_count
        && before.residency == "resident"
        && after.residency == "resident"
        && before.worker_alive
        && after.worker_alive
        && before.load_error.is_none()
        && after.load_error.is_none()
        && before.model_digest == after.model_digest
        && before.ggml_build_identity == after.ggml_build_identity
        && before.backend == after.backend
        && before.adapter_name == after.adapter_name
        && before.policy == after.policy
        && before.accelerator_execution_verified == after.accelerator_execution_verified
}

fn embedding_identity_matches_lease(
    lease: &crate::in_process_embedding::ProcessEmbeddingIdentity,
    before: &crate::in_process_embedding::ProcessEmbeddingIdentity,
    after: &crate::in_process_embedding::ProcessEmbeddingIdentity,
) -> bool {
    embedding_identity_matches(lease, before) && embedding_identity_matches(before, after)
}

fn validate_candidate_generation(
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
    let embedding_identity_before = crate::in_process_embedding::process_embedding_identity(
        &context.runtime.cache_root,
        context.runtime.embedding.allow_cpu,
    )
    .context("validate in-process embedding identity before final probes")?;
    let semantic_generation = SemanticGeneration {
        layout: context.layout,
        collection: &manifest.semantic_generation,
        generation,
        input_hash: &sidecar_input.hash,
        embedding_backend: manifest.embedding_backend.as_deref().unwrap_or_default(),
        embedding_dim: manifest.embedding_dim.unwrap_or_default(),
        expected_points: sidecar_input.projection_count,
    };
    let semantic_points = semantic_ready_point_count(&semantic_generation);
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
    let embedding_identity_after = crate::in_process_embedding::process_embedding_identity(
        &context.runtime.cache_root,
        context.runtime.embedding.allow_cpu,
    )
    .context("validate in-process embedding identity after final probes")?;
    if let Some(lease_identity) = context.embedding_residency.identity() {
        if !embedding_identity_matches_lease(
            lease_identity,
            &embedding_identity_before,
            &embedding_identity_after,
        ) {
            bail!("embedding engine load generation changed inside the publication fence");
        }
    } else if !cfg!(feature = "test-support") {
        bail!("embedding publication fence is missing its residency lease identity");
    }
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
        semantic_points: (sidecar_input.projection_count > 0)
            .then_some(semantic_points)
            .flatten(),
        semantic_ready: status.semantic.capabilities.semantic,
        semantic_zero_dense_policy: sidecar_input.projection_count == 0
            && sidecar_input.dense_projection_count == 0
            && status.semantic.status == crate::health::ComponentStatus::Healthy
            && status.semantic.degraded_reason.is_none()
            && status.semantic.capabilities.semantic,
        embedding_device,
        embedding_accelerator_smoke_elapsed_ms: embedding_accelerator_smoke
            .map(|smoke| smoke.elapsed_ms),
        embedding_identity_before,
        embedding_identity_after,
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
    let plan = plan_generation_retention(context.layout, project_id, &protection);
    let mut remover = FsGenerationRemover::new(context.layout)?;
    let apply = apply_generation_retention(&plan, &mut remover);
    Ok((plan, apply))
}

pub(crate) fn compute_sidecar_input_fingerprint(
    storage: &Store,
    project_root: &Path,
    project_id: &str,
    embedding_backend: &str,
    embedding_dim: i32,
) -> Result<SidecarInputFingerprint> {
    let lexical_source = lexical_source_input(project_root).context("hash lexical source input")?;
    let embedding_contract = SidecarEmbeddingContract {
        backend: embedding_backend,
        dimension: embedding_dim,
    };
    compute_sidecar_input_fingerprint_with_lexical_source(
        storage,
        project_root,
        project_id,
        &embedding_contract,
        lexical_source,
    )
}

fn compute_sidecar_input_fingerprint_with_lexical_source(
    storage: &Store,
    project_root: &Path,
    project_id: &str,
    embedding: &SidecarEmbeddingContract<'_>,
    lexical_source: crate::lexical_index::LexicalSourceInput,
) -> Result<SidecarInputFingerprint> {
    let lexical = finish_lexical_input_for_store(lexical_source, project_root, storage)
        .context("hash lexical symbol input")?;
    let mut hasher = Sha256::new();
    let mut graph_hasher = Sha256::new();
    hash_part(&mut hasher, "codestory-sidecar-input-v7");
    hash_part(&mut graph_hasher, "codestory-symbol-search-docs-v1");
    hash_part(&mut hasher, project_id);
    hash_part(&mut hasher, &SIDECAR_SCHEMA_VERSION.to_string());
    hash_part(&mut hasher, LEXICAL_INDEX_VERSION);
    hash_part(&mut hasher, &lexical.file_count.to_string());
    hash_part(&mut hasher, &lexical.hash);
    hash_part(&mut hasher, embedding.backend);
    hash_part(&mut hasher, &embedding.dimension.to_string());
    hash_part(&mut hasher, "semantic-vectors-v2");
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
            .filter(semantic_doc_row)
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
fn semantic_projection_row(row: &codestory_store::SearchSymbolProjectionDetail) -> bool {
    let Some(kind) = row
        .node_kind
        .and_then(|kind| i32::try_from(kind).ok())
        .and_then(|kind| codestory_contracts::graph::NodeKind::try_from(kind).ok())
    else {
        return false;
    };
    crate::generation::sidecar_semantic_node_kind(kind)
}

fn semantic_doc_row(doc: &LlmSymbolDoc) -> bool {
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

fn visit_semantic_points_from_store(
    storage_path: &Path,
    project_root: &Path,
    visit: &mut dyn FnMut(SemanticPoint) -> Result<()>,
) -> Result<()> {
    let mut storage = Store::open(storage_path).context("open storage for semantic vectors")?;
    ensure_search_symbol_projection(&mut storage)?;
    let file_roles = storage
        .get_files()
        .map(|files| sidecar_file_role_map(files, project_root))
        .unwrap_or_default();
    let mut after = None;
    loop {
        let batch = storage
            .get_llm_symbol_docs_batch_after(after, SIDECAR_INPUT_BATCH_SIZE)
            .context("load stored semantic docs for vector indexing")?;
        if batch.is_empty() {
            break;
        }
        after = batch.last().map(|doc| doc.node_id);
        for doc in batch.into_iter().filter(semantic_doc_row) {
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
            visit(SemanticPoint {
                display_name,
                node_id: doc.node_id.0.to_string(),
                file_path,
                file_role,
                dense_reason: doc.dense_reason.clone(),
                vector: doc.embedding,
            })?;
        }
    }
    Ok(())
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
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn test_embedding_identity(
        policy: &'static str,
    ) -> crate::in_process_embedding::ProcessEmbeddingIdentity {
        let accelerated = policy == "accelerated";
        crate::in_process_embedding::ProcessEmbeddingIdentity {
            instance_id: "inprocess:test".into(),
            load_generation: 1,
            model_load_count: 1,
            residency: "resident",
            worker_alive: true,
            load_error: None,
            model_digest: codestory_llama_sys::MODEL_SHA256,
            ggml_build_identity: codestory_llama_sys::GGML_BUILD_IDENTITY,
            backend: if accelerated { "Metal" } else { "CPU" }.into(),
            adapter_name: if accelerated {
                "test accelerator"
            } else {
                "CPU"
            }
            .into(),
            adapter_description: "test".into(),
            policy,
            embedded_model: true,
            materialized_path: PathBuf::from("model.gguf"),
            materialized_reused: true,
            initialization_ms: 1,
            smoke_ms: 1,
            adapter_memory_total: 1,
            adapter_memory_used_by_load: usize::from(accelerated),
            execution_device_names: if accelerated {
                vec!["test accelerator".into()]
            } else {
                Vec::new()
            },
            model_layer_count: 13,
            offloaded_layer_count: if accelerated { 13 } else { 0 },
            accelerator_execution_verified: accelerated,
        }
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
            message.contains("mandatory")
                || message.contains("embedding_device_unverified")
                || message.contains("without its embedded embedding model"),
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
    fn publication_requires_one_pinned_embedding_load_generation() {
        let before = test_embedding_identity("accelerated");
        let mut after = before.clone();
        assert!(embedding_identity_matches_lease(&before, &before, &after));

        after.load_generation += 1;
        after.model_load_count += 1;
        assert!(!embedding_identity_matches_lease(&before, &before, &after));

        after = before.clone();
        after.residency = "sleeping";
        assert!(!embedding_identity_matches_lease(&before, &before, &after));

        let mut lease = before.clone();
        lease.load_generation += 1;
        lease.model_load_count += 1;
        assert!(!embedding_identity_matches_lease(&lease, &before, &before));
    }

    #[test]
    fn publication_scope_retains_the_residency_guard_through_commit() {
        use std::sync::atomic::Ordering;

        let (residency, active) = crate::embeddings::ProductEmbeddingResidencyLease::test_lease(
            test_embedding_identity("accelerated"),
        );
        with_embedding_publication_residency(&residency, || {
            assert!(active.load(Ordering::Acquire));
            Ok(())
        })
        .expect("publication under residency guard");
        assert!(active.load(Ordering::Acquire));

        drop(residency);
        assert!(!active.load(Ordering::Acquire));
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
        let collection = crate::generation::sidecar_vector_generation(project_id, &input.hash);

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
        assert_eq!(manifest.semantic_generation, collection);
    }

    #[test]
    fn semantic_projection_excludes_low_value_local_symbols() {
        let row = |kind: NodeKind| SearchSymbolProjectionDetail {
            node_id: codestory_contracts::graph::NodeId(1),
            display_name: "symbol".into(),
            node_kind: Some(kind as i64),
            file_path: Some("src/lib.rs".into()),
            start_line: Some(1),
            end_line: Some(1),
        };

        assert!(semantic_projection_row(&row(NodeKind::FUNCTION)));
        assert!(semantic_projection_row(&row(NodeKind::ENUM_CONSTANT)));
        assert!(!semantic_projection_row(&row(NodeKind::VARIABLE)));
        assert!(!semantic_projection_row(&row(NodeKind::FIELD)));
        assert!(!semantic_projection_row(&row(NodeKind::UNKNOWN)));
    }

    #[test]
    fn semantic_doc_requires_product_stored_embedding() {
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
            embedding_profile: Some("coderank-embed".into()),
            embedding_model: crate::embeddings::PRODUCT_EMBEDDING_RUNTIME_ID.into(),
            embedding_backend: backend.map(str::to_string),
            embedding_dim: dim,
            doc_shape: Some("semantic_doc_version=4;scope=durable_symbols".into()),
            semantic_policy_version: Some(crate::generation::SEMANTIC_POLICY_VERSION.into()),
            dense_reason: Some("public_api".into()),
            embedding: vec![0.01; dim as usize],
            updated_at_epoch_ms: 123,
        };

        assert!(!semantic_doc_row(&doc(
            NodeKind::FUNCTION,
            Some("other"),
            768
        )));
        assert!(semantic_doc_row(&doc(
            NodeKind::METHOD,
            Some("inprocess"),
            768
        )));
        assert!(!semantic_doc_row(&doc(
            NodeKind::VARIABLE,
            Some("other"),
            768
        )));
        assert!(!semantic_doc_row(&doc(
            NodeKind::FUNCTION,
            Some("hash"),
            768
        )));
        assert!(!semantic_doc_row(&doc(
            NodeKind::FUNCTION,
            Some("other"),
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
            first_project.path(),
            &first_project_id,
            crate::embeddings::PRODUCT_EMBEDDING_RUNTIME_ID,
            crate::embeddings::RETRIEVAL_EMBEDDING_DIM as i32,
        )
        .expect("first input");
        let second_input = compute_sidecar_input_fingerprint(
            &second_storage,
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
        let runtime = SidecarRuntimeConfig::local();
        let embedding_contract = SidecarEmbeddingContract {
            backend: crate::embeddings::PRODUCT_EMBEDDING_RUNTIME_ID,
            dimension: crate::embeddings::RETRIEVAL_EMBEDDING_DIM as i32,
        };
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
            embedding_profile: Some("coderank-embed".into()),
            embedding_model: crate::embeddings::PRODUCT_EMBEDDING_RUNTIME_ID.into(),
            embedding_backend: Some("inprocess".into()),
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
            project.path(),
            "proj",
            crate::embeddings::PRODUCT_EMBEDDING_RUNTIME_ID,
            crate::embeddings::RETRIEVAL_EMBEDDING_DIM as i32,
        )
        .expect("first fingerprint");
        let old_manifest = retrieval_manifest_for_sidecar(
            "proj",
            &sidecar_generation_id("proj", &first.hash),
            &crate::generation::sidecar_vector_generation("proj", &first.hash),
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
        let second = compute_sidecar_input_fingerprint(
            &storage,
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

        let mut rejected_manifest = retrieval_manifest_for_sidecar(
            "proj",
            &sidecar_generation_id("proj", &first.hash),
            &crate::generation::sidecar_vector_generation("proj", &first.hash),
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
                    &embedding_contract,
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
            &crate::generation::sidecar_vector_generation("proj", &second.hash),
            crate::embeddings::PRODUCT_EMBEDDING_RUNTIME_ID,
            crate::embeddings::RETRIEVAL_EMBEDDING_DIM as i32,
            &second,
        );
        new_manifest.scip_revision = Some("graph-test".into());
        let passing = CandidateGenerationEvidence {
            lexical_matches: true,
            scip_revision: new_manifest.scip_revision.clone(),
            scip_graph: true,
            semantic_points: Some(second.projection_count as u64),
            semantic_ready: true,
            semantic_zero_dense_policy: false,
            embedding_device: crate::embeddings::EmbeddingDeviceReadiness {
                requested_policy: "accelerator_required",
                observed_state: "accelerated",
                observation_source: "inprocess_engine",
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
            embedding_identity_before: test_embedding_identity("accelerated"),
            embedding_identity_after: test_embedding_identity("accelerated"),
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
            &crate::generation::sidecar_vector_generation("proj", &zero_dense_input.hash),
            crate::embeddings::PRODUCT_EMBEDDING_RUNTIME_ID,
            crate::embeddings::RETRIEVAL_EMBEDDING_DIM as i32,
            &zero_dense_input,
        );
        zero_dense_manifest.scip_revision = Some("graph-test".into());
        let mut zero_dense_evidence = passing.clone();
        zero_dense_evidence.scip_revision = zero_dense_manifest.scip_revision.clone();
        zero_dense_evidence.semantic_points = None;
        zero_dense_evidence.semantic_zero_dense_policy = true;
        validate_candidate_generation_evidence(
            "proj",
            &zero_dense_input,
            &zero_dense_manifest,
            &runtime,
            &zero_dense_evidence,
        )
        .expect("zero-dense policy accepts an intentionally absent vector generation");

        let mut missing_nonzero_collection = passing.clone();
        missing_nonzero_collection.semantic_points = None;
        validate_candidate_generation_evidence(
            "proj",
            &second,
            &new_manifest,
            &runtime,
            &missing_nonzero_collection,
        )
        .expect_err("a missing nonzero vector generation must reject promotion");

        let mut cpu_runtime = runtime.clone();
        cpu_runtime.embedding.allow_cpu = true;
        let cpu_input = compute_sidecar_input_fingerprint(
            &storage,
            project.path(),
            "proj",
            crate::embeddings::PRODUCT_EMBEDDING_RUNTIME_ID,
            crate::embeddings::RETRIEVAL_EMBEDDING_DIM as i32,
        )
        .expect("cpu-policy fingerprint");
        let mut cpu_manifest = retrieval_manifest_for_sidecar(
            "proj",
            &sidecar_generation_id("proj", &cpu_input.hash),
            &crate::generation::sidecar_vector_generation("proj", &cpu_input.hash),
            crate::embeddings::PRODUCT_EMBEDDING_RUNTIME_ID,
            crate::embeddings::RETRIEVAL_EMBEDDING_DIM as i32,
            &cpu_input,
        );
        cpu_manifest.scip_revision = Some("graph-test".into());
        let mut cpu_evidence = passing.clone();
        cpu_evidence.scip_revision = cpu_manifest.scip_revision.clone();
        cpu_evidence.embedding_device = crate::embeddings::EmbeddingDeviceReadiness {
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
        };
        cpu_evidence.embedding_accelerator_smoke_elapsed_ms = None;
        cpu_evidence.embedding_identity_before = test_embedding_identity("cpu_explicit");
        cpu_evidence.embedding_identity_after = test_embedding_identity("cpu_explicit");
        validate_candidate_generation_evidence(
            "proj",
            &cpu_input,
            &cpu_manifest,
            &cpu_runtime,
            &cpu_evidence,
        )
        .expect("explicit CPU policy remains valid");
        let mut rollback_manifest = old_manifest.clone();
        let rollback_hash = "verified-rollback-input";
        rollback_manifest.sidecar_input_hash = Some(rollback_hash.into());
        rollback_manifest.sidecar_generation = Some(sidecar_generation_id("proj", rollback_hash));
        rollback_manifest.semantic_generation =
            crate::generation::sidecar_vector_generation("proj", rollback_hash);
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
            "semantic",
            "embedding_runtime",
            "accelerator_proof",
        ] {
            let mut failing = passing.clone();
            let candidate_input = second.clone();
            match component {
                "lexical" => failing.lexical_matches = false,
                "scip" => failing.scip_revision = Some("wrong-revision".into()),
                "semantic" => failing.semantic_points = None,
                "embedding_runtime" => {
                    failing.embedding_identity_after.instance_id = "inprocess:replaced".into();
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
                        &runtime,
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
                    &embedding_contract,
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
                    &embedding_contract,
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
                embedding_profile: Some("coderank-embed".into()),
                embedding_model: "legacy-producer".into(),
                embedding_backend: Some("legacy".into()),
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
