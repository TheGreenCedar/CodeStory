use crate::config::{
    AGENT_SIDECAR_NAMESPACE_PREFIX_V3, LOCAL_SIDECAR_NAMESPACE_V3, SIDECAR_STATE_FILE_V3,
    SidecarRuntimeConfig, current_v3_state_ownership_matches, user_cache_root,
};
use crate::generation::{
    manifest_has_current_sidecar_contract, manifest_unavailable_reason_for_runtime,
};
use crate::native_embedding::{EmbedModelInventory, embed_model_inventory};
use crate::retention::{
    FsGenerationRemover, GLOBAL_GENERATION_GC_LOCK_SCOPE, GenerationRetentionApplyReport,
    GenerationRetentionLock, GenerationRetentionPlan, GenerationRetentionState,
    apply_generation_retention, global_generation_gc_state_file,
    plan_generation_retention_with_unrooted_state, scan_retention_protection,
};
use crate::sidecar::SidecarStateFile;
use anyhow::{Context, Result};
use codestory_store::Store;
use codestory_workspace::owned_deletion::OwnedDeletionRoot;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const STALE_AFTER_MS: i64 = 24 * 60 * 60 * 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SidecarInventoryState {
    Live,
    Stale,
    Incomplete,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SidecarInventoryEntry {
    pub namespace: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    pub state_path: String,
    pub state_exists: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cleanup_command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub age_ms: Option<i64>,
    pub model: EmbedModelInventory,
    pub state: SidecarInventoryState,
    pub reasons: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub safe_candidate_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocking_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SidecarInventoryReport {
    pub dry_run: bool,
    pub cache_root: String,
    pub namespaces: Vec<SidecarInventoryEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generation_retention: Option<GenerationRetentionPlan>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SidecarGcNamespaceResult {
    pub namespace: String,
    pub state: SidecarInventoryState,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub removed_paths: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SidecarGcReport {
    pub dry_run: bool,
    pub cache_root: String,
    pub removed: Vec<SidecarGcNamespaceResult>,
    pub blocked: Vec<SidecarGcNamespaceResult>,
    pub namespaces: Vec<SidecarInventoryEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generation_retention: Option<GenerationRetentionApplyReport>,
}

#[derive(Debug, Clone)]
struct StateCandidate {
    namespace: String,
    state_path: PathBuf,
    state: Option<SidecarStateFile>,
    read_error: Option<String>,
}

pub fn sidecar_inventory_with_storage(
    project_root: &Path,
    storage_path: &Path,
) -> Result<SidecarInventoryReport> {
    let cache_root = user_cache_root();
    let mut report = sidecar_inventory_with_cache_root(&cache_root)?;
    report.generation_retention = Some(generation_retention_plan_for_storage(
        project_root,
        storage_path,
        &cache_root,
    )?);
    Ok(report)
}

fn sidecar_inventory_with_cache_root(cache_root: &Path) -> Result<SidecarInventoryReport> {
    Ok(build_inventory(
        cache_root,
        chrono::Utc::now().timestamp_millis(),
    ))
}

pub fn sidecar_gc_apply_with_storage(
    project_root: &Path,
    storage_path: &Path,
) -> Result<SidecarGcReport> {
    let cache_root = user_cache_root();
    let runtime = SidecarRuntimeConfig::for_project_auto(project_root);
    let global_gc_state_file = global_generation_gc_state_file(&runtime);
    let _global_gc_lock =
        GenerationRetentionLock::acquire(&global_gc_state_file, GLOBAL_GENERATION_GC_LOCK_SCOPE)
            .context("coordinate global sidecar cleanup with generation publication")?;
    let mut report = sidecar_gc_apply_with_cache_root(&cache_root)?;
    report.generation_retention = Some(apply_generation_retention_for_storage(
        project_root,
        storage_path,
        &cache_root,
    )?);
    Ok(report)
}

fn sidecar_gc_apply_with_cache_root(cache_root: &Path) -> Result<SidecarGcReport> {
    apply_inventory(cache_root, chrono::Utc::now().timestamp_millis())
}

fn build_inventory(cache_root: &Path, now_ms: i64) -> SidecarInventoryReport {
    let model = embed_model_inventory();
    let mut namespaces = discover_state_candidates(cache_root)
        .into_iter()
        .map(|candidate| inventory_entry(now_ms, candidate, model.clone()))
        .collect::<Vec<_>>();
    namespaces.sort_by(|left, right| left.namespace.cmp(&right.namespace));
    SidecarInventoryReport {
        dry_run: true,
        cache_root: cache_root.display().to_string(),
        namespaces,
        generation_retention: None,
    }
}

fn apply_inventory(cache_root: &Path, now_ms: i64) -> Result<SidecarGcReport> {
    let snapshot = build_inventory(cache_root, now_ms);
    let mut removed = Vec::new();
    let mut blocked = Vec::new();
    let deletion = OwnedDeletionRoot::open(cache_root)
        .with_context(|| format!("open CodeStory cache root {}", cache_root.display()))?;
    for entry in &snapshot.namespaces {
        if entry.safe_candidate_reason.is_none() {
            blocked.push(blocked_result(entry));
            continue;
        }
        let mut result = SidecarGcNamespaceResult {
            namespace: entry.namespace.clone(),
            state: entry.state,
            reason: "owned native retrieval state is stale; applying cleanup".into(),
            removed_paths: Vec::new(),
            errors: Vec::new(),
        };
        let candidate =
            read_state_candidate(entry.namespace.clone(), PathBuf::from(&entry.state_path));
        if let Some(state) = candidate.state.as_ref() {
            for path in [
                &state.lexical_data_dir,
                &state.semantic_data_dir,
                &state.scip_artifacts_root,
            ] {
                remove_owned_path(cache_root, Path::new(path), &deletion, &mut result);
            }
        }
        if result.errors.is_empty() {
            remove_owned_path(cache_root, &candidate.state_path, &deletion, &mut result);
        }
        if result.errors.is_empty() {
            removed.push(result);
        } else {
            result.reason = "cleanup partially failed".into();
            blocked.push(result);
        }
    }
    Ok(SidecarGcReport {
        dry_run: false,
        cache_root: cache_root.display().to_string(),
        removed,
        blocked,
        namespaces: snapshot.namespaces,
        generation_retention: None,
    })
}

fn remove_owned_path(
    cache_root: &Path,
    path: &Path,
    deletion: &OwnedDeletionRoot,
    result: &mut SidecarGcNamespaceResult,
) {
    let relative = match path.strip_prefix(cache_root) {
        Ok(relative) if !relative.as_os_str().is_empty() => relative,
        _ => {
            result.errors.push(format!(
                "refuse cleanup outside CodeStory cache root: {}",
                path.display()
            ));
            return;
        }
    };
    match deletion.remove(relative) {
        Ok(true) => result.removed_paths.push(path.display().to_string()),
        Ok(false) => {}
        Err(error) => result
            .errors
            .push(format!("remove owned path {}: {error}", path.display())),
    }
}

fn blocked_result(entry: &SidecarInventoryEntry) -> SidecarGcNamespaceResult {
    SidecarGcNamespaceResult {
        namespace: entry.namespace.clone(),
        state: entry.state,
        reason: entry
            .blocking_reason
            .clone()
            .unwrap_or_else(|| "not a safe cleanup candidate".into()),
        removed_paths: Vec::new(),
        errors: Vec::new(),
    }
}

fn generation_retention_plan_for_storage(
    project_root: &Path,
    storage_path: &Path,
    cache_root: &Path,
) -> Result<GenerationRetentionPlan> {
    let runtime = SidecarRuntimeConfig::for_project_auto(project_root);
    let layout = &runtime.layout;
    let project_id = crate::index::sidecar_project_id_for_runtime(project_root, &runtime)?;
    let (_lock, unrooted_state) = inventory_retention_view(layout, &project_id)?;
    Ok(build_generation_retention_plan(
        storage_path,
        cache_root,
        &runtime,
        &project_id,
        unrooted_state,
    ))
}

fn inventory_retention_view(
    layout: &crate::config::SidecarLayout,
    project_id: &str,
) -> Result<(Option<GenerationRetentionLock>, GenerationRetentionState)> {
    let lock = GenerationRetentionLock::try_acquire_shared(&layout.state_file, project_id)
        .context("observe retrieval generation inventory lock")?;
    let state = if lock.is_some() {
        GenerationRetentionState::Reclaimable
    } else {
        GenerationRetentionState::Building
    };
    Ok((lock, state))
}

fn apply_generation_retention_for_storage(
    project_root: &Path,
    storage_path: &Path,
    cache_root: &Path,
) -> Result<GenerationRetentionApplyReport> {
    let runtime = SidecarRuntimeConfig::for_project_auto(project_root);
    let project_id = crate::index::sidecar_project_id_for_runtime(project_root, &runtime)?;
    let _lock = GenerationRetentionLock::acquire(&runtime.layout.state_file, &project_id)
        .context("lock retrieval generation retention apply")?;
    let plan = build_generation_retention_plan(
        storage_path,
        cache_root,
        &runtime,
        &project_id,
        GenerationRetentionState::Reclaimable,
    );
    let mut remover = FsGenerationRemover::new(&runtime.layout)?;
    Ok(apply_generation_retention(&plan, &mut remover))
}

fn build_generation_retention_plan(
    storage_path: &Path,
    cache_root: &Path,
    runtime: &SidecarRuntimeConfig,
    project_id: &str,
    unrooted_state: GenerationRetentionState,
) -> GenerationRetentionPlan {
    let layout = &runtime.layout;
    let mut protection =
        scan_retention_protection(cache_root, Some(storage_path), &layout.state_file);
    let manifest = if storage_path.is_file() {
        match Store::open(storage_path) {
            Ok(store) => match store.get_retrieval_index_manifest(project_id) {
                Ok(Some(manifest)) => {
                    record_manifest_retention_freshness(
                        &store,
                        project_id,
                        &manifest,
                        runtime,
                        &mut protection.errors,
                    );
                    Some(manifest)
                }
                Ok(None) => None,
                Err(error) => {
                    protection
                        .errors
                        .push(format!("load active manifest for retention: {error:#}"));
                    None
                }
            },
            Err(error) => {
                protection
                    .errors
                    .push(format!("open active storage for retention: {error:#}"));
                None
            }
        }
    } else {
        None
    };
    match manifest {
        Some(manifest) if manifest_has_current_sidecar_contract(project_id, &manifest) => {
            let embedding_device =
                crate::embeddings::embedding_device_readiness_for_runtime(runtime);
            let health = crate::health::probe_sidecar_health_for_runtime(
                layout,
                project_id,
                Some(manifest),
                &embedding_device,
                runtime,
            );
            if health.retrieval_mode != "full" {
                protection.errors.push(format!(
                    "active generation is not verified full; pruning suppressed: mode={} reason={}",
                    health.retrieval_mode,
                    health.degraded_reason.as_deref().unwrap_or("unknown")
                ));
            }
        }
        Some(_) => protection.errors.push(
            "active retrieval manifest does not satisfy the current generation contract; pruning suppressed"
                .into(),
        ),
        None => protection
            .errors
            .push("active retrieval manifest is unavailable; pruning suppressed".into()),
    }
    plan_generation_retention_with_unrooted_state(layout, project_id, &protection, unrooted_state)
}

fn record_manifest_retention_freshness(
    store: &Store,
    project_id: &str,
    manifest: &codestory_store::RetrievalIndexManifest,
    runtime: &SidecarRuntimeConfig,
    errors: &mut Vec<String>,
) {
    if let Some(reason) =
        manifest_unavailable_reason_for_runtime(project_id, store, manifest, runtime)
    {
        errors.push(format!(
            "active retrieval manifest is stale; pruning suppressed: {reason}"
        ));
    }
}

fn discover_state_candidates(cache_root: &Path) -> Vec<StateCandidate> {
    let mut candidates = Vec::new();
    for (namespace, path) in [(
        LOCAL_SIDECAR_NAMESPACE_V3.to_string(),
        cache_root.join(SIDECAR_STATE_FILE_V3),
    )] {
        if path.exists() {
            candidates.push(read_state_candidate(namespace, path));
        }
    }
    let sidecars_root = cache_root.join("sidecars");
    if let Ok(entries) = std::fs::read_dir(&sidecars_root) {
        for entry in entries.flatten() {
            if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                continue;
            }
            let namespace = entry.file_name().to_string_lossy().to_string();
            if !namespace.starts_with(AGENT_SIDECAR_NAMESPACE_PREFIX_V3) {
                continue;
            }
            let state_path = entry.path().join(SIDECAR_STATE_FILE_V3);
            candidates.push(if state_path.exists() {
                read_state_candidate(namespace, state_path)
            } else {
                StateCandidate {
                    namespace,
                    state_path,
                    state: None,
                    read_error: Some("state file missing".into()),
                }
            });
        }
    }
    candidates
}

fn read_state_candidate(namespace: String, state_path: PathBuf) -> StateCandidate {
    let parsed = std::fs::read_to_string(&state_path)
        .with_context(|| format!("read {}", state_path.display()))
        .and_then(|contents| {
            serde_json::from_str::<SidecarStateFile>(&contents)
                .with_context(|| format!("parse {}", state_path.display()))
        });
    match parsed {
        Ok(state) => StateCandidate {
            namespace,
            state_path,
            state: Some(state),
            read_error: None,
        },
        Err(error) => StateCandidate {
            namespace,
            state_path,
            state: None,
            read_error: Some(error.to_string()),
        },
    }
}

fn inventory_entry(
    now_ms: i64,
    candidate: StateCandidate,
    model: EmbedModelInventory,
) -> SidecarInventoryEntry {
    let state = candidate.state.as_ref();
    let state_exists = state.is_some();
    let age_ms = state.map(|state| now_ms.saturating_sub(state.started_at_epoch_ms).max(0));
    let mut reasons = candidate.read_error.iter().cloned().collect::<Vec<_>>();
    if state.is_some_and(|state| state.owner != "codestory") {
        reasons.push("state owner is not codestory".into());
    }
    if state.is_some() && !model.required_gguf_present {
        reasons.push(format!("missing required GGUF {}", model.required_gguf));
    }
    if state.is_some_and(|state| {
        !Path::new(&state.lexical_data_dir).exists()
            || !Path::new(&state.semantic_data_dir).exists()
            || !Path::new(&state.scip_artifacts_root).exists()
    }) {
        reasons.push("one or more retrieval data directories are missing".into());
    }
    let current = state.is_some_and(|state| {
        serde_json::to_value(state)
            .is_ok_and(|value| current_v3_state_ownership_matches(&value, &candidate.namespace))
    });
    let live_process = state
        .and_then(|state| state.embedding_launch.as_ref())
        .is_some_and(|launch| {
            crate::sidecar::ensure_native_embedding_launch_identity(launch).is_ok()
        });
    let inventory_state = if !current {
        SidecarInventoryState::Unknown
    } else if !reasons.is_empty() {
        SidecarInventoryState::Incomplete
    } else if live_process || age_ms.is_some_and(|age| age < STALE_AFTER_MS) {
        SidecarInventoryState::Live
    } else {
        SidecarInventoryState::Stale
    };
    let (safe_candidate_reason, blocking_reason) = match inventory_state {
        SidecarInventoryState::Stale => (
            Some("owned native retrieval state is stale; dry-run only".into()),
            None,
        ),
        SidecarInventoryState::Live => (
            None,
            Some("live native retrieval state blocks cleanup".into()),
        ),
        SidecarInventoryState::Incomplete | SidecarInventoryState::Unknown => (
            None,
            Some(
                reasons
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "state ownership is incomplete or unknown".into()),
            ),
        ),
    };
    SidecarInventoryEntry {
        namespace: candidate.namespace,
        owner: state.map(|state| state.owner.clone()),
        profile: state.map(|state| state.profile.clone()),
        state_path: candidate.state_path.display().to_string(),
        state_exists,
        cleanup_command: state
            .map(|state| state.cleanup_command.clone())
            .filter(|command| !command.trim().is_empty()),
        age_ms,
        model,
        state: inventory_state,
        reasons,
        safe_candidate_reason,
        blocking_reason,
    }
}
