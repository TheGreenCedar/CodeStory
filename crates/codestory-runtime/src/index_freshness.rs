use super::{
    AffectedOperationIdentityIndex, ApiError, CURRENT_SCHEMA_VERSION, HashMap, HashSet,
    IndexFreshnessChangeKindDto, IndexFreshnessDto, IndexFreshnessObservation,
    IndexFreshnessSampleDto, IndexFreshnessStatusDto, Path, PathBuf, RefreshExecutionPlan,
    RefreshInputs, SourceIndexPolicy, Storage, WorkspaceInventoryOutcome, WorkspaceManifest,
    WorkspaceMemberIndexDto, WorkspacePathIdentity, clamp_u128_to_u32, clamp_usize_to_u32,
    runtime_relative_path, source_policy_exclusion_candidate, validate_source_policy_exclusions,
    validate_structural_text_units,
};
#[cfg(test)]
use std::cell::RefCell;
use std::io;
use std::time::{Instant, UNIX_EPOCH};

const INDEX_FRESHNESS_INDEXED_FILE_CAP: usize = 25_000;
const INDEX_FRESHNESS_CURRENT_FILE_CAP: usize = 25_000;
const INDEX_FRESHNESS_SAMPLE_LIMIT: usize = 8;
const INDEX_FRESHNESS_CACHE_DEFAULT_TTL_SECS: u64 = 60;
#[cfg(test)]
pub(super) const EXACT_SYMBOL_HYBRID_MAX_RESULTS_CAP: usize = 80;

#[cfg(test)]
thread_local! {
    static AFTER_INDEX_FRESHNESS_FENCE_TEST_HOOK: RefCell<Option<Box<dyn FnOnce()>>> =
        const { RefCell::new(None) };
}

#[cfg(test)]
pub(super) fn arm_after_index_freshness_fence_test_hook(hook: impl FnOnce() + 'static) {
    AFTER_INDEX_FRESHNESS_FENCE_TEST_HOOK.with(|slot| *slot.borrow_mut() = Some(Box::new(hook)));
}

#[cfg(test)]
fn run_after_index_freshness_fence_test_hook() {
    let hook = AFTER_INDEX_FRESHNESS_FENCE_TEST_HOOK.with(|slot| slot.borrow_mut().take());
    if let Some(hook) = hook {
        hook();
    }
}

pub(super) fn not_checked_index_freshness(
    reason: impl Into<String>,
    indexed_file_count: u32,
    started_at: Instant,
) -> IndexFreshnessDto {
    IndexFreshnessDto {
        status: IndexFreshnessStatusDto::NotChecked,
        changed_file_count: 0,
        new_file_count: 0,
        removed_file_count: 0,
        checked_file_count: 0,
        indexed_file_count,
        duration_ms: clamp_u128_to_u32(started_at.elapsed().as_millis()),
        reason: Some(reason.into()),
        samples: Vec::new(),
    }
}

pub(super) fn indexable_source_path(path: &Path) -> bool {
    if path.to_str().is_some_and(|path| {
        !Path::new(path).is_absolute()
            && codestory_contracts::language_support::is_structural_source_path(path)
            && codestory_contracts::language_support::structural_source_path_exclusion(path)
                .is_some()
    }) {
        return false;
    }
    let tree_sitter_supported = path
        .extension()
        .and_then(|value| value.to_str())
        .and_then(codestory_indexer::get_language_for_ext)
        .is_some();
    tree_sitter_supported
        || codestory_indexer::template_pipeline::template_kind_for_path(path).is_some()
        || codestory_indexer::structural::is_structural_candidate_path(path)
        || codestory_indexer::is_text_only_candidate_path(path)
        || looks_like_openapi_source_path(path)
}

pub(super) fn indexable_source_path_in_workspace(root: &Path, path: &Path) -> bool {
    let Some(relative) = codestory_workspace::workspace_relative_path(root, path) else {
        return false;
    };
    indexable_source_path(&relative)
}

pub(super) fn indexable_source_path_with_root(root: Option<&Path>, path: &Path) -> bool {
    root.map_or_else(
        || indexable_source_path(path),
        |root| indexable_source_path_in_workspace(root, path),
    )
}

