use crate::compose::{EmbedModelInventory, embed_model_inventory};
use crate::config::{
    AGENT_SIDECAR_NAMESPACE_PREFIX_V3, LEGACY_SIDECAR_STATE_FILE, LOCAL_SIDECAR_NAMESPACE_V3,
    SIDECAR_STATE_FILE_V3, SidecarRuntimeConfig, current_v3_state_ownership_matches,
    user_cache_root,
};
use crate::generation::{
    manifest_has_current_sidecar_contract, manifest_unavailable_reason_for_runtime,
};
use crate::qdrant_client::QdrantClient;
use crate::retention::{
    FsQdrantGenerationRemover, GLOBAL_GENERATION_GC_LOCK_SCOPE, GenerationRetentionApplyReport,
    GenerationRetentionLock, GenerationRetentionPlan, GenerationRetentionState,
    apply_generation_retention, global_generation_gc_state_file,
    plan_generation_retention_with_unrooted_state, scan_retention_protection,
};
use crate::sidecar::SidecarStateFile;
use anyhow::{Context, Result};
use codestory_store::Store;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const STALE_AFTER_MS: i64 = 24 * 60 * 60 * 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SidecarInventoryState {
    Live,
    Stale,
    Orphaned,
    Incomplete,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SidecarDockerResourceKind {
    Container,
    Network,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SidecarDockerResource {
    pub kind: SidecarDockerResourceKind,
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ports: Option<String>,
    pub labels: BTreeMap<String, String>,
    pub match_reason: String,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compose_project: Option<String>,
    pub containers: Vec<SidecarDockerResource>,
    pub networks: Vec<SidecarDockerResource>,
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
    pub docker_available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub docker_error: Option<String>,
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
    pub removed_docker_resources: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SidecarGcReport {
    pub dry_run: bool,
    pub docker_available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub docker_error: Option<String>,
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

#[derive(Debug, Clone, Default)]
struct DockerInventory {
    available: bool,
    error: Option<String>,
    containers: Vec<SidecarDockerResource>,
    networks: Vec<SidecarDockerResource>,
}

pub fn sidecar_inventory(project_root: &Path) -> Result<SidecarInventoryReport> {
    sidecar_inventory_with_cache_root(project_root, &user_cache_root())
}

pub fn sidecar_inventory_with_storage(
    project_root: &Path,
    storage_path: &Path,
) -> Result<SidecarInventoryReport> {
    let cache_root = user_cache_root();
    let mut report = sidecar_inventory_with_cache_root(project_root, &cache_root)?;
    report.generation_retention = Some(generation_retention_plan_for_storage(
        project_root,
        storage_path,
        &cache_root,
    )?);
    Ok(report)
}

pub fn sidecar_inventory_with_cache_root(
    project_root: &Path,
    cache_root: &Path,
) -> Result<SidecarInventoryReport> {
    let docker = read_docker_inventory();
    Ok(build_inventory(
        project_root,
        cache_root,
        chrono::Utc::now().timestamp_millis(),
        docker,
    ))
}

pub fn sidecar_gc_apply(project_root: &Path) -> Result<SidecarGcReport> {
    sidecar_gc_apply_with_cache_root(project_root, &user_cache_root())
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
    let mut report = sidecar_gc_apply_with_cache_root(project_root, &cache_root)?;
    report.generation_retention = Some(apply_generation_retention_for_storage(
        project_root,
        storage_path,
        &cache_root,
    )?);
    Ok(report)
}

pub fn sidecar_gc_apply_with_cache_root(
    project_root: &Path,
    cache_root: &Path,
) -> Result<SidecarGcReport> {
    let docker = read_docker_inventory();
    let mut remover = FsSidecarGcRemover;
    Ok(apply_inventory(
        project_root,
        cache_root,
        chrono::Utc::now().timestamp_millis(),
        docker,
        None,
        &mut remover,
    ))
}

fn build_inventory(
    project_root: &Path,
    cache_root: &Path,
    now_ms: i64,
    docker: DockerInventory,
) -> SidecarInventoryReport {
    build_inventory_with_model(project_root, cache_root, now_ms, docker, None)
}

fn build_inventory_with_model(
    project_root: &Path,
    cache_root: &Path,
    now_ms: i64,
    docker: DockerInventory,
    model_override: Option<EmbedModelInventory>,
) -> SidecarInventoryReport {
    let mut states = discover_state_candidates(cache_root);
    add_resource_only_candidates(&mut states, cache_root, &docker);
    let mut compose_projects = BTreeSet::new();
    for state in &states {
        if let Some(compose_project) = state
            .state
            .as_ref()
            .and_then(|state| owned_state(state).then(|| state.compose_project.clone()))
        {
            compose_projects.insert(compose_project);
        }
    }

    let layout = SidecarRuntimeConfig::for_project_auto(project_root).layout;
    let model =
        model_override.unwrap_or_else(|| embed_model_inventory(Some(project_root), &layout));
    let mut entries = states
        .into_iter()
        .map(|candidate| {
            let resources = matching_resources(&candidate, &docker, &compose_projects);
            inventory_entry(
                now_ms,
                candidate,
                resources,
                docker.available,
                model.clone(),
            )
        })
        .collect::<Vec<_>>();
    entries.sort_by(|a, b| a.namespace.cmp(&b.namespace));

    SidecarInventoryReport {
        dry_run: true,
        docker_available: docker.available,
        docker_error: docker.error,
        cache_root: cache_root.display().to_string(),
        namespaces: entries,
        generation_retention: None,
    }
}

trait SidecarGcRemover {
    fn remove_path(&mut self, path: &Path) -> Result<()>;
    fn remove_docker_resource(&mut self, resource: &SidecarDockerResource) -> Result<()>;
}

struct FsSidecarGcRemover;

impl SidecarGcRemover for FsSidecarGcRemover {
    fn remove_path(&mut self, path: &Path) -> Result<()> {
        if !path.exists() {
            return Ok(());
        }
        if path.is_dir() {
            std::fs::remove_dir_all(path)
        } else {
            std::fs::remove_file(path)
        }
        .with_context(|| format!("remove {}", path.display()))
    }

    fn remove_docker_resource(&mut self, resource: &SidecarDockerResource) -> Result<()> {
        let target = if resource.id.trim().is_empty() {
            resource.name.as_str()
        } else {
            resource.id.as_str()
        };
        let args = match resource.kind {
            SidecarDockerResourceKind::Container => ["container", "rm", target],
            SidecarDockerResourceKind::Network => ["network", "rm", target],
        };
        let output = Command::new("docker")
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .context("spawn docker")?;
        if !output.status.success() {
            anyhow::bail!(
                "docker {:?} failed: {}{}",
                args,
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        Ok(())
    }
}

fn apply_inventory(
    project_root: &Path,
    cache_root: &Path,
    now_ms: i64,
    docker: DockerInventory,
    model_override: Option<EmbedModelInventory>,
    remover: &mut dyn SidecarGcRemover,
) -> SidecarGcReport {
    let mut states = discover_state_candidates(cache_root);
    add_resource_only_candidates(&mut states, cache_root, &docker);
    let mut compose_projects = BTreeSet::new();
    for state in &states {
        if let Some(compose_project) = state
            .state
            .as_ref()
            .and_then(|state| owned_state(state).then(|| state.compose_project.clone()))
        {
            compose_projects.insert(compose_project);
        }
    }

    let layout = SidecarRuntimeConfig::for_project_auto(project_root).layout;
    let model =
        model_override.unwrap_or_else(|| embed_model_inventory(Some(project_root), &layout));
    let mut rows = states
        .into_iter()
        .map(|candidate| {
            let resources = matching_resources(&candidate, &docker, &compose_projects);
            let entry = inventory_entry(
                now_ms,
                candidate.clone(),
                resources,
                docker.available,
                model.clone(),
            );
            (candidate, entry)
        })
        .collect::<Vec<_>>();
    rows.sort_by(|a, b| a.1.namespace.cmp(&b.1.namespace));

    let mut removed = Vec::new();
    let mut blocked = Vec::new();
    for (candidate, entry) in &rows {
        if entry.safe_candidate_reason.is_some() {
            let result = remove_safe_candidate(candidate, entry, remover);
            if result.errors.is_empty() {
                removed.push(result);
            } else {
                blocked.push(result);
            }
        } else {
            blocked.push(SidecarGcNamespaceResult {
                namespace: entry.namespace.clone(),
                state: entry.state,
                reason: entry
                    .blocking_reason
                    .clone()
                    .unwrap_or_else(|| "not an inventory safe candidate".to_string()),
                removed_paths: Vec::new(),
                removed_docker_resources: Vec::new(),
                errors: Vec::new(),
            });
        }
    }

    SidecarGcReport {
        dry_run: false,
        docker_available: docker.available,
        docker_error: docker.error,
        cache_root: cache_root.display().to_string(),
        removed,
        blocked,
        namespaces: rows.into_iter().map(|(_, entry)| entry).collect(),
        generation_retention: None,
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
        .context("observe sidecar generation inventory lock")?;
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
        .context("lock sidecar generation retention apply")?;
    let plan = build_generation_retention_plan(
        storage_path,
        cache_root,
        &runtime,
        &project_id,
        GenerationRetentionState::Reclaimable,
    );
    let mut remover = FsQdrantGenerationRemover::new(&runtime.layout);
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
            let embedding_device = crate::embeddings::embedding_device_readiness_for_runtime(runtime);
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
            "active retrieval manifest does not satisfy the current sidecar contract; pruning suppressed"
                .to_string(),
        ),
        None => protection
            .errors
            .push("active retrieval manifest is unavailable; pruning suppressed".to_string()),
    }
    let live_qdrant_collections = match QdrantClient::new(layout).list_collection_names() {
        Ok(collections) => collections,
        Err(error) => {
            protection.errors.push(format!(
                "list live Qdrant collections for retention: {error:#}"
            ));
            Vec::new()
        }
    };
    plan_generation_retention_with_unrooted_state(
        layout,
        project_id,
        &protection,
        &live_qdrant_collections,
        unrooted_state,
    )
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

fn remove_safe_candidate(
    candidate: &StateCandidate,
    entry: &SidecarInventoryEntry,
    remover: &mut dyn SidecarGcRemover,
) -> SidecarGcNamespaceResult {
    let mut result = SidecarGcNamespaceResult {
        namespace: entry.namespace.clone(),
        state: entry.state,
        reason: apply_safe_candidate_reason(entry),
        removed_paths: Vec::new(),
        removed_docker_resources: Vec::new(),
        errors: Vec::new(),
    };

    for resource in entry.containers.iter().chain(entry.networks.iter()) {
        if let Err(error) = remover.remove_docker_resource(resource) {
            result.errors.push(format!(
                "remove docker {:?} {}: {error}",
                resource.kind, resource.name
            ));
        } else {
            result
                .removed_docker_resources
                .push(format!("{:?}:{}", resource.kind, resource.name));
        }
    }

    if let Some(state) = candidate.state.as_ref() {
        for path in [
            &state.lexical_data_dir,
            &state.qdrant_data_dir,
            &state.scip_artifacts_root,
        ] {
            remove_path(Path::new(path), remover, &mut result);
        }
    }
    if result.errors.is_empty() {
        remove_path(&candidate.state_path, remover, &mut result);
    }

    if !result.errors.is_empty() {
        result.reason = "cleanup partially failed".to_string();
    }
    result
}

fn apply_safe_candidate_reason(entry: &SidecarInventoryEntry) -> String {
    match entry.state {
        SidecarInventoryState::Stale => {
            "owned state/resources are stale; applying approved cleanup".to_string()
        }
        SidecarInventoryState::Orphaned => {
            "owned state/resources have no live match; applying approved cleanup".to_string()
        }
        _ => entry
            .safe_candidate_reason
            .clone()
            .unwrap_or_else(|| "inventory safe candidate".to_string()),
    }
}

fn remove_path(
    path: &Path,
    remover: &mut dyn SidecarGcRemover,
    result: &mut SidecarGcNamespaceResult,
) {
    if !path.exists() {
        return;
    }
    if let Err(error) = remover.remove_path(path) {
        result
            .errors
            .push(format!("remove {}: {error}", path.display()));
    } else {
        result.removed_paths.push(path.display().to_string());
    }
}

fn discover_state_candidates(cache_root: &Path) -> Vec<StateCandidate> {
    let mut candidates = Vec::new();
    let local_v3 = cache_root.join(SIDECAR_STATE_FILE_V3);
    if local_v3.exists() {
        candidates.push(read_state_candidate(
            LOCAL_SIDECAR_NAMESPACE_V3.to_string(),
            local_v3,
        ));
    }
    let legacy_local = cache_root.join(LEGACY_SIDECAR_STATE_FILE);
    if legacy_local.exists() {
        candidates.push(read_state_candidate("codestory".to_string(), legacy_local));
    }
    let sidecars_root = cache_root.join("sidecars");
    if let Ok(entries) = std::fs::read_dir(&sidecars_root) {
        for entry in entries.flatten() {
            if !entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
                continue;
            }
            let namespace = entry.file_name().to_string_lossy().to_string();
            if !namespace.starts_with("codestory-") {
                continue;
            }
            let state_path = entry.path().join(state_file_name(&namespace));
            candidates.push(if state_path.exists() {
                read_state_candidate(namespace, state_path)
            } else {
                StateCandidate {
                    namespace,
                    state_path,
                    state: None,
                    read_error: Some("state file missing".to_string()),
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

fn add_resource_only_candidates(
    states: &mut Vec<StateCandidate>,
    cache_root: &Path,
    docker: &DockerInventory,
) {
    let mut known = states
        .iter()
        .map(|state| state.namespace.clone())
        .collect::<BTreeSet<_>>();
    for resource in docker.containers.iter().chain(docker.networks.iter()) {
        let Some(namespace) = resource_namespace(resource) else {
            continue;
        };
        if !known.insert(namespace.clone()) {
            continue;
        }
        states.push(StateCandidate {
            state_path: cache_root
                .join("sidecars")
                .join(&namespace)
                .join(state_file_name(&namespace)),
            namespace,
            state: None,
            read_error: Some("no state file for matching Docker resource".to_string()),
        });
    }
}

fn is_v3_sidecar_namespace(namespace: &str) -> bool {
    namespace == LOCAL_SIDECAR_NAMESPACE_V3
        || namespace.starts_with(AGENT_SIDECAR_NAMESPACE_PREFIX_V3)
}

fn state_file_name(namespace: &str) -> &'static str {
    if is_v3_sidecar_namespace(namespace) {
        SIDECAR_STATE_FILE_V3
    } else {
        LEGACY_SIDECAR_STATE_FILE
    }
}

fn current_cleanup_candidate(candidate: &StateCandidate) -> bool {
    candidate.state.as_ref().is_some_and(|state| {
        serde_json::to_value(state)
            .is_ok_and(|value| current_v3_state_ownership_matches(&value, &candidate.namespace))
    })
}

fn matching_resources(
    candidate: &StateCandidate,
    docker: &DockerInventory,
    owned_compose_projects: &BTreeSet<String>,
) -> (Vec<SidecarDockerResource>, Vec<SidecarDockerResource>) {
    let containers = docker
        .containers
        .iter()
        .filter_map(|resource| match_resource(candidate, resource, owned_compose_projects))
        .collect();
    let networks = docker
        .networks
        .iter()
        .filter_map(|resource| match_resource(candidate, resource, owned_compose_projects))
        .collect();
    (containers, networks)
}

fn match_resource(
    candidate: &StateCandidate,
    resource: &SidecarDockerResource,
    owned_compose_projects: &BTreeSet<String>,
) -> Option<SidecarDockerResource> {
    if resource_owner(resource).as_deref() == Some("codestory")
        && resource_namespace(resource).as_deref() == Some(candidate.namespace.as_str())
    {
        let mut resource = resource.clone();
        resource.match_reason = "matched codestory namespace label".to_string();
        return Some(resource);
    }
    let compose_project = compose_project(resource);
    if let Some(state) = candidate.state.as_ref()
        && owned_state(state)
        && resource_owner(resource).as_deref() == Some("codestory")
        && compose_project.as_deref() == Some(state.compose_project.as_str())
    {
        let mut resource = resource.clone();
        resource.match_reason = "matched compose project from owned state".to_string();
        return Some(resource);
    }
    if resource.kind == SidecarDockerResourceKind::Network
        && !resource.labels.contains_key("dev.codestory.owner")
        && compose_project
            .as_ref()
            .is_some_and(|project| owned_compose_projects.contains(project))
        && candidate
            .state
            .as_ref()
            .is_some_and(|state| state.compose_project == compose_project.unwrap())
    {
        let mut resource = resource.clone();
        resource.match_reason = "matched unlabeled compose network via owned state".to_string();
        return Some(resource);
    }
    None
}

fn inventory_entry(
    now_ms: i64,
    candidate: StateCandidate,
    resources: (Vec<SidecarDockerResource>, Vec<SidecarDockerResource>),
    docker_available: bool,
    model: EmbedModelInventory,
) -> SidecarInventoryEntry {
    let (containers, networks) = resources;
    let state_exists = candidate.state.is_some();
    let state = candidate.state.as_ref();
    let owner = state.map(|state| state.owner.clone());
    let profile = state.map(|state| state.profile.clone());
    let cleanup_command = state
        .map(|state| state.cleanup_command.clone())
        .filter(|command| !command.trim().is_empty());
    let compose_project = state.map(|state| state.compose_project.clone());
    let age_ms = state.map(|state| now_ms.saturating_sub(state.started_at_epoch_ms).max(0));

    let mut reasons = Vec::new();
    if let Some(error) = candidate.read_error.as_ref() {
        reasons.push(error.clone());
    }
    if state.is_some_and(|state| !owned_state(state)) {
        reasons.push("state owner is not codestory".to_string());
    }
    if state.is_some() && !model.required_gguf_present {
        reasons.push(format!("missing required GGUF {}", model.required_gguf));
    }
    if state.is_some_and(|state| {
        !Path::new(&state.lexical_data_dir).exists()
            || !Path::new(&state.qdrant_data_dir).exists()
            || !Path::new(&state.scip_artifacts_root).exists()
    }) {
        reasons.push("one or more sidecar data directories are missing".to_string());
    }
    let live_container = containers.iter().any(resource_running);
    let status = classify_entry(
        state,
        state_exists,
        live_container,
        docker_available,
        age_ms,
        model.required_gguf_present,
        &containers,
        &networks,
    );
    let (safe_candidate_reason, blocking_reason) = if current_cleanup_candidate(&candidate) {
        candidate_reasons(status, &reasons)
    } else {
        (
            None,
            Some(
                "state ownership, namespace, profile, or project generation is not an exact V3 match; inventory-only"
                    .to_string(),
            ),
        )
    };

    SidecarInventoryEntry {
        namespace: candidate.namespace,
        owner,
        profile,
        state_path: candidate.state_path.display().to_string(),
        state_exists,
        cleanup_command,
        age_ms,
        compose_project,
        containers,
        networks,
        model,
        state: status,
        reasons,
        safe_candidate_reason,
        blocking_reason,
    }
}

#[allow(clippy::too_many_arguments)]
fn classify_entry(
    state: Option<&SidecarStateFile>,
    state_exists: bool,
    live_container: bool,
    docker_available: bool,
    age_ms: Option<i64>,
    model_ready: bool,
    containers: &[SidecarDockerResource],
    networks: &[SidecarDockerResource],
) -> SidecarInventoryState {
    if state.is_some_and(|state| !owned_state(state))
        || (!state_exists
            && containers
                .iter()
                .chain(networks.iter())
                .any(|resource| resource_owner(resource).as_deref() != Some("codestory")))
    {
        return SidecarInventoryState::Unknown;
    }
    if !state_exists {
        return if containers.is_empty() && networks.is_empty() {
            SidecarInventoryState::Incomplete
        } else {
            SidecarInventoryState::Orphaned
        };
    }
    if state.is_some_and(|_| !model_ready) {
        return SidecarInventoryState::Incomplete;
    }
    if live_container {
        return SidecarInventoryState::Live;
    }
    if !docker_available {
        return SidecarInventoryState::Unknown;
    }
    if age_ms.is_some_and(|age| age >= STALE_AFTER_MS) {
        return SidecarInventoryState::Stale;
    }
    if containers.is_empty() && networks.is_empty() {
        return SidecarInventoryState::Orphaned;
    }
    SidecarInventoryState::Stale
}

fn candidate_reasons(
    status: SidecarInventoryState,
    reasons: &[String],
) -> (Option<String>, Option<String>) {
    match status {
        SidecarInventoryState::Stale => (
            Some("owned state/resources are stale; dry-run only".to_string()),
            None,
        ),
        SidecarInventoryState::Orphaned => (
            Some("owned state/resources have no live match; dry-run only".to_string()),
            None,
        ),
        SidecarInventoryState::Live => (
            None,
            Some("matching running containers block cleanup".to_string()),
        ),
        SidecarInventoryState::Incomplete => (
            None,
            Some(
                reasons
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "inventory is incomplete".to_string()),
            ),
        ),
        SidecarInventoryState::Unknown => (
            None,
            Some(
                reasons
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "ownership is unknown".to_string()),
            ),
        ),
    }
}

fn owned_state(state: &SidecarStateFile) -> bool {
    state.owner == "codestory"
}

fn resource_namespace(resource: &SidecarDockerResource) -> Option<String> {
    resource.labels.get("dev.codestory.namespace").cloned()
}

fn resource_owner(resource: &SidecarDockerResource) -> Option<String> {
    resource.labels.get("dev.codestory.owner").cloned()
}

fn compose_project(resource: &SidecarDockerResource) -> Option<String> {
    resource
        .labels
        .get("com.docker.compose.project")
        .cloned()
        .or_else(|| resource.name.strip_suffix("_default").map(str::to_string))
}

fn resource_running(resource: &SidecarDockerResource) -> bool {
    resource
        .state
        .as_deref()
        .is_some_and(|state| state.eq_ignore_ascii_case("running"))
        || resource
            .status
            .as_deref()
            .is_some_and(|status| status.to_ascii_lowercase().starts_with("up "))
}

fn read_docker_inventory() -> DockerInventory {
    let containers = read_docker_resources(
        SidecarDockerResourceKind::Container,
        &["container", "ls", "-a", "--format", "{{json .}}"],
    );
    let networks = read_docker_resources(
        SidecarDockerResourceKind::Network,
        &["network", "ls", "--format", "{{json .}}"],
    );
    match (containers, networks) {
        (Ok(containers), Ok(networks)) => DockerInventory {
            available: true,
            error: None,
            containers,
            networks,
        },
        (Err(error), Ok(networks)) => DockerInventory {
            available: false,
            error: Some(error.to_string()),
            containers: Vec::new(),
            networks,
        },
        (Ok(containers), Err(error)) => DockerInventory {
            available: false,
            error: Some(error.to_string()),
            containers,
            networks: Vec::new(),
        },
        (Err(left), Err(right)) => DockerInventory {
            available: false,
            error: Some(format!("{left}; {right}")),
            containers: Vec::new(),
            networks: Vec::new(),
        },
    }
}

fn read_docker_resources(
    kind: SidecarDockerResourceKind,
    args: &[&str],
) -> Result<Vec<SidecarDockerResource>> {
    let output = Command::new("docker")
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .context("spawn docker")?;
    if !output.status.success() {
        anyhow::bail!(
            "docker {:?} failed: {}{}",
            args,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| docker_resource_from_json_line(kind.clone(), line))
        .collect())
}

fn docker_resource_from_json_line(
    kind: SidecarDockerResourceKind,
    line: &str,
) -> Option<SidecarDockerResource> {
    let value = serde_json::from_str::<serde_json::Value>(line).ok()?;
    Some(SidecarDockerResource {
        kind,
        id: string_field(&value, &["ID"]).unwrap_or_default(),
        name: string_field(&value, &["Names", "Name"]).unwrap_or_default(),
        state: string_field(&value, &["State"]),
        status: string_field(&value, &["Status"]),
        ports: string_field(&value, &["Ports"]).filter(|value| !value.trim().is_empty()),
        labels: labels_from_value(value.get("Labels")),
        match_reason: String::new(),
    })
}

fn string_field(value: &serde_json::Value, names: &[&str]) -> Option<String> {
    names
        .iter()
        .find_map(|name| value.get(*name).and_then(|value| value.as_str()))
        .map(str::to_string)
}

fn labels_from_value(value: Option<&serde_json::Value>) -> BTreeMap<String, String> {
    match value {
        Some(serde_json::Value::Object(object)) => object
            .iter()
            .filter_map(|(key, value)| value.as_str().map(|value| (key.clone(), value.to_string())))
            .collect(),
        Some(serde_json::Value::String(labels)) => labels
            .split(',')
            .filter_map(|label| label.split_once('='))
            .map(|(key, value)| (key.trim().to_string(), value.trim().to_string()))
            .collect(),
        _ => BTreeMap::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn stale_active_manifest_records_a_pruning_suppression_error() {
        let root = tempdir().expect("root");
        let store = Store::open(root.path().join("codestory.db")).expect("store");
        let project_id = "repo-v1-project";
        let mut manifest = crate::test_support::retrieval_manifest_fixture(project_id, "input");
        manifest.embedding_dim = Some(1);
        let mut errors = Vec::new();
        let mut runtime = SidecarRuntimeConfig::local();
        runtime.embedding.backend = "llamacpp".into();
        runtime.embedding.profile = "bge-base-en-v1.5".into();
        runtime.embedding.model_id = None;

        record_manifest_retention_freshness(&store, project_id, &manifest, &runtime, &mut errors);

        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("active retrieval manifest is stale; pruning suppressed"));
        assert!(errors[0].contains("sidecar_embedding_dim_changed"));
    }

    #[test]
    fn inventory_reports_unrooted_bytes_as_building_while_writer_is_active() {
        let root = tempdir().expect("root");
        let project_id = "repo-v1-project";
        let mut runtime = SidecarRuntimeConfig::local();
        runtime.layout = crate::config::SidecarLayout {
            qdrant_http_port: 9,
            qdrant_grpc_port: 10,
            lexical_data_dir: root.path().join("lexical"),
            qdrant_data_dir: root.path().join("qdrant"),
            scip_artifacts_root: root.path().join("scip"),
            state_file: root.path().join("retrieval-sidecars.json"),
        };
        for suffix in ["aaaaaaaaaaaaaaaa", "bbbbbbbbbbbbbbbb"] {
            let generation = format!("{project_id}-{suffix}");
            for path in [
                runtime
                    .layout
                    .lexical_data_dir
                    .join("shards")
                    .join(&generation),
                runtime.layout.scip_artifacts_root.join(&generation),
                runtime
                    .layout
                    .qdrant_data_dir
                    .join("collections")
                    .join(format!("codestory_{project_id}_{suffix}")),
            ] {
                std::fs::create_dir_all(&path).expect("generation dir");
                std::fs::write(path.join("data"), b"x").expect("generation bytes");
            }
        }
        let protection = crate::retention::RetentionProtectionScan {
            authoritative_active: vec![crate::test_support::retrieval_manifest_fixture(
                project_id,
                "aaaaaaaaaaaaaaaa",
            )],
            ..crate::retention::RetentionProtectionScan::default()
        };
        let writer = GenerationRetentionLock::acquire(&runtime.layout.state_file, project_id)
            .expect("writer");

        let (shared, state) = inventory_retention_view(&runtime.layout, project_id).expect("view");
        assert!(shared.is_none());
        let plan = plan_generation_retention_with_unrooted_state(
            &runtime.layout,
            project_id,
            &protection,
            &[],
            state,
        );
        assert_eq!(plan.active_bytes, 3);
        assert_eq!(plan.building_bytes, 3);
        assert_eq!(plan.reclaimable_bytes, 0);
        assert!(plan.pruning_suppressed);

        drop(writer);
        let (_shared, state) =
            inventory_retention_view(&runtime.layout, project_id).expect("stable view");
        let plan = plan_generation_retention_with_unrooted_state(
            &runtime.layout,
            project_id,
            &protection,
            &[],
            state,
        );
        assert_eq!(plan.building_bytes, 0);
        assert_eq!(plan.reclaimable_bytes, 3);
        assert!(!plan.pruning_suppressed);
    }

    fn write_state(path: &Path, state: &SidecarStateFile) {
        std::fs::create_dir_all(path.parent().expect("state parent")).expect("state parent");
        std::fs::write(
            path,
            serde_json::to_string_pretty(state).expect("state json"),
        )
        .expect("write state");
    }

    fn state(namespace: &str, root: &Path, started_at_epoch_ms: i64) -> SidecarStateFile {
        let agent = namespace.starts_with(AGENT_SIDECAR_NAMESPACE_PREFIX_V3);
        SidecarStateFile {
            project_identity: agent.then(|| codestory_workspace::project_identity_v3(root)),
            owner: "codestory".to_string(),
            profile: if agent { "agent" } else { "local" }.to_string(),
            namespace: namespace.to_string(),
            compose_project: namespace.to_string(),
            run_id: agent.then(|| "test".to_string()),
            qdrant_http_port: 21002,
            qdrant_grpc_port: 21003,
            embed_http_port: 21004,
            embed_url: "http://127.0.0.1:21004/v1/embeddings".to_string(),
            embedding_endpoint_origin: Some(crate::config::EmbeddingEndpointOrigin::ManagedSidecar),
            embedding_endpoint_fingerprint_sha256: Some("hmac-sha256:fixture".to_string()),
            embedding_device_policy: "accelerator_required".to_string(),
            embedding_device_state: "unknown".to_string(),
            embedding_device_observation_source: "sidecar_unobserved".to_string(),
            embedding_detected_provider: None,
            embedding_detected_gpu: None,
            embedding_accelerator_requested: false,
            embedding_accelerator_request_provider: None,
            embedding_accelerator_request_device: None,
            embedding_cpu_allowed: false,
            embedding_launch: None,
            embedding_launch_ownership: crate::sidecar::EmbeddingLaunchOwnership::Owner,
            sidecar_images: crate::config::default_sidecar_image_pins(),
            lexical_data_dir: root.join("lexical").display().to_string(),
            qdrant_data_dir: root.join("qdrant").display().to_string(),
            scip_artifacts_root: root.join("scip").display().to_string(),
            compose_file: None,
            compose_started_by_bootstrap: true,
            cleanup_command: format!(
                "codestory-cli retrieval down --project C:/repo --profile agent --run-id {}",
                namespace
            ),
            started_at_epoch_ms,
        }
    }

    fn create_state_dirs(state: &SidecarStateFile) {
        std::fs::create_dir_all(&state.lexical_data_dir).expect("lexical dir");
        std::fs::create_dir_all(&state.qdrant_data_dir).expect("qdrant dir");
        std::fs::create_dir_all(&state.scip_artifacts_root).expect("scip dir");
    }

    fn test_model(project: &Path, present: bool) -> EmbedModelInventory {
        let dir = project.join("models").join("gguf").join("bge-base-en-v1.5");
        EmbedModelInventory {
            model_dir: Some(dir.display().to_string()),
            required_gguf: crate::embeddings::BGE_BASE_EN_V1_5_GGUF.to_string(),
            required_gguf_present: present,
            candidate_dirs: vec![dir.display().to_string()],
        }
    }

    fn test_inventory(
        project: &Path,
        cache: &Path,
        now_ms: i64,
        docker: DockerInventory,
        model_ready: bool,
    ) -> SidecarInventoryReport {
        build_inventory_with_model(
            project,
            cache,
            now_ms,
            docker,
            Some(test_model(project, model_ready)),
        )
    }

    fn container(namespace: &str, running: bool) -> SidecarDockerResource {
        let mut labels = BTreeMap::new();
        labels.insert("dev.codestory.owner".to_string(), "codestory".to_string());
        labels.insert("dev.codestory.namespace".to_string(), namespace.to_string());
        labels.insert(
            "com.docker.compose.project".to_string(),
            namespace.to_string(),
        );
        SidecarDockerResource {
            kind: SidecarDockerResourceKind::Container,
            id: "container-id".to_string(),
            name: format!("{namespace}-qdrant"),
            state: Some(if running { "running" } else { "exited" }.to_string()),
            status: Some(if running { "Up 1 minute" } else { "Exited" }.to_string()),
            ports: Some("127.0.0.1:21002->6333/tcp".to_string()),
            labels,
            match_reason: String::new(),
        }
    }

    fn foreign_container(namespace: &str) -> SidecarDockerResource {
        let mut container = container(namespace, false);
        container.labels.insert(
            "dev.codestory.owner".to_string(),
            "someone-else".to_string(),
        );
        container.id = "foreign-container-id".to_string();
        container.name = format!("{namespace}-foreign-qdrant");
        container
    }

    fn unlabeled_compose_network(compose_project: &str) -> SidecarDockerResource {
        let mut labels = BTreeMap::new();
        labels.insert(
            "com.docker.compose.project".to_string(),
            compose_project.to_string(),
        );
        SidecarDockerResource {
            kind: SidecarDockerResourceKind::Network,
            id: "network-id".to_string(),
            name: format!("{compose_project}_default"),
            state: None,
            status: None,
            ports: None,
            labels,
            match_reason: String::new(),
        }
    }

    struct FailingPathRemover {
        fail_fragment: String,
    }

    impl SidecarGcRemover for FailingPathRemover {
        fn remove_path(&mut self, path: &Path) -> Result<()> {
            if path.display().to_string().contains(&self.fail_fragment) {
                anyhow::bail!("planned remove failure");
            }
            if path.is_dir() {
                std::fs::remove_dir_all(path).context("remove dir")
            } else {
                std::fs::remove_file(path).context("remove file")
            }
        }

        fn remove_docker_resource(&mut self, _resource: &SidecarDockerResource) -> Result<()> {
            Ok(())
        }
    }

    #[test]
    fn stale_state_dirs_are_safe_dry_run_candidates() {
        let project = tempdir().expect("project");
        let cache = tempdir().expect("cache");
        let namespace = "codestory-agent-v3-stale-test";
        let root = cache.path().join("sidecars").join(namespace);
        let state = state(namespace, &root, 0);
        create_state_dirs(&state);
        write_state(&root.join(SIDECAR_STATE_FILE_V3), &state);

        let report = test_inventory(
            project.path(),
            cache.path(),
            STALE_AFTER_MS + 1,
            DockerInventory {
                available: true,
                ..DockerInventory::default()
            },
            true,
        );

        let entry = &report.namespaces[0];
        assert!(report.dry_run);
        assert_eq!(entry.state, SidecarInventoryState::Stale);
        assert!(entry.safe_candidate_reason.is_some());
        assert!(
            entry
                .cleanup_command
                .as_deref()
                .unwrap()
                .contains("retrieval down")
        );
    }

    #[test]
    fn live_matching_resources_block_cleanup_and_report_ports() {
        let project = tempdir().expect("project");
        let cache = tempdir().expect("cache");
        let namespace = "codestory-agent-v3-live-test";
        let root = cache.path().join("sidecars").join(namespace);
        let state = state(namespace, &root, 100);
        create_state_dirs(&state);
        write_state(&root.join(SIDECAR_STATE_FILE_V3), &state);

        let report = test_inventory(
            project.path(),
            cache.path(),
            200,
            DockerInventory {
                available: true,
                containers: vec![container(namespace, true)],
                networks: Vec::new(),
                error: None,
            },
            true,
        );

        let entry = &report.namespaces[0];
        assert_eq!(entry.state, SidecarInventoryState::Live);
        assert!(
            entry
                .blocking_reason
                .as_deref()
                .unwrap()
                .contains("running")
        );
        assert_eq!(
            entry.containers[0].ports.as_deref(),
            Some("127.0.0.1:21002->6333/tcp")
        );
    }

    #[test]
    fn missing_model_file_marks_inventory_incomplete() {
        let project = tempdir().expect("project");
        let cache = tempdir().expect("cache");
        let namespace = "codestory-agent-v3-missing-model";
        let root = cache.path().join("sidecars").join(namespace);
        let state = state(namespace, &root, 100);
        create_state_dirs(&state);
        write_state(&root.join(SIDECAR_STATE_FILE_V3), &state);

        let report = test_inventory(
            project.path(),
            cache.path(),
            200,
            DockerInventory {
                available: true,
                ..DockerInventory::default()
            },
            false,
        );

        let entry = &report.namespaces[0];
        assert_eq!(entry.state, SidecarInventoryState::Incomplete);
        assert!(!entry.model.required_gguf_present);
        assert!(
            entry
                .blocking_reason
                .as_deref()
                .unwrap()
                .contains(crate::embeddings::BGE_BASE_EN_V1_5_GGUF)
        );
    }

    #[test]
    fn resource_without_state_is_orphaned_but_inventory_only() {
        let project = tempdir().expect("project");
        let cache = tempdir().expect("cache");
        let namespace = "codestory-agent-v3-resource-only";

        let report = test_inventory(
            project.path(),
            cache.path(),
            200,
            DockerInventory {
                available: true,
                containers: vec![container(namespace, false)],
                networks: Vec::new(),
                error: None,
            },
            true,
        );

        let entry = &report.namespaces[0];
        assert_eq!(entry.namespace, namespace);
        assert_eq!(entry.state, SidecarInventoryState::Orphaned);
        assert!(entry.safe_candidate_reason.is_none());
        assert!(
            entry
                .blocking_reason
                .as_deref()
                .is_some_and(|reason| reason.contains("inventory-only"))
        );
    }

    #[test]
    fn unlabeled_compose_network_matches_only_through_owned_state() {
        let project = tempdir().expect("project");
        let cache = tempdir().expect("cache");
        let namespace = "codestory-agent-v3-compose-network";
        let root = cache.path().join("sidecars").join(namespace);
        let state = state(namespace, &root, 100);
        create_state_dirs(&state);
        write_state(&root.join(SIDECAR_STATE_FILE_V3), &state);

        let report = test_inventory(
            project.path(),
            cache.path(),
            200,
            DockerInventory {
                available: true,
                containers: Vec::new(),
                networks: vec![unlabeled_compose_network(namespace)],
                error: None,
            },
            true,
        );

        let entry = &report.namespaces[0];
        assert_eq!(entry.networks.len(), 1);
        assert_eq!(
            entry.networks[0].match_reason,
            "matched unlabeled compose network via owned state"
        );
        assert!(entry.safe_candidate_reason.is_some());
    }

    #[test]
    fn mismatched_v3_state_is_not_a_cleanup_candidate() {
        let project = tempdir().expect("project");
        let cache = tempdir().expect("cache");
        let namespace = "codestory-agent-v3-mismatched-state";
        let root = cache.path().join("sidecars").join(namespace);
        let mut state = state(namespace, &root, 100);
        state.compose_project = "codestory-agent-v3-other-state".to_string();
        create_state_dirs(&state);
        write_state(&root.join(SIDECAR_STATE_FILE_V3), &state);

        let report = test_inventory(
            project.path(),
            cache.path(),
            200,
            DockerInventory {
                available: true,
                ..DockerInventory::default()
            },
            true,
        );

        let entry = &report.namespaces[0];
        assert_eq!(entry.state, SidecarInventoryState::Orphaned);
        assert!(entry.safe_candidate_reason.is_none());
        assert!(
            entry
                .blocking_reason
                .as_deref()
                .unwrap()
                .contains("exact V3")
        );
    }

    #[test]
    fn dry_run_inventory_does_not_delete_state_files() {
        let project = tempdir().expect("project");
        let cache = tempdir().expect("cache");
        let namespace = "codestory-agent-v3-no-delete";
        let root = cache.path().join("sidecars").join(namespace);
        let state_path = root.join(SIDECAR_STATE_FILE_V3);
        let state = state(namespace, &root, 0);
        create_state_dirs(&state);
        write_state(&state_path, &state);

        let report = test_inventory(
            project.path(),
            cache.path(),
            STALE_AFTER_MS + 1,
            DockerInventory {
                available: true,
                ..DockerInventory::default()
            },
            true,
        );

        assert!(report.dry_run);
        assert!(state_path.exists(), "inventory must not remove state files");
    }

    #[test]
    fn apply_removes_only_safe_candidates() {
        let project = tempdir().expect("project");
        let cache = tempdir().expect("cache");
        let stale_namespace = "codestory-agent-v3-stale-apply";
        let live_namespace = "codestory-agent-v3-live-apply";
        let unknown_namespace = "codestory-agent-v3-unknown-apply";
        let legacy_path = cache.path().join(LEGACY_SIDECAR_STATE_FILE);
        let legacy = state("codestory", cache.path(), 0);
        create_state_dirs(&legacy);
        write_state(&legacy_path, &legacy);

        let stale_root = cache.path().join("sidecars").join(stale_namespace);
        let stale_path = stale_root.join(SIDECAR_STATE_FILE_V3);
        let stale = state(stale_namespace, &stale_root, 0);
        create_state_dirs(&stale);
        write_state(&stale_path, &stale);

        let live_root = cache.path().join("sidecars").join(live_namespace);
        let live_path = live_root.join(SIDECAR_STATE_FILE_V3);
        let live = state(live_namespace, &live_root, 100);
        create_state_dirs(&live);
        write_state(&live_path, &live);

        let unknown_root = cache.path().join("sidecars").join(unknown_namespace);
        let unknown_path = unknown_root.join(SIDECAR_STATE_FILE_V3);
        let mut unknown = state(unknown_namespace, &unknown_root, 100);
        unknown.owner = "someone-else".to_string();
        create_state_dirs(&unknown);
        write_state(&unknown_path, &unknown);

        let mut remover = FsSidecarGcRemover;
        let report = apply_inventory(
            project.path(),
            cache.path(),
            STALE_AFTER_MS + 1,
            DockerInventory {
                available: true,
                containers: vec![container(live_namespace, true)],
                networks: Vec::new(),
                error: None,
            },
            Some(test_model(project.path(), true)),
            &mut remover,
        );

        assert_eq!(report.removed.len(), 1);
        assert_eq!(report.removed[0].namespace, stale_namespace);
        assert!(!report.removed[0].reason.contains("dry-run only"));
        assert!(report.removed[0].reason.contains("approved cleanup"));
        assert!(
            !stale_path.exists(),
            "safe stale state file should be removed"
        );
        assert!(!Path::new(&stale.lexical_data_dir).exists());
        assert!(!Path::new(&stale.qdrant_data_dir).exists());
        assert!(!Path::new(&stale.scip_artifacts_root).exists());
        assert!(live_path.exists(), "live state file must stay blocked");
        assert!(
            unknown_path.exists(),
            "unknown owner state file must stay blocked"
        );
        assert!(legacy_path.exists(), "legacy local state must stay blocked");
        assert!(Path::new(&legacy.lexical_data_dir).exists());
        assert!(
            report
                .blocked
                .iter()
                .any(|blocked| blocked.namespace == live_namespace
                    && blocked.reason.contains("running containers"))
        );
        assert!(
            report
                .blocked
                .iter()
                .any(|blocked| blocked.namespace == unknown_namespace
                    && blocked.reason.contains("owner"))
        );
        assert!(
            report
                .blocked
                .iter()
                .any(|blocked| blocked.namespace == "codestory"
                    && blocked.reason.contains("inventory-only"))
        );
    }

    #[test]
    fn apply_does_not_remove_foreign_resource_with_matching_namespace() {
        let project = tempdir().expect("project");
        let cache = tempdir().expect("cache");
        let namespace = "codestory-agent-v3-foreign-resource";
        let root = cache.path().join("sidecars").join(namespace);
        let state = state(namespace, &root, 0);
        create_state_dirs(&state);
        write_state(&root.join(SIDECAR_STATE_FILE_V3), &state);

        let mut remover = FsSidecarGcRemover;
        let report = apply_inventory(
            project.path(),
            cache.path(),
            STALE_AFTER_MS + 1,
            DockerInventory {
                available: true,
                containers: vec![foreign_container(namespace)],
                networks: Vec::new(),
                error: None,
            },
            Some(test_model(project.path(), true)),
            &mut remover,
        );

        assert_eq!(report.removed.len(), 1);
        assert_eq!(report.removed[0].namespace, namespace);
        assert!(
            report.removed[0].removed_docker_resources.is_empty(),
            "foreign-owned matching namespace resource must not be removed: {:?}",
            report.removed[0]
        );
        assert!(report.namespaces[0].containers.is_empty());
    }

    #[test]
    fn apply_refuses_live_namespaces() {
        let project = tempdir().expect("project");
        let cache = tempdir().expect("cache");
        let namespace = "codestory-agent-v3-live-refused";
        let root = cache.path().join("sidecars").join(namespace);
        let state_path = root.join(SIDECAR_STATE_FILE_V3);
        let state = state(namespace, &root, 100);
        create_state_dirs(&state);
        write_state(&state_path, &state);

        let mut remover = FsSidecarGcRemover;
        let report = apply_inventory(
            project.path(),
            cache.path(),
            200,
            DockerInventory {
                available: true,
                containers: vec![container(namespace, true)],
                networks: Vec::new(),
                error: None,
            },
            Some(test_model(project.path(), true)),
            &mut remover,
        );

        assert!(report.removed.is_empty());
        assert_eq!(report.blocked[0].namespace, namespace);
        assert!(report.blocked[0].reason.contains("running containers"));
        assert!(state_path.exists(), "live namespace must not be removed");
    }

    #[test]
    fn apply_refuses_unknown_ownership() {
        let project = tempdir().expect("project");
        let cache = tempdir().expect("cache");
        let namespace = "codestory-agent-v3-unknown-refused";
        let root = cache.path().join("sidecars").join(namespace);
        let state_path = root.join(SIDECAR_STATE_FILE_V3);
        let mut state = state(namespace, &root, 0);
        state.owner = "someone-else".to_string();
        create_state_dirs(&state);
        write_state(&state_path, &state);

        let mut remover = FsSidecarGcRemover;
        let report = apply_inventory(
            project.path(),
            cache.path(),
            STALE_AFTER_MS + 1,
            DockerInventory {
                available: true,
                ..DockerInventory::default()
            },
            Some(test_model(project.path(), true)),
            &mut remover,
        );

        assert!(report.removed.is_empty());
        assert_eq!(report.blocked[0].namespace, namespace);
        assert!(report.blocked[0].reason.contains("owner"));
        assert!(
            state_path.exists(),
            "unknown owner state file must not be removed"
        );
    }

    #[test]
    fn apply_reports_partial_failures() {
        let project = tempdir().expect("project");
        let cache = tempdir().expect("cache");
        let namespace = "codestory-agent-v3-partial-failure";
        let root = cache.path().join("sidecars").join(namespace);
        let state_path = root.join(SIDECAR_STATE_FILE_V3);
        let state = state(namespace, &root, 0);
        create_state_dirs(&state);
        write_state(&state_path, &state);

        let mut remover = FailingPathRemover {
            fail_fragment: "qdrant".to_string(),
        };
        let report = apply_inventory(
            project.path(),
            cache.path(),
            STALE_AFTER_MS + 1,
            DockerInventory {
                available: true,
                ..DockerInventory::default()
            },
            Some(test_model(project.path(), true)),
            &mut remover,
        );

        assert!(report.removed.is_empty());
        assert_eq!(report.blocked.len(), 1);
        let blocked = &report.blocked[0];
        assert_eq!(blocked.namespace, namespace);
        assert_eq!(blocked.reason, "cleanup partially failed");
        assert!(blocked.errors.iter().any(|error| error.contains("qdrant")));
        assert!(
            blocked
                .removed_paths
                .iter()
                .any(|path| path.contains("lexical")),
            "partial successes should stay visible: {blocked:?}"
        );
        assert!(Path::new(&state.qdrant_data_dir).exists());
    }
}