fn looks_like_openapi_source_path(path: &Path) -> bool {
    if !codestory_indexer::is_openapi_candidate_path(path) {
        return false;
    }
    let Ok(source) = std::fs::read_to_string(path) else {
        return true;
    };
    codestory_indexer::looks_like_openapi_schema(&source)
}

pub(super) fn index_freshness_observation_from_storage(
    root: &Path,
    workspace: &WorkspaceManifest,
    storage: &Storage,
    policy: &SourceIndexPolicy,
) -> IndexFreshnessObservation {
    let mut identities = AffectedOperationIdentityIndex::native();
    index_freshness_observation_from_storage_with_identities(
        root,
        workspace,
        storage,
        policy,
        &mut identities,
    )
}

enum IndexFreshnessFenceFailure {
    IncompleteRun,
    Unavailable(String),
}

struct IndexFreshnessInventory {
    indexed_file_count: u32,
    removed_paths: HashMap<i64, PathBuf>,
    refresh_inputs: RefreshInputs,
    stored_policy_exclusions: Vec<codestory_store::SourcePolicyExclusionRecord>,
}

struct IndexFreshnessPlan {
    plan: codestory_contracts::workspace::RefreshPlan,
    current_policy_exclusions: Vec<codestory_workspace::OversizedSourceExclusionCandidate>,
}

struct IndexFreshnessChanges {
    changed_file_count: u32,
    new_file_count: u32,
    removed_file_count: u32,
    samples: Vec<IndexFreshnessSampleDto>,
}

struct IndexFreshnessIdentityAccounting {
    admitted_identities: HashSet<WorkspacePathIdentity>,
    stale_identities: HashSet<WorkspacePathIdentity>,
    gap_count: usize,
    gap_sample: Option<String>,
}

fn validate_index_freshness_publication(
    storage: &Storage,
    root: &Path,
    policy: &SourceIndexPolicy,
) -> Result<(), IndexFreshnessFenceFailure> {
    match storage.has_incomplete_incremental_run() {
        Ok(true) => return Err(IndexFreshnessFenceFailure::IncompleteRun),
        Ok(false) => {}
        Err(error) => {
            return Err(IndexFreshnessFenceFailure::Unavailable(format!(
                "failed to inspect incomplete index marker: {error}"
            )));
        }
    }

    let publication = storage.get_complete_index_publication().map_err(|error| {
        IndexFreshnessFenceFailure::Unavailable(format!(
            "failed to read complete core publication: {error}"
        ))
    })?;
    let Some(publication) = publication else {
        return Ok(());
    };
    validate_structural_text_units(storage, &publication).map_err(|error| {
        IndexFreshnessFenceFailure::Unavailable(format!(
            "structural text unit publication is incomplete: {}",
            error.message
        ))
    })?;
    validate_source_policy_exclusions(storage, root, &publication, policy).map_err(|error| {
        IndexFreshnessFenceFailure::Unavailable(format!(
            "source policy exclusion publication is incomplete: {}",
            error.message
        ))
    })
}

fn load_index_freshness_inventory(
    storage: &Storage,
) -> Result<IndexFreshnessInventory, (String, u32)> {
    let files = storage
        .get_files()
        .map_err(|error| (format!("failed to read indexed file inventory: {error}"), 0))?;
    let indexed_file_count = clamp_usize_to_u32(files.len());
    if files.is_empty() {
        return Err((
            "no indexed file inventory is available yet".to_string(),
            indexed_file_count,
        ));
    }
    if files.len() > INDEX_FRESHNESS_INDEXED_FILE_CAP {
        return Err((
            format!(
                "indexed file inventory exceeds bounded freshness cap ({} > {})",
                files.len(),
                INDEX_FRESHNESS_INDEXED_FILE_CAP
            ),
            indexed_file_count,
        ));
    }

    let stored_files = storage.files().inventory().map_err(|error| {
        (
            format!("failed to read refresh inventory: {error}"),
            indexed_file_count,
        )
    })?;
    let removed_paths = files
        .iter()
        .map(|file| (file.id, file.path.clone()))
        .collect::<HashMap<_, _>>();
    let stored_policy_exclusions = storage.get_source_policy_exclusions().map_err(|error| {
        (
            format!("failed to read source policy exclusions: {error}"),
            indexed_file_count,
        )
    })?;
    let refresh_inputs = RefreshInputs {
        stored_files,
        policy_exclusions: stored_policy_exclusions
            .iter()
            .map(source_policy_exclusion_candidate)
            .collect(),
        inventory: Default::default(),
    };

    Ok(IndexFreshnessInventory {
        indexed_file_count,
        removed_paths,
        refresh_inputs,
        stored_policy_exclusions,
    })
}

fn plan_index_freshness(
    workspace: &WorkspaceManifest,
    inventory: &IndexFreshnessInventory,
    policy: &SourceIndexPolicy,
) -> Result<IndexFreshnessPlan, String> {
    let refresh = workspace
        .build_execution_outcome_bounded_with_policy(
            &inventory.refresh_inputs,
            INDEX_FRESHNESS_CURRENT_FILE_CAP,
            policy,
        )
        .map_err(|error| format!("failed to check workspace inventory: {error}"))?;
    if refresh.refresh.inventory_outcome != WorkspaceInventoryOutcome::Complete {
        let detail = refresh
            .refresh
            .inventory_issues
            .first()
            .map(|issue| format!("{}: {}", issue.path.display(), issue.message));
        return Err(match detail {
            Some(detail) => format!(
                "current workspace inventory is {:?}: {detail}",
                refresh.refresh.inventory_outcome
            ),
            None => format!(
                "current workspace inventory is {:?} (>{})",
                refresh.refresh.inventory_outcome, INDEX_FRESHNESS_CURRENT_FILE_CAP
            ),
        });
    }

    Ok(IndexFreshnessPlan {
        plan: refresh.refresh.plan,
        current_policy_exclusions: refresh.policy_exclusions,
    })
}

fn classify_index_freshness_changes(
    root: &Path,
    inventory: &IndexFreshnessInventory,
    planned: &IndexFreshnessPlan,
) -> IndexFreshnessChanges {
    let mut changed_file_count = 0u32;
    let mut new_file_count = 0u32;
    let mut samples = Vec::new();
    for path in &planned.plan.files_to_index {
        let existing_indexed_file = planned.plan.existing_file_ids.contains_key(path);
        if !existing_indexed_file && !indexable_source_path_in_workspace(root, path) {
            continue;
        }
        let kind = if existing_indexed_file {
            changed_file_count = changed_file_count.saturating_add(1);
            IndexFreshnessChangeKindDto::Changed
        } else {
            new_file_count = new_file_count.saturating_add(1);
            IndexFreshnessChangeKindDto::New
        };
        if samples.len() < INDEX_FRESHNESS_SAMPLE_LIMIT {
            samples.push(IndexFreshnessSampleDto {
                kind,
                path: runtime_relative_path(root, path),
            });
        }
    }

    let previous_policy_by_path = inventory
        .stored_policy_exclusions
        .iter()
        .map(|entry| (entry.normalized_path.as_str(), entry))
        .collect::<HashMap<_, _>>();
    let current_policy_paths = planned
        .current_policy_exclusions
        .iter()
        .map(|entry| entry.normalized_path.as_str())
        .collect::<HashSet<_>>();
    let planned_paths = planned
        .plan
        .files_to_index
        .iter()
        .map(|path| runtime_relative_path(root, path))
        .collect::<HashSet<_>>();
    for exclusion in &planned.current_policy_exclusions {
        let kind = match previous_policy_by_path.get(exclusion.normalized_path.as_str()) {
            Some(previous)
                if previous.content_hash == exclusion.content_hash
                    && previous.observed_size == exclusion.observed_size
                    && previous.observed_unit_count == exclusion.observed_unit_count
                    && previous.policy_version == exclusion.policy_version
                    && previous.byte_cap == exclusion.byte_cap
                    && previous.structural_unit_cap == exclusion.structural_unit_cap =>
            {
                continue;
            }
            Some(_) => {
                changed_file_count = changed_file_count.saturating_add(1);
                IndexFreshnessChangeKindDto::Changed
            }
            None => {
                new_file_count = new_file_count.saturating_add(1);
                IndexFreshnessChangeKindDto::New
            }
        };
        if samples.len() < INDEX_FRESHNESS_SAMPLE_LIMIT {
            samples.push(IndexFreshnessSampleDto {
                kind,
                path: exclusion.normalized_path.clone(),
            });
        }
    }

    let removed_policy_exclusions = inventory
        .stored_policy_exclusions
        .iter()
        .filter(|entry| {
            !current_policy_paths.contains(entry.normalized_path.as_str())
                && !planned_paths.contains(&entry.normalized_path)
        })
        .collect::<Vec<_>>();
    let removed_file_count = clamp_usize_to_u32(
        planned
            .plan
            .files_to_remove
            .len()
            .saturating_add(removed_policy_exclusions.len()),
    );
    for removed_id in &planned.plan.files_to_remove {
        if samples.len() >= INDEX_FRESHNESS_SAMPLE_LIMIT {
            break;
        }
        if let Some(path) = inventory.removed_paths.get(removed_id) {
            samples.push(IndexFreshnessSampleDto {
                kind: IndexFreshnessChangeKindDto::Removed,
                path: runtime_relative_path(root, path),
            });
        }
    }
    for removed in removed_policy_exclusions {
        if samples.len() >= INDEX_FRESHNESS_SAMPLE_LIMIT {
            break;
        }
        samples.push(IndexFreshnessSampleDto {
            kind: IndexFreshnessChangeKindDto::Removed,
            path: removed.normalized_path.clone(),
        });
    }

    IndexFreshnessChanges {
        changed_file_count,
        new_file_count,
        removed_file_count,
        samples,
    }
}

fn account_index_freshness_identities<R>(
    planned: &IndexFreshnessPlan,
    removed_paths: &HashMap<i64, PathBuf>,
    identities: &mut AffectedOperationIdentityIndex<R>,
) -> IndexFreshnessIdentityAccounting
where
    R: FnMut(&Path) -> io::Result<WorkspacePathIdentity>,
{
    for path in planned
        .plan
        .existing_file_ids
        .keys()
        .chain(planned.plan.files_to_index.iter())
    {
        identities.record_admitted(path);
    }
    for path in &planned.plan.files_to_index {
        identities.record_stale(path);
    }
    for removed_id in &planned.plan.files_to_remove {
        if let Some(path) = removed_paths.get(removed_id) {
            identities.record_stale(path);
        }
    }

    IndexFreshnessIdentityAccounting {
        admitted_identities: identities.admitted_identities.clone(),
        stale_identities: identities.stale_identities.clone(),
        gap_count: identities.freshness_identity_gap_count(),
        gap_sample: identities.freshness_identity_gap_sample(),
    }
}

pub(super) fn index_freshness_observation_from_storage_with_identities<R>(
    root: &Path,
    workspace: &WorkspaceManifest,
    storage: &Storage,
    policy: &SourceIndexPolicy,
    identities: &mut AffectedOperationIdentityIndex<R>,
) -> IndexFreshnessObservation
where
    R: FnMut(&Path) -> io::Result<WorkspacePathIdentity>,
{
    let started_at = Instant::now();
    if let Err(error) = validate_index_freshness_publication(storage, root, policy) {
        return match error {
            IndexFreshnessFenceFailure::IncompleteRun => {
                IndexFreshnessObservation::incomplete(IndexFreshnessDto {
                    status: IndexFreshnessStatusDto::Stale,
                    changed_file_count: 0,
                    new_file_count: 0,
                    removed_file_count: 0,
                    checked_file_count: 0,
                    indexed_file_count: 0,
                    duration_ms: clamp_u128_to_u32(started_at.elapsed().as_millis()),
                    reason: Some(
                        "previous_incremental_run_incomplete_full_refresh_required".to_string(),
                    ),
                    samples: Vec::new(),
                })
            }
            IndexFreshnessFenceFailure::Unavailable(reason) => {
                IndexFreshnessObservation::incomplete(not_checked_index_freshness(
                    reason, 0, started_at,
                ))
            }
        };
    }
    #[cfg(test)]
    run_after_index_freshness_fence_test_hook();

    let inventory = match load_index_freshness_inventory(storage) {
        Ok(inventory) => inventory,
        Err((reason, indexed_file_count)) => {
            return IndexFreshnessObservation::incomplete(not_checked_index_freshness(
                reason,
                indexed_file_count,
                started_at,
            ));
        }
    };
    let planned = match plan_index_freshness(workspace, &inventory, policy) {
        Ok(planned) => planned,
        Err(reason) => {
            return IndexFreshnessObservation::incomplete(not_checked_index_freshness(
                reason,
                inventory.indexed_file_count,
                started_at,
            ));
        }
    };
    let changes = classify_index_freshness_changes(root, &inventory, &planned);
    let identity =
        account_index_freshness_identities(&planned, &inventory.removed_paths, identities);
    let status = if changes.changed_file_count == 0
        && changes.new_file_count == 0
        && changes.removed_file_count == 0
    {
        IndexFreshnessStatusDto::Fresh
    } else {
        IndexFreshnessStatusDto::Stale
    };
    let checked_file_count = inventory
        .indexed_file_count
        .saturating_sub(changes.removed_file_count)
        .saturating_add(changes.new_file_count);

    IndexFreshnessObservation {
        freshness: IndexFreshnessDto {
            status,
            changed_file_count: changes.changed_file_count,
            new_file_count: changes.new_file_count,
            removed_file_count: changes.removed_file_count,
            checked_file_count,
            indexed_file_count: inventory.indexed_file_count,
            duration_ms: clamp_u128_to_u32(started_at.elapsed().as_millis()),
            reason: None,
            samples: changes.samples,
        },
        inventory_complete: identity.gap_count == 0,
        admitted_identities: identity.admitted_identities,
        stale_identities: identity.stale_identities,
        identity_gap_count: identity.gap_count,
        identity_gap_sample: identity.gap_sample,
    }
}

pub(super) fn index_freshness_from_storage_with_policy(
    root: &Path,
    workspace: &WorkspaceManifest,
    storage: &Storage,
    policy: &SourceIndexPolicy,
) -> IndexFreshnessDto {
    index_freshness_observation_from_storage(root, workspace, storage, policy).freshness
}

#[cfg(test)]
pub(super) fn index_freshness_from_storage(
    root: &Path,
    workspace: &WorkspaceManifest,
    storage: &Storage,
) -> IndexFreshnessDto {
    index_freshness_from_storage_with_policy(
        root,
        workspace,
        storage,
        &SourceIndexPolicy::default(),
    )
}

pub(super) fn workspace_member_index_summaries(
    root: &Path,
    workspace: &WorkspaceManifest,
    refresh_inputs: &RefreshInputs,
    execution_plan: &RefreshExecutionPlan,
) -> Vec<WorkspaceMemberIndexDto> {
    workspace
        .members()
        .iter()
        .map(|member| {
            let absolute = if member.is_absolute() {
                member.clone()
            } else {
                root.join(member)
            };
            let files_to_index = execution_plan
                .files_to_index
                .iter()
                .filter(|path| path.starts_with(&absolute))
                .count()
                .min(u32::MAX as usize) as u32;
            let indexed_files = refresh_inputs
                .stored_files
                .iter()
                .filter(|file| file.path.starts_with(&absolute))
                .count()
                .min(u32::MAX as usize) as u32;
            WorkspaceMemberIndexDto {
                path: runtime_relative_path(root, &absolute),
                files_to_index,
                indexed_files,
                file_count: None,
                node_count: None,
                edge_count: None,
            }
        })
        .collect()
}

pub(super) fn workspace_member_storage_summaries(
    root: &Path,
    workspace: &WorkspaceManifest,
    storage: &Storage,
) -> Result<Vec<WorkspaceMemberIndexDto>, ApiError> {
    if workspace.members().is_empty() {
        return Ok(Vec::new());
    }
    let files = storage
        .get_files()
        .map_err(|e| ApiError::internal(format!("Failed to query member files: {e}")))?;
    let nodes = storage
        .get_nodes()
        .map_err(|e| ApiError::internal(format!("Failed to query member nodes: {e}")))?;
    let edges = storage
        .get_edges()
        .map_err(|e| ApiError::internal(format!("Failed to query member edges: {e}")))?;

    let node_file_ids = nodes
        .iter()
        .map(|node| (node.id, node.file_node_id))
        .collect::<HashMap<_, _>>();

    Ok(workspace
        .members()
        .iter()
        .map(|member| {
            let absolute = if member.is_absolute() {
                member.clone()
            } else {
                root.join(member)
            };
            let file_ids = files
                .iter()
                .filter(|file| file.path.starts_with(&absolute))
                .map(|file| codestory_contracts::graph::NodeId(file.id))
                .collect::<HashSet<_>>();
            let file_count = file_ids.len().min(u32::MAX as usize) as u32;
            let node_count = nodes
                .iter()
                .filter(|node| {
                    file_ids.contains(&node.id)
                        || node
                            .file_node_id
                            .is_some_and(|file_id| file_ids.contains(&file_id))
                })
                .count()
                .min(u32::MAX as usize) as u32;
            let edge_count = edges
                .iter()
                .filter(|edge| {
                    edge.file_node_id
                        .is_some_and(|file_id| file_ids.contains(&file_id))
                        || node_file_ids
                            .get(&edge.effective_source())
                            .and_then(|file_id| *file_id)
                            .is_some_and(|file_id| file_ids.contains(&file_id))
                })
                .count()
                .min(u32::MAX as usize) as u32;
            WorkspaceMemberIndexDto {
                path: runtime_relative_path(root, &absolute),
                files_to_index: 0,
                indexed_files: file_count,
                file_count: Some(file_count),
                node_count: Some(node_count),
                edge_count: Some(edge_count),
            }
        })
        .collect())
}

#[derive(Debug, Clone)]
pub(super) struct CachedIndexFreshness {
    pub(super) root: PathBuf,
    pub(super) storage_path: PathBuf,
    pub(super) storage_fingerprint: String,
    pub(super) value: IndexFreshnessDto,
    pub(super) cached_at: Instant,
}

pub(super) fn index_freshness_cache_ttl_secs() -> u64 {
    std::env::var("CODESTORY_INDEX_FRESHNESS_TTL_SECS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|ttl| *ttl > 0)
        .unwrap_or(INDEX_FRESHNESS_CACHE_DEFAULT_TTL_SECS)
}

pub(super) fn storage_fingerprint(path: &Path) -> String {
    [
        storage_path_fingerprint(path),
        storage_path_fingerprint(&path.with_extension("db-wal")),
        storage_path_fingerprint(&path.with_extension("db-shm")),
    ]
    .join("|")
}

fn storage_path_fingerprint(path: &Path) -> String {
    let Ok(metadata) = std::fs::metadata(path) else {
        return "missing".to_string();
    };
    let modified_ms = metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis())
        .unwrap_or_default();
    format!("len:{}:mtime_ms:{modified_ms}", metadata.len())
}

pub(super) fn open_storage_for_read(path: &Path) -> Result<Storage, ApiError> {
    let requires_initialization = !path.exists()
        || Storage::database_schema_version(path)
            .map(|version| version != CURRENT_SCHEMA_VERSION)
            .map_err(|error| {
                ApiError::internal(format!("Failed to inspect storage schema: {error}"))
            })?;
    let storage = if requires_initialization {
        Storage::open(path)
    } else {
        Storage::open_read_only(path)
    };
    storage.map_err(|error| ApiError::internal(format!("Failed to open storage: {error}")))
}

pub(super) fn open_existing_storage_for_read(path: &Path) -> Result<Storage, ApiError> {
    if !path.is_file() {
        return Err(ApiError::new(
            "project_unavailable",
            "no complete project storage is available",
        ));
    }
    let schema = Storage::database_schema_version(path).map_err(|error| {
        ApiError::internal(format!("Failed to inspect storage schema: {error}"))
    })?;
    if schema != CURRENT_SCHEMA_VERSION {
        return Err(ApiError::new(
            "project_unavailable",
            format!(
                "project storage schema {schema} is not readable by runtime schema {CURRENT_SCHEMA_VERSION}"
            ),
        ));
    }
    Storage::open_read_only(path)
        .map_err(|error| ApiError::internal(format!("Failed to open storage: {error}")))
}
