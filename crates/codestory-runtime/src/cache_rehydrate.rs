use anyhow::{Context, Result, bail};
use codestory_store::{
    CURRENT_SCHEMA_VERSION, CompactRehydratePeakSpace, CorePublicationLayout,
    RehydratedCacheRebaseStats, SqliteVacuumIntoStats, Store, ensure_compact_rehydrate_peak_space,
    measure_compact_rehydrate_peak_space, publish_rehydrated_generation, remove_staging_database,
    vacuum_into_database,
};
use codestory_workspace::{
    RefreshInputs, SourceIndexPolicy, WorkspaceInventory, WorkspaceInventoryOutcome,
    WorkspaceManifest, read_repository_metadata,
};
use serde::Serialize;
use std::fs;
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone)]
/// Request to copy a compatible CodeStory cache between sibling worktrees.
///
/// Source and target must share git remote, tree, schema, and freshness. Retrieval manifests are
/// path- and sidecar-bound, so successful rehydrate invalidates them before the target can serve
/// sidecar retrieval.
pub struct CacheRehydrateRequest<'a> {
    pub source_project: &'a Path,
    pub source_cache_dir: &'a Path,
    pub target_project: &'a Path,
    pub target_cache_dir: &'a Path,
    pub dry_run: bool,
}

#[derive(Debug, Clone, Serialize)]
/// Machine-readable result of a cache rehydrate attempt.
///
/// `preserved_scope` and `retrieval` explain the contract boundary: only core graph/file inventory
/// and verified policy exclusions cross the worktree boundary. Derived semantic state is discarded
/// and a full refresh is required before the target can publish.
pub struct CacheRehydrateOutput {
    pub status: String,
    pub reason: Option<String>,
    pub source_project: String,
    pub target_project: String,
    pub source_cache_dir: String,
    pub target_cache_dir: String,
    pub source_remote: Option<String>,
    pub target_remote: Option<String>,
    pub source_tree: Option<String>,
    pub target_tree: Option<String>,
    pub schema_version: Option<u32>,
    pub source_file_count: Option<i64>,
    pub copied: bool,
    pub dry_run: bool,
    pub invalidated_retrieval_manifests: usize,
    pub invalidated_index_artifact_rows: usize,
    pub invalidated_semantic_rows: usize,
    pub rebased_path_bound_rows: usize,
    pub carried_policy_exclusion_rows: usize,
    pub preserved_scope: String,
    pub retrieval_status: String,
    pub retrieval_reason: String,
    pub retrieval_next_command: Option<String>,
    pub retrieval: String,
    pub next_commands: Vec<String>,
    pub peak_space_required_bytes: Option<u64>,
    pub available_bytes: Option<u64>,
    pub source_logical_bytes: Option<u64>,
    pub source_file_bytes: Option<u64>,
    pub source_freelist_count: Option<u64>,
    pub candidate_logical_bytes: Option<u64>,
    pub candidate_file_bytes: Option<u64>,
    pub candidate_freelist_count: Option<u64>,
    pub freelist_pages_reclaimed: Option<u64>,
}

/// Copy a compatible cache, rebase path-bound rows, and invalidate copied retrieval manifests.
///
/// Skipped results are intentional safety outcomes, not hard failures. They preserve correctness
/// when cache identity, freshness, or directory boundaries are not strong enough.
pub fn rehydrate_cache(request: CacheRehydrateRequest<'_>) -> Result<CacheRehydrateOutput> {
    let logical_source = request.source_cache_dir.join("codestory.db");
    let logical_target = request.target_cache_dir.join("codestory.db");
    let source_layout = CorePublicationLayout::from_storage_path(&logical_source)
        .with_context(|| format!("resolve source cache layout {}", logical_source.display()))?;
    let target_layout = CorePublicationLayout::from_storage_path(&logical_target)
        .with_context(|| format!("resolve target cache layout {}", logical_target.display()))?;
    let source_db = source_layout
        .resolve_active_database()
        .with_context(|| format!("resolve source core database {}", logical_source.display()))?;
    let rebuild = rebuild_commands(request.target_project);

    if request.source_cache_dir == request.target_cache_dir {
        return Ok(skipped(
            request,
            "source and target cache dirs are identical",
            rebuild,
        ));
    }
    if source_db.is_none() {
        return Ok(skipped(
            request,
            "source cache has no published core database",
            rebuild,
        ));
    }
    let source_db = source_db.expect("source database just checked");
    if target_cache_nested_in_source(request.source_cache_dir, request.target_cache_dir)? {
        return Ok(skipped(
            request,
            "target cache dir is inside source cache dir",
            rebuild,
        ));
    }
    let _source_writer_guard = if request.dry_run {
        None
    } else {
        Some(
            match super::IndexWriterGuard::try_acquire(&logical_source) {
                Ok(guard) => guard,
                Err(error) if error.code == "cache_busy" => {
                    return Ok(skipped(
                        request,
                        format!("source cache is busy: {}", error.message),
                        rebuild,
                    ));
                }
                Err(error) => bail!(
                    "failed to acquire source cache writer lock: {}",
                    error.message
                ),
            },
        )
    };
    let _target_writer_guard = if request.dry_run {
        None
    } else {
        Some(
            match super::IndexWriterGuard::try_acquire(&logical_target) {
                Ok(guard) => guard,
                Err(error) if error.code == "cache_busy" => {
                    return Ok(skipped(
                        request,
                        format!("target cache is busy: {}", error.message),
                        rebuild,
                    ));
                }
                Err(error) => bail!(
                    "failed to acquire target cache writer lock: {}",
                    error.message
                ),
            },
        )
    };
    if target_cache_has_contents(request.target_cache_dir)? {
        return Ok(skipped(request, "target cache dir is not empty", rebuild));
    }

    let source_git = match git_identity(request.source_project) {
        Ok(identity) => identity,
        Err(error) => return Ok(skipped(request, error.to_string(), rebuild)),
    };
    let target_git = match git_identity(request.target_project) {
        Ok(identity) => identity,
        Err(error) => return Ok(skipped(request, error.to_string(), rebuild)),
    };
    if source_git.remote != target_git.remote {
        return Ok(skipped_with_git(
            request,
            "git remote mismatch",
            source_git,
            target_git,
            rebuild,
        ));
    }
    if source_git.tree != target_git.tree {
        return Ok(skipped_with_git(
            request,
            "git tree mismatch",
            source_git,
            target_git,
            rebuild,
        ));
    }

    let schema_version = Store::database_schema_version(&logical_source)
        .with_context(|| format!("read source cache schema {}", logical_source.display()))?;
    if schema_version != CURRENT_SCHEMA_VERSION {
        return Ok(skipped_with_git_schema(
            request,
            format!(
                "cache schema mismatch: source={schema_version} current={CURRENT_SCHEMA_VERSION}"
            ),
            source_git,
            target_git,
            Some(schema_version),
            None,
            rebuild,
        ));
    }

    let source_file_count = {
        let storage = Store::open(&logical_source).context("open source cache for stats")?;
        storage.get_stats()?.file_count
    };
    if source_file_count == 0 {
        return Ok(skipped_with_git_schema(
            request,
            "source cache has no indexed files",
            source_git,
            target_git,
            Some(schema_version),
            Some(source_file_count),
            rebuild,
        ));
    }

    let source_freshness = match source_cache_freshness(request.source_project, &logical_source) {
        Ok(freshness) => freshness,
        Err(error) => {
            return Ok(skipped_with_git_schema(
                request,
                format!("source cache freshness check failed: {error}"),
                source_git,
                target_git,
                Some(schema_version),
                Some(source_file_count),
                rebuild,
            ));
        }
    };
    if source_freshness.changed_or_new_files > 0 || source_freshness.removed_files > 0 {
        return Ok(skipped_with_git_schema(
            request,
            format!(
                "source cache is stale: changed_or_new_files={} removed_files={}",
                source_freshness.changed_or_new_files, source_freshness.removed_files
            ),
            source_git,
            target_git,
            Some(schema_version),
            Some(source_file_count),
            rebuild,
        ));
    }

    let mut invalidated_retrieval_manifests = 0;
    let mut rebase_stats = RehydratedCacheRebaseStats::default();
    let mut vacuum_stats = None;
    let destination_parent = existing_filesystem_parent(request.target_cache_dir);
    let peak_space = measure_compact_rehydrate_peak_space(&source_db, destination_parent)
        .context("measure compact rehydrate peak space")?;
    if !request.dry_run {
        if peak_space.available_bytes < peak_space.peak_space_required_bytes {
            return Ok(insufficient_space_output(
                request,
                peak_space,
                source_git,
                target_git,
                Some(schema_version),
                Some(source_file_count),
                rebuild,
            ));
        }
        let published = publish_rehydrated_database(
            &source_db,
            &target_layout,
            &logical_target,
            request.source_project,
            request.target_project,
        )?;
        invalidated_retrieval_manifests = published.invalidated_retrieval_manifests;
        rebase_stats = published.rebase_stats;
        vacuum_stats = Some(published.vacuum_stats);
    }

    Ok(rehydrate_success_output(
        request,
        source_git,
        target_git,
        schema_version,
        source_file_count,
        invalidated_retrieval_manifests,
        rebase_stats,
        Some(peak_space),
        vacuum_stats,
    ))
}

#[derive(Debug, Clone)]
struct GitIdentity {
    remote: String,
    tree: String,
}

#[derive(Debug, Clone)]
struct SourceCacheFreshness {
    changed_or_new_files: usize,
    removed_files: usize,
}

fn source_cache_freshness(project: &Path, source_db: &Path) -> Result<SourceCacheFreshness> {
    let workspace =
        WorkspaceManifest::open_with_storage_owned_exclusions(project.to_path_buf(), source_db)
            .with_context(|| format!("open source workspace {}", project.display()))?;
    let storage = Store::open(source_db).context("open source cache for freshness")?;
    if storage
        .has_incomplete_incremental_run()
        .context("inspect source cache incomplete index marker")?
    {
        bail!("source cache has an incomplete incremental index run");
    }
    let policy = SourceIndexPolicy::default();
    let stored_policy_exclusions = storage
        .get_source_policy_exclusions()
        .context("read source cache policy exclusions")?;
    let refresh = workspace
        .build_execution_outcome_with_policy(
            &RefreshInputs {
                stored_files: storage.files().inventory()?,
                policy_exclusions: stored_policy_exclusions
                    .iter()
                    .map(super::source_policy_exclusion_candidate)
                    .collect(),
                inventory: WorkspaceInventory::default(),
            },
            &policy,
        )
        .context("build source cache refresh plan")?;
    if refresh.refresh.inventory_outcome != WorkspaceInventoryOutcome::Complete {
        bail!(
            "source workspace inventory is {:?}; cache freshness cannot be proven",
            refresh.refresh.inventory_outcome
        );
    }
    let plan = refresh.refresh.plan;
    Ok(SourceCacheFreshness {
        changed_or_new_files: plan.files_to_index.len(),
        removed_files: plan.files_to_remove.len(),
    })
}

fn git_identity(project: &Path) -> Result<GitIdentity> {
    let metadata = read_repository_metadata(project);
    if let Some(issue) = metadata.issues.first() {
        bail!(
            "git metadata inspection failed for {}: {}: {}",
            project.display(),
            issue.code,
            issue.message
        );
    }
    if metadata.dirty {
        bail!("git worktree is dirty: {}", project.display());
    }
    let remote = metadata
        .remote_url
        .filter(|remote| !remote.trim().is_empty())
        .with_context(|| format!("git remote origin is missing: {}", project.display()))?;
    let tree = metadata
        .head_tree
        .with_context(|| format!("git HEAD tree is missing: {}", project.display()))?;
    Ok(GitIdentity { remote, tree })
}

fn target_cache_has_contents(path: &Path) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    for entry in
        fs::read_dir(path).with_context(|| format!("read target cache dir {}", path.display()))?
    {
        let writer_lock = Path::new("codestory.db")
            .with_extension(codestory_contracts::owned_artifacts::INDEX_WRITER_LOCK_EXTENSION);
        if entry?.file_name() != writer_lock.as_os_str() {
            return Ok(true);
        }
    }
    Ok(false)
}

fn target_cache_nested_in_source(source: &Path, target: &Path) -> Result<bool> {
    let source = source
        .canonicalize()
        .with_context(|| format!("canonicalize source cache dir {}", source.display()))?;
    let target = normalize_cache_target_path(target)
        .with_context(|| format!("normalize target cache dir {}", target.display()))?;
    Ok(target.starts_with(&source) && target != source)
}

fn normalize_cache_target_path(path: &Path) -> Result<PathBuf> {
    if path.exists() {
        return path
            .canonicalize()
            .with_context(|| format!("canonicalize {}", path.display()));
    }

    let mut missing = Vec::new();
    let mut current = path;
    while !current.exists() {
        let Some(name) = current.file_name() else {
            break;
        };
        missing.push(name.to_os_string());
        let Some(parent) = current.parent() else {
            break;
        };
        current = parent;
    }

    let mut normalized = if current.exists() {
        current
            .canonicalize()
            .with_context(|| format!("canonicalize {}", current.display()))?
    } else {
        absolutize_lexical_path(current)?
    };
    for component in missing.iter().rev() {
        normalized.push(component);
    }
    Ok(normalized)
}

fn absolutize_lexical_path(path: &Path) -> Result<PathBuf> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .context("read current dir for path normalization")?
            .join(path)
    };
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    Ok(normalized)
}

fn publish_rehydrated_database(
    source_db: &Path,
    target_layout: &CorePublicationLayout,
    logical_target: &Path,
    source_project: &Path,
    target_project: &Path,
) -> Result<PublishedRehydrate> {
    // Fail closed before allocating the stage copy so insufficient_space never
    // mutates the target cache. Measure the same file the snapshot copy reads.
    let destination_parent = existing_filesystem_parent(logical_target);
    ensure_compact_rehydrate_peak_space(source_db, destination_parent)
        .context("preflight compact rehydrate peak space before stage copy")?;

    let stage_path = target_layout
        .create_staging_database_path()
        .context("create rehydrate stage under the target core layout")?;
    let mut candidate_path = None;
    let result = (|| {
        Store::copy_database_snapshot(source_db, &stage_path)
            .context("copy source database into rehydrate stage")?;
        let (invalidated_retrieval_manifests, rebase_stats) = {
            let mut storage = Store::open(&stage_path).context("open rehydrate stage")?;
            let invalidated_retrieval_manifests = storage
                .clear_retrieval_index_manifests()
                .context("invalidate copied retrieval manifests")?;
            let rebase_stats = storage
                .rebase_rehydrated_path_bound_cache(source_project, target_project)
                .context("rebase and invalidate copied cache rows")?;
            (invalidated_retrieval_manifests, rebase_stats)
        };

        let compacted = target_layout
            .create_staging_database_path()
            .context("create compact rehydrate candidate under the target core layout")?;
        candidate_path = Some(compacted.clone());
        let vacuum_stats = vacuum_into_database(&stage_path, &compacted)
            .context("compact rehydrate stage with VACUUM INTO")?;
        remove_staging_database(&stage_path).context("remove rehydrate stage")?;
        validate_rehydrated_database(&compacted).context("validate rehydrate publish candidate")?;
        fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&compacted)
            .and_then(|file| file.sync_all())
            .with_context(|| format!("sync rehydrate candidate {}", compacted.display()))?;
        publish_rehydrated_generation(&compacted, logical_target)
            .context("publish rehydrated generation and swap the publication pointer")?;
        Ok(PublishedRehydrate {
            invalidated_retrieval_manifests,
            rebase_stats,
            vacuum_stats,
        })
    })();

    if result.is_err() {
        let _ = remove_staging_database(&stage_path);
        if let Some(path) = candidate_path.as_deref() {
            let _ = remove_staging_database(path);
        }
    }
    result
}

fn existing_filesystem_parent(path: &Path) -> &Path {
    path.ancestors()
        .find(|ancestor| ancestor.is_dir())
        .unwrap_or_else(|| Path::new("."))
}

struct PublishedRehydrate {
    invalidated_retrieval_manifests: usize,
    rebase_stats: RehydratedCacheRebaseStats,
    vacuum_stats: SqliteVacuumIntoStats,
}

fn validate_rehydrated_database(path: &Path) -> Result<()> {
    let storage = Store::open_observational(path).context("open candidate observationally")?;
    let conn = storage.get_connection();
    for table in [
        "index_publication",
        "retrieval_index_manifest",
        "index_artifact_cache",
        "llm_symbol_doc",
        "symbol_search_doc",
        "symbol_summary",
        "dense_anchor_input",
        "dense_anchor_publication",
        "structural_text_unit_publication",
        "structural_text_unit",
        "structural_text_projection",
        "structural_text_artifact_cache",
        "source_policy_exclusion_publication",
        "grounding_repo_stats_snapshot",
        "grounding_file_snapshot",
        "grounding_node_snapshot",
        "grounding_node_summary_snapshot",
        "grounding_node_edge_digest_snapshot",
    ] {
        let count: i64 = conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
            row.get(0)
        })?;
        if count != 0 {
            bail!("rehydrate candidate retained {count} rows in {table}");
        }
    }
    let component_report_nodes: i64 = conn.query_row(
        "SELECT COUNT(*) FROM node
         WHERE serialized_name LIKE 'component_report:%'
            OR canonical_id LIKE 'codestory:component_report:%'",
        [],
        |row| row.get(0),
    )?;
    if component_report_nodes != 0 {
        bail!("rehydrate candidate retained {component_report_nodes} component-report nodes");
    }
    let component_report_projections: i64 = conn.query_row(
        "SELECT COUNT(*)
         FROM search_symbol_projection AS projection
         LEFT JOIN node ON node.id = projection.node_id
         WHERE projection.display_name LIKE 'component_report:%'
            OR projection.display_name LIKE 'codestory::component_report::%'
            OR node.id IS NULL",
        [],
        |row| row.get(0),
    )?;
    if component_report_projections != 0 {
        bail!(
            "rehydrate candidate retained {component_report_projections} component-report search projections"
        );
    }
    let grounding_state: (i64, i64, Option<i64>, Option<i64>) = conn.query_row(
        "SELECT summary_state, detail_state, summary_built_at_epoch_ms, detail_built_at_epoch_ms
         FROM grounding_snapshot_meta WHERE id = 1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    )?;
    if grounding_state != (0, 0, None, None) {
        bail!("rehydrate candidate retained ready grounding snapshot state");
    }
    let resolution_state: (i64, Option<Vec<u8>>, Option<i64>) = conn.query_row(
        "SELECT state, snapshot_blob, built_at_epoch_ms
         FROM resolution_support_snapshot WHERE id = 1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    if resolution_state != (0, None, None) {
        bail!("rehydrate candidate retained ready resolution support state");
    }
    Ok(())
}

fn skipped(
    request: CacheRehydrateRequest<'_>,
    reason: impl Into<String>,
    next_commands: Vec<String>,
) -> CacheRehydrateOutput {
    CacheRehydrateOutput {
        status: "skipped".into(),
        reason: Some(reason.into()),
        source_project: display_path(request.source_project),
        target_project: display_path(request.target_project),
        source_cache_dir: display_path(request.source_cache_dir),
        target_cache_dir: display_path(request.target_cache_dir),
        source_remote: None,
        target_remote: None,
        source_tree: None,
        target_tree: None,
        schema_version: None,
        source_file_count: None,
        copied: false,
        dry_run: request.dry_run,
        invalidated_retrieval_manifests: 0,
        invalidated_index_artifact_rows: 0,
        invalidated_semantic_rows: 0,
        rebased_path_bound_rows: 0,
        carried_policy_exclusion_rows: 0,
        preserved_scope: "none".into(),
        retrieval_status: "not_rehydrated".into(),
        retrieval_reason: "normal index and retrieval rebuild required".into(),
        retrieval_next_command: None,
        retrieval: "not rehydrated; normal index/retrieval rebuild required".into(),
        next_commands,
        peak_space_required_bytes: None,
        available_bytes: None,
        source_logical_bytes: None,
        source_file_bytes: None,
        source_freelist_count: None,
        candidate_logical_bytes: None,
        candidate_file_bytes: None,
        candidate_freelist_count: None,
        freelist_pages_reclaimed: None,
    }
}

fn insufficient_space_output(
    request: CacheRehydrateRequest<'_>,
    peak_space: CompactRehydratePeakSpace,
    source_git: GitIdentity,
    target_git: GitIdentity,
    schema_version: Option<u32>,
    source_file_count: Option<i64>,
    next_commands: Vec<String>,
) -> CacheRehydrateOutput {
    let mut output = skipped_with_git_schema(
        request,
        format!(
            "insufficient space for compact rehydrate: need at least {} bytes, available {} bytes",
            peak_space.peak_space_required_bytes, peak_space.available_bytes
        ),
        source_git,
        target_git,
        schema_version,
        source_file_count,
        next_commands,
    );
    output.status = "insufficient_space".into();
    output.peak_space_required_bytes = Some(peak_space.peak_space_required_bytes);
    output.available_bytes = Some(peak_space.available_bytes);
    output.source_logical_bytes = Some(peak_space.candidate_upper_bytes);
    output
}

#[allow(clippy::too_many_arguments)]
fn rehydrate_success_output(
    request: CacheRehydrateRequest<'_>,
    source_git: GitIdentity,
    target_git: GitIdentity,
    schema_version: u32,
    source_file_count: i64,
    invalidated_retrieval_manifests: usize,
    rebase_stats: RehydratedCacheRebaseStats,
    peak_space: Option<CompactRehydratePeakSpace>,
    vacuum_stats: Option<SqliteVacuumIntoStats>,
) -> CacheRehydrateOutput {
    let mut output = CacheRehydrateOutput {
        status: if request.dry_run {
            "would_rehydrate".into()
        } else {
            "rehydrated".into()
        },
        reason: None,
        source_project: display_path(request.source_project),
        target_project: display_path(request.target_project),
        source_cache_dir: display_path(request.source_cache_dir),
        target_cache_dir: display_path(request.target_cache_dir),
        source_remote: Some(source_git.remote),
        target_remote: Some(target_git.remote),
        source_tree: Some(source_git.tree),
        target_tree: Some(target_git.tree),
        schema_version: Some(schema_version),
        source_file_count: Some(source_file_count),
        copied: !request.dry_run,
        dry_run: request.dry_run,
        invalidated_retrieval_manifests,
        invalidated_index_artifact_rows: rebase_stats.invalidated_index_artifact_rows,
        invalidated_semantic_rows: rebase_stats.invalidated_semantic_rows,
        rebased_path_bound_rows: rebase_stats.rebased_path_bound_rows,
        carried_policy_exclusion_rows: rebase_stats.carried_policy_exclusion_rows,
        preserved_scope: "core_graph_file_inventory_and_policy_exclusions_only".into(),
        retrieval_status: retrieval_rehydrate_status(request.dry_run),
        retrieval_reason: retrieval_rehydrate_reason(),
        retrieval_next_command: Some(retrieval_next_command(request.target_project)),
        retrieval: retrieval_rehydrate_policy(request.dry_run),
        next_commands: rehydrate_next_commands(request.target_project),
        peak_space_required_bytes: peak_space
            .as_ref()
            .map(|space| space.peak_space_required_bytes),
        available_bytes: peak_space.as_ref().map(|space| space.available_bytes),
        source_logical_bytes: None,
        source_file_bytes: None,
        source_freelist_count: None,
        candidate_logical_bytes: None,
        candidate_file_bytes: None,
        candidate_freelist_count: None,
        freelist_pages_reclaimed: None,
    };
    if let Some(stats) = vacuum_stats {
        output.source_logical_bytes = Some(stats.source_logical_bytes);
        output.source_file_bytes = Some(stats.source_file_bytes);
        output.source_freelist_count = Some(stats.source_freelist_count);
        output.candidate_logical_bytes = Some(stats.candidate_logical_bytes);
        output.candidate_file_bytes = Some(stats.candidate_file_bytes);
        output.candidate_freelist_count = Some(stats.candidate_freelist_count);
        output.freelist_pages_reclaimed = Some(stats.freelist_pages_reclaimed);
    } else if let Some(space) = peak_space {
        output.source_logical_bytes = Some(space.candidate_upper_bytes);
    }
    output
}

fn skipped_with_git(
    request: CacheRehydrateRequest<'_>,
    reason: impl Into<String>,
    source_git: GitIdentity,
    target_git: GitIdentity,
    next_commands: Vec<String>,
) -> CacheRehydrateOutput {
    skipped_with_git_schema(
        request,
        reason,
        source_git,
        target_git,
        None,
        None,
        next_commands,
    )
}

fn skipped_with_git_schema(
    request: CacheRehydrateRequest<'_>,
    reason: impl Into<String>,
    source_git: GitIdentity,
    target_git: GitIdentity,
    schema_version: Option<u32>,
    source_file_count: Option<i64>,
    next_commands: Vec<String>,
) -> CacheRehydrateOutput {
    let mut output = skipped(request, reason, next_commands);
    output.source_remote = Some(source_git.remote);
    output.target_remote = Some(target_git.remote);
    output.source_tree = Some(source_git.tree);
    output.target_tree = Some(target_git.tree);
    output.schema_version = schema_version;
    output.source_file_count = source_file_count;
    output
}

fn rebuild_commands(project: &Path) -> Vec<String> {
    let project = quote_path(project);
    vec![
        format!("codestory-cli index --project {project} --refresh full"),
        format!("codestory-cli retrieval index --project {project} --refresh full"),
        format!("codestory-cli doctor --project {project}"),
    ]
}

fn rehydrate_next_commands(project: &Path) -> Vec<String> {
    let project = quote_path(project);
    vec![
        format!("codestory-cli index --project {project} --refresh full"),
        format!("codestory-cli retrieval index --project {project} --refresh full"),
        format!("codestory-cli doctor --project {project}"),
    ]
}

fn retrieval_next_command(project: &Path) -> String {
    format!(
        "codestory-cli retrieval index --project {} --refresh full",
        quote_path(project)
    )
}

fn retrieval_rehydrate_status(dry_run: bool) -> String {
    if dry_run {
        "would_invalidate_requires_rebuild".into()
    } else {
        "invalidated_requires_rebuild".into()
    }
}

fn retrieval_rehydrate_reason() -> String {
    "cache rehydrate carries only core graph/file inventory and verified policy exclusions; semantic docs, dense inputs, structural identities, retrieval manifests, and index artifacts are discarded, and the target remains unpublished until a full refresh".into()
}

fn retrieval_rehydrate_policy(dry_run: bool) -> String {
    let action = if dry_run {
        "would be invalidated"
    } else {
        "invalidated"
    };
    format!(
        "derived semantic rows, structural identities, index artifacts, publications, and retrieval manifests {action}; only rebased core graph/file inventory and verified policy exclusions are carried; no sidecar directory is copied; a full core refresh must publish the target before retrieval is rebuilt"
    )
}

fn quote_path(path: &Path) -> String {
    let value = path.to_string_lossy();
    if value.contains([' ', '"', '\'']) {
        format!("{value:?}")
    } else {
        value.to_string()
    }
}

fn display_path(path: &Path) -> String {
    path.to_string_lossy().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{git, git_available, git_output};
    use codestory_contracts::graph::{Node, NodeId, NodeKind};
    use sha2::{Digest, Sha256};
    use std::path::PathBuf;
    use tempfile::tempdir;

    #[test]
    fn rehydrate_atomically_carries_only_core_inventory_and_policy_exclusions() {
        let Some((source_project, target_project)) = matching_git_projects() else {
            return;
        };
        add_matching_oversized_source(source_project.path(), target_project.path());
        let source_cache = tempdir().expect("source cache");
        let target_cache = tempdir().expect("target cache");
        let target_cache_path = target_cache.path().join("empty");
        fs::create_dir_all(&target_cache_path).expect("create lock-only target cache");
        fs::write(target_cache_path.join("codestory.index-writer.lock"), b"")
            .expect("seed persistent target lock");
        let source_db = source_cache.path().join("codestory.db");
        seed_cache(&source_db, source_project.path());
        let source_sidecar_dir = source_cache.path().join("semantic-generation");
        fs::create_dir_all(&source_sidecar_dir).expect("create source-only sidecar directory");
        fs::write(source_sidecar_dir.join("vectors.bin"), b"source-only")
            .expect("seed source-only sidecar");

        let output = rehydrate_cache(CacheRehydrateRequest {
            source_project: source_project.path(),
            source_cache_dir: source_cache.path(),
            target_project: target_project.path(),
            target_cache_dir: &target_cache_path,
            dry_run: false,
        })
        .expect("rehydrate");

        assert_eq!(output.status, "rehydrated");
        assert!(
            !target_cache_path.join("codestory.db").exists(),
            "rehydrate must publish a generation, not replace the legacy target file"
        );
        let published = resolved_core_database(&target_cache_path)
            .expect("rehydrate must install a published generation");
        assert!(published.is_file());
        assert!(
            target_cache_path
                .join("codestory.index-writer.lock")
                .exists(),
            "the target owns its persistent writer lock after rehydrate"
        );
        assert_eq!(output.invalidated_retrieval_manifests, 1);
        assert_eq!(output.invalidated_index_artifact_rows, 2);
        assert!(output.invalidated_semantic_rows > 0);
        assert!(output.rebased_path_bound_rows > 0);
        assert_eq!(output.carried_policy_exclusion_rows, 1);
        assert_eq!(
            output.preserved_scope,
            "core_graph_file_inventory_and_policy_exclusions_only"
        );
        assert_eq!(output.retrieval_status, "invalidated_requires_rebuild");
        assert!(
            output
                .retrieval_reason
                .contains("target remains unpublished until a full refresh"),
            "rehydrate output should expose the publication fence: {}",
            output.retrieval_reason
        );
        assert!(
            output
                .retrieval_next_command
                .as_deref()
                .is_some_and(|command| command.contains("retrieval index")
                    && command.contains("--refresh full")),
            "rehydrate output should expose the sidecar rebuild command: {output:?}"
        );
        assert!(
            output.retrieval.contains("no sidecar directory is copied"),
            "rehydrate output should name the sidecar boundary: {}",
            output.retrieval
        );
        assert!(
            output.next_commands.first().is_some_and(|command| {
                command.contains("index --project") && command.contains("--refresh full")
            }) && output
                .next_commands
                .iter()
                .all(|command| !command.contains("--refresh incremental")),
            "rehydrate output must prescribe a full core refresh first: {output:?}"
        );
        assert!(
            !target_cache_path.join("semantic-generation").exists(),
            "source cache sidecars are not portable rehydrate input"
        );
        let unexpected_files = leftover_rehydrate_temps(&target_cache_path);
        assert!(unexpected_files.is_empty(), "{unexpected_files:?}");
        for suffix in ["-wal", "-shm", "-journal"] {
            let database = published.clone();
            let mut sidecar_name = database
                .file_name()
                .expect("database file name")
                .to_os_string();
            sidecar_name.push(suffix);
            assert!(
                !database.with_file_name(sidecar_name).exists(),
                "the atomically published database must be self-contained before activation"
            );
        }
        let storage = Store::open_observational(&published).expect("open published target");
        assert!(
            storage
                .list_retrieval_semantic_generations()
                .expect("list manifests")
                .is_empty()
        );
        assert!(
            storage
                .get_index_publication()
                .expect("publication")
                .is_none()
        );
        assert!(
            storage
                .get_complete_index_publication()
                .expect("complete publication")
                .is_none()
        );
        assert!(
            storage
                .get_dense_anchor_publication_manifest()
                .expect("dense publication")
                .is_none()
        );
        assert!(
            storage
                .get_dense_anchor_inputs_batch_after(None, 10)
                .expect("dense inputs")
                .is_empty()
        );
        assert!(
            storage
                .get_structural_text_unit_publication_manifest()
                .expect("structural publication")
                .is_none()
        );
        assert_eq!(
            storage
                .get_source_policy_exclusions()
                .expect("carried policy exclusions")
                .len(),
            1
        );
        assert!(
            storage
                .get_source_policy_exclusion_manifest()
                .expect("policy publication")
                .is_none()
        );
        let source_root = source_project.path().to_string_lossy();
        assert_eq!(
            storage
                .path_bound_text_match_count(&source_root)
                .expect("source root scan"),
            0,
            "rehydrated target DB must not retain source-worktree absolute paths"
        );
        let target_root = target_project.path().to_string_lossy();
        assert!(
            storage
                .path_bound_text_match_count(&target_root)
                .expect("target root scan")
                > 0,
            "rehydrated target DB should retain rebased target-worktree paths"
        );
        assert_eq!(storage.get_stats().expect("stats").file_count, 1);
        let target_cache_key = test_artifact_cache_key();
        assert!(
            storage
                .get_index_artifact_cache(Path::new("src.rs"), &target_cache_key)
                .expect("target artifact cache lookup")
                .is_none()
        );
        assert!(
            storage
                .get_index_artifact_cache(Path::new("legacy.rs"), "v1:path-bound:legacy")
                .expect("legacy artifact cache lookup")
                .is_none()
        );
        for table in [
            "llm_symbol_doc",
            "symbol_search_doc",
            "symbol_summary",
            "dense_anchor_input",
            "structural_text_unit",
            "structural_text_projection",
            "structural_text_artifact_cache",
            "grounding_repo_stats_snapshot",
            "grounding_file_snapshot",
            "grounding_node_snapshot",
            "grounding_node_summary_snapshot",
            "grounding_node_edge_digest_snapshot",
        ] {
            assert_eq!(table_row_count(&storage, table), 0, "{table}");
        }
        let component_report_nodes: i64 = storage
            .get_connection()
            .query_row(
                "SELECT COUNT(*) FROM node
                 WHERE serialized_name LIKE 'component_report:%'
                    OR canonical_id LIKE 'codestory:component_report:%'",
                [],
                |row| row.get(0),
            )
            .expect("count component-report nodes");
        assert_eq!(component_report_nodes, 0);
        let component_report_projections: i64 = storage
            .get_connection()
            .query_row(
                "SELECT COUNT(*) FROM search_symbol_projection
                 WHERE node_id = 3
                    OR display_name LIKE 'component_report:%'
                    OR display_name LIKE 'codestory::component_report::%'",
                [],
                |row| row.get(0),
            )
            .expect("count component-report projections");
        assert_eq!(component_report_projections, 0);
    }

    #[test]
    fn rehydrate_dry_run_does_not_create_target_cache_metadata() {
        let Some((source_project, target_project)) = matching_git_projects() else {
            return;
        };
        let source_cache = tempdir().expect("source cache");
        let target_parent = tempdir().expect("target parent");
        let target_cache_path = target_parent.path().join("absent-cache");
        seed_cache(
            &source_cache.path().join("codestory.db"),
            source_project.path(),
        );

        let output = rehydrate_cache(CacheRehydrateRequest {
            source_project: source_project.path(),
            source_cache_dir: source_cache.path(),
            target_project: target_project.path(),
            target_cache_dir: &target_cache_path,
            dry_run: true,
        })
        .expect("rehydrate dry run");

        assert_eq!(output.status, "would_rehydrate");
        assert!(!output.copied);
        assert!(
            !source_cache
                .path()
                .join("codestory.index-writer.lock")
                .exists(),
            "dry-run must not create a source lock"
        );
        assert!(
            !target_cache_path.exists(),
            "dry-run must not create a target lock or cache directory"
        );
    }

    #[test]
    fn rehydrate_skips_when_git_tree_differs() {
        let Some((source_project, target_project)) = matching_git_projects() else {
            return;
        };
        fs::write(
            target_project.path().join("src.rs"),
            "pub fn changed() {}\n",
        )
        .expect("modify target");
        git(target_project.path(), &["add", "."]);
        git(target_project.path(), &["commit", "-m", "change"]);

        let source_cache = tempdir().expect("source cache");
        let target_cache = tempdir().expect("target cache");
        seed_cache(
            &source_cache.path().join("codestory.db"),
            source_project.path(),
        );

        let output = rehydrate_cache(CacheRehydrateRequest {
            source_project: source_project.path(),
            source_cache_dir: source_cache.path(),
            target_project: target_project.path(),
            target_cache_dir: target_cache.path(),
            dry_run: false,
        })
        .expect("rehydrate");

        assert_eq!(output.status, "skipped");
        assert_eq!(output.reason.as_deref(), Some("git tree mismatch"));
        assert!(!target_cache.path().join("codestory.db").exists());
    }

    #[test]
    fn rehydrate_skips_when_target_worktree_is_dirty() {
        let Some((source_project, target_project)) = matching_git_projects() else {
            return;
        };
        fs::write(target_project.path().join("src.rs"), "pub fn dirty() {}\n")
            .expect("dirty target");

        let source_cache = tempdir().expect("source cache");
        let target_cache = tempdir().expect("target cache");
        seed_cache(
            &source_cache.path().join("codestory.db"),
            source_project.path(),
        );

        let output = rehydrate_cache(CacheRehydrateRequest {
            source_project: source_project.path(),
            source_cache_dir: source_cache.path(),
            target_project: target_project.path(),
            target_cache_dir: target_cache.path(),
            dry_run: false,
        })
        .expect("rehydrate");

        assert_eq!(output.status, "skipped");
        let reason = output.reason.as_deref().expect("skip reason");
        assert!(
            reason.contains("git worktree is dirty"),
            "dirty target must refuse reuse: {reason}"
        );
        assert!(!target_cache.path().join("codestory.db").exists());
    }

    #[test]
    fn rehydrate_skips_when_target_metadata_reports_issues() {
        let Some((source_project, target_project)) = matching_git_projects() else {
            return;
        };
        // A gitlink index entry makes the target a submodule parent; the
        // confined reader conservatively refuses worktree status for it.
        let head = git_output(target_project.path(), &["rev-parse", "HEAD"]);
        git(
            target_project.path(),
            &[
                "update-index",
                "--add",
                "--cacheinfo",
                &format!("160000,{head},nested"),
            ],
        );

        let source_cache = tempdir().expect("source cache");
        let target_cache = tempdir().expect("target cache");
        seed_cache(
            &source_cache.path().join("codestory.db"),
            source_project.path(),
        );

        let output = rehydrate_cache(CacheRehydrateRequest {
            source_project: source_project.path(),
            source_cache_dir: source_cache.path(),
            target_project: target_project.path(),
            target_cache_dir: target_cache.path(),
            dry_run: false,
        })
        .expect("rehydrate");

        assert_eq!(output.status, "skipped");
        let reason = output.reason.as_deref().expect("skip reason");
        assert!(
            reason.contains("git metadata inspection failed")
                || reason.contains("git worktree is dirty"),
            "metadata issues must refuse reuse: {reason}"
        );
        assert!(!target_cache.path().join("codestory.db").exists());
    }

    #[test]
    fn rehydrate_reuses_when_a_tracked_source_is_repo_ignored_but_restored() {
        let Some((source_project, target_project)) = matching_git_projects() else {
            return;
        };
        // A committed file later covered by the repository's own ignore rules
        // is restored by the repository index, so the inventory stays complete
        // and freshness remains provable: reuse is allowed and the degraded
        // discovery route is carried as a warning, not a refusal (#1734).
        for project in [source_project.path(), target_project.path()] {
            fs::write(project.join("ignored.rs"), "pub fn hidden() {}\n").expect("ignored source");
            fs::write(project.join(".gitignore"), "ignored.rs\n").expect("gitignore");
            git(project, &["add", "-f", "ignored.rs", ".gitignore"]);
            git(project, &["commit", "-m", "track ignored"]);
        }

        let source_cache = tempdir().expect("source cache");
        let target_cache = tempdir().expect("target cache");
        seed_cache(
            &source_cache.path().join("codestory.db"),
            source_project.path(),
        );

        let output = rehydrate_cache(CacheRehydrateRequest {
            source_project: source_project.path(),
            source_cache_dir: source_cache.path(),
            target_project: target_project.path(),
            target_cache_dir: target_cache.path(),
            dry_run: true,
        })
        .expect("rehydrate");

        assert!(
            !output
                .reason
                .as_deref()
                .unwrap_or_default()
                .contains("Partial"),
            "a restored tracked source must not make the inventory partial: {output:?}"
        );
        let manifest =
            codestory_workspace::WorkspaceManifest::open(source_project.path().to_path_buf())
                .expect("open source workspace");
        let inventory = manifest.source_inventory().expect("source inventory");
        assert_eq!(
            inventory.outcome,
            codestory_workspace::WorkspaceInventoryOutcome::Complete
        );
        assert!(inventory.issues.is_empty(), "{inventory:?}");
        assert_eq!(inventory.warnings.len(), 1, "{inventory:?}");
    }

    #[test]
    fn source_cache_freshness_ignores_custom_in_worktree_storage_artifacts() {
        let project = tempdir().expect("project");
        let source_db = project.path().join("cache").join("custom-core.db");
        fs::create_dir_all(source_db.parent().expect("cache parent")).expect("cache parent");
        let _storage = Store::open(&source_db).expect("source store");
        let legacy = codestory_workspace::legacy_search_directory_for_storage(&source_db);
        let generations = codestory_workspace::search_generation_directory_for_storage(&source_db);
        fs::create_dir_all(&legacy).expect("legacy search directory");
        fs::create_dir_all(generations.join("generation-1")).expect("search generation directory");
        fs::write(legacy.join("meta.json"), "{\"generated\":true}\n").expect("legacy metadata");
        fs::write(
            generations.join("generation-1").join("meta.json"),
            "{\"generated\":true}\n",
        )
        .expect("generation metadata");

        let freshness =
            source_cache_freshness(project.path(), &source_db).expect("fresh source cache");
        assert_eq!(freshness.changed_or_new_files, 0);
        assert_eq!(freshness.removed_files, 0);
    }

    #[test]
    fn rehydrate_skips_when_source_cache_is_stale() {
        let scenarios = [
            StaleSourceChange::Modify,
            StaleSourceChange::Add,
            StaleSourceChange::Remove,
        ];
        for scenario in scenarios {
            let Some((source_project, target_project)) = matching_git_projects() else {
                return;
            };
            let source_cache = tempdir().expect("source cache");
            let target_cache = tempdir().expect("target cache");
            seed_cache(
                &source_cache.path().join("codestory.db"),
                source_project.path(),
            );
            apply_stale_source_change(source_project.path(), scenario);
            apply_stale_source_change(target_project.path(), scenario);

            let output = rehydrate_cache(CacheRehydrateRequest {
                source_project: source_project.path(),
                source_cache_dir: source_cache.path(),
                target_project: target_project.path(),
                target_cache_dir: target_cache.path(),
                dry_run: false,
            })
            .expect("rehydrate");

            assert_eq!(output.status, "skipped", "{scenario:?}");
            assert!(
                output
                    .reason
                    .as_deref()
                    .is_some_and(|reason| reason.starts_with("source cache is stale:")),
                "stale source cache should return a clear skip reason: {output:?}"
            );
            assert!(!target_cache.path().join("codestory.db").exists());
        }
    }

    #[test]
    fn rehydrate_skips_incomplete_source_cache() {
        let Some((source_project, target_project)) = matching_git_projects() else {
            return;
        };
        let source_cache = tempdir().expect("source cache");
        let target_cache = tempdir().expect("target cache");
        let target_cache_path = target_cache.path().join("empty");
        let source_db = source_cache.path().join("codestory.db");
        seed_cache(&source_db, source_project.path());
        Store::open(&source_db)
            .expect("open source cache")
            .get_connection()
            .execute(
                "INSERT INTO incomplete_index_run (id, started_at_epoch_ms) VALUES (1, 1)",
                [],
            )
            .expect("seed schema-compatible incomplete marker");

        let output = rehydrate_cache(CacheRehydrateRequest {
            source_project: source_project.path(),
            source_cache_dir: source_cache.path(),
            target_project: target_project.path(),
            target_cache_dir: &target_cache_path,
            dry_run: false,
        })
        .expect("rehydrate");

        assert_eq!(output.status, "skipped");
        assert!(
            output
                .reason
                .as_deref()
                .is_some_and(|reason| reason.contains("incomplete incremental")),
            "unexpected skip reason: {output:?}"
        );
        assert!(!target_cache_path.join("codestory.db").exists());
    }

    #[test]
    fn rehydrate_skips_while_source_index_writer_is_active() {
        let source_project = tempdir().expect("source project");
        let target_project = tempdir().expect("target project");
        let source_cache = tempdir().expect("source cache");
        let target_cache = tempdir().expect("target cache");
        let source_db = source_cache.path().join("codestory.db");
        drop(Store::open(&source_db).expect("seed source cache"));
        let _guard = crate::IndexWriterGuard::try_acquire(&source_db).expect("source writer lock");

        let output = rehydrate_cache(CacheRehydrateRequest {
            source_project: source_project.path(),
            source_cache_dir: source_cache.path(),
            target_project: target_project.path(),
            target_cache_dir: target_cache.path(),
            dry_run: false,
        })
        .expect("rehydrate");

        assert_eq!(output.status, "skipped");
        assert!(
            output
                .reason
                .as_deref()
                .is_some_and(|reason| reason.contains("source cache is busy")),
            "unexpected skip reason: {output:?}"
        );
    }

    #[test]
    fn rehydrate_skips_while_target_index_writer_is_active() {
        let source_project = tempdir().expect("source project");
        let target_project = tempdir().expect("target project");
        let source_cache = tempdir().expect("source cache");
        let target_cache = tempdir().expect("target cache");
        let source_db = source_cache.path().join("codestory.db");
        let target_db = target_cache.path().join("codestory.db");
        drop(Store::open(&source_db).expect("seed source cache"));
        let _guard = crate::IndexWriterGuard::try_acquire(&target_db).expect("target writer lock");

        let output = rehydrate_cache(CacheRehydrateRequest {
            source_project: source_project.path(),
            source_cache_dir: source_cache.path(),
            target_project: target_project.path(),
            target_cache_dir: target_cache.path(),
            dry_run: false,
        })
        .expect("rehydrate");

        assert_eq!(output.status, "skipped");
        assert!(
            output
                .reason
                .as_deref()
                .is_some_and(|reason| reason.contains("target cache is busy")),
            "unexpected skip reason: {output:?}"
        );
    }

    #[test]
    fn rehydrate_skips_when_target_cache_is_inside_source_cache() {
        let project = tempdir().expect("project");
        let source_cache = tempdir().expect("source cache");
        let target_cache_path = source_cache.path().join("nested-target");
        fs::write(project.path().join("src.rs"), "pub fn run() {}\n").expect("write source");
        seed_cache(&source_cache.path().join("codestory.db"), project.path());

        let output = rehydrate_cache(CacheRehydrateRequest {
            source_project: project.path(),
            source_cache_dir: source_cache.path(),
            target_project: project.path(),
            target_cache_dir: &target_cache_path,
            dry_run: false,
        })
        .expect("rehydrate");

        assert_eq!(output.status, "skipped");
        assert_eq!(
            output.reason.as_deref(),
            Some("target cache dir is inside source cache dir")
        );
        assert!(
            !target_cache_path.exists(),
            "nested target should not be created before the guard skips"
        );
    }

    #[test]
    fn compact_rehydrate_publishes_zero_freelist_database() {
        let Some((source_project, target_project)) = matching_git_projects() else {
            return;
        };
        let source_cache = tempdir().expect("source cache");
        let target_cache = tempdir().expect("target cache");
        let target_cache_path = target_cache.path().join("empty");
        fs::create_dir_all(&target_cache_path).expect("create target cache");
        fs::write(target_cache_path.join("codestory.index-writer.lock"), b"")
            .expect("seed persistent target lock");
        let source_db = source_cache.path().join("codestory.db");
        seed_cache(&source_db, source_project.path());

        let output = rehydrate_cache(CacheRehydrateRequest {
            source_project: source_project.path(),
            source_cache_dir: source_cache.path(),
            target_project: target_project.path(),
            target_cache_dir: &target_cache_path,
            dry_run: false,
        })
        .expect("rehydrate");

        assert_eq!(output.status, "rehydrated");
        assert!(
            output
                .peak_space_required_bytes
                .is_some_and(|bytes| bytes > 0),
            "receipt should surface peak space: {output:?}"
        );
        assert!(
            output.available_bytes.is_some(),
            "receipt should surface available bytes"
        );
        assert_eq!(output.candidate_freelist_count, Some(0));
        assert!(
            output.freelist_pages_reclaimed.is_some(),
            "receipt should surface vacuum reclaim stats"
        );
        let observation = codestory_store::observe_sqlite_database(
            &resolved_core_database(&target_cache_path)
                .expect("compact rehydrate must publish a generation"),
        )
        .expect("observe compact rehydrate target");
        assert_eq!(observation.freelist_count, 0);
        assert_eq!(observation.wal_bytes, 0);
        assert_eq!(observation.shm_bytes, 0);
    }

    #[test]
    fn compact_rehydrate_reports_insufficient_space_before_stage_copy() {
        let Some((source_project, target_project)) = matching_git_projects() else {
            return;
        };
        let source_cache = tempdir().expect("source cache");
        let target_cache = tempdir().expect("target cache");
        let target_cache_path = target_cache.path().join("empty");
        fs::create_dir_all(&target_cache_path).expect("create target cache");
        fs::write(target_cache_path.join("codestory.index-writer.lock"), b"")
            .expect("seed persistent target lock");
        let source_db = source_cache.path().join("codestory.db");
        seed_cache(&source_db, source_project.path());

        let output = codestory_store::with_available_filesystem_bytes_override(0, || {
            rehydrate_cache(CacheRehydrateRequest {
                source_project: source_project.path(),
                source_cache_dir: source_cache.path(),
                target_project: target_project.path(),
                target_cache_dir: &target_cache_path,
                dry_run: false,
            })
        })
        .expect("rehydrate");

        assert_eq!(output.status, "insufficient_space");
        assert!(
            output
                .reason
                .as_deref()
                .is_some_and(|reason| reason.contains("insufficient space for compact rehydrate")),
            "unexpected reason: {output:?}"
        );
        assert_eq!(output.copied, false);
        assert!(
            output
                .peak_space_required_bytes
                .is_some_and(|bytes| bytes > 0)
        );
        assert_eq!(output.available_bytes, Some(0));
        assert!(
            resolved_core_database(&target_cache_path).is_none(),
            "insufficient_space must not publish a target generation"
        );
        assert!(
            !target_cache_path.join("codestory.db").exists(),
            "insufficient_space must not publish a target database"
        );
        let leftover_temps = leftover_rehydrate_temps(&target_cache_path);
        assert!(
            leftover_temps.is_empty(),
            "insufficient_space must not allocate stage/candidate temps: {leftover_temps:?}"
        );
    }

    fn resolved_core_database(cache_dir: &Path) -> Option<PathBuf> {
        CorePublicationLayout::from_storage_path(&cache_dir.join("codestory.db"))
            .ok()?
            .resolve_active_database()
            .ok()
            .flatten()
    }

    fn leftover_rehydrate_temps(cache_dir: &Path) -> Vec<PathBuf> {
        let mut leftover = Vec::new();
        let staging = cache_dir.join("core").join("staging");
        if let Ok(entries) = fs::read_dir(&staging) {
            leftover.extend(
                entries
                    .filter_map(|entry| entry.ok())
                    .map(|entry| entry.path()),
            );
        }
        leftover
    }

    /// A leftover legacy `codestory.db` is not the published image. Preflight
    /// and copy must measure the generation `CorePublicationLayout` selects.
    #[test]
    fn rehydrate_copies_the_published_generation_not_a_leftover_legacy_file() {
        let Some((source_project, target_project)) = matching_git_projects() else {
            return;
        };
        let source_cache = tempdir().expect("source cache");
        let target_cache = tempdir().expect("target cache");
        let target_cache_path = target_cache.path().join("empty");
        let logical_source = source_cache.path().join("codestory.db");
        seed_cache(&logical_source, source_project.path());
        let layout = CorePublicationLayout::from_storage_path(&logical_source).expect("layout");
        let staged = layout
            .create_staging_database_path()
            .expect("stage the seeded image");
        fs::copy(&logical_source, &staged).expect("copy seed into staging");
        publish_rehydrated_generation(&staged, &logical_source)
            .expect("publish the seeded image as a generation");
        fs::write(&logical_source, b"stale-leftover").expect("leave a wrong leftover file");

        let output = rehydrate_cache(CacheRehydrateRequest {
            source_project: source_project.path(),
            source_cache_dir: source_cache.path(),
            target_project: target_project.path(),
            target_cache_dir: &target_cache_path,
            dry_run: false,
        })
        .expect("rehydrate");

        assert_eq!(output.status, "rehydrated");
        assert_eq!(output.source_file_count, Some(1));
        let published =
            resolved_core_database(&target_cache_path).expect("target generation published");
        let observation =
            codestory_store::observe_sqlite_database(&published).expect("observe target");
        assert!(
            observation.logical_bytes > b"stale-leftover".len() as u64,
            "the leftover legacy file must not be the measured or copied image: {observation:?}"
        );
        let storage = Store::open_observational(&published).expect("open target");
        assert_eq!(storage.get_stats().expect("stats").file_count, 1);
    }

    fn matching_git_projects() -> Option<(tempfile::TempDir, tempfile::TempDir)> {
        if !git_available() {
            return None;
        }
        let source = tempdir().expect("source project");
        let target = tempdir().expect("target project");
        for project in [source.path(), target.path()] {
            git(project, &["init"]);
            git(
                project,
                &["config", "user.email", "codestory@example.invalid"],
            );
            git(project, &["config", "user.name", "CodeStory Test"]);
            git(
                project,
                &[
                    "remote",
                    "add",
                    "origin",
                    "https://example.invalid/repo.git",
                ],
            );
            fs::write(project.join("src.rs"), "pub fn run() {}\n").expect("write source");
            git(project, &["add", "."]);
            git(project, &["commit", "-m", "init"]);
        }
        Some((source, target))
    }

    #[derive(Debug, Clone, Copy)]
    enum StaleSourceChange {
        Modify,
        Add,
        Remove,
    }

    fn apply_stale_source_change(project: &Path, scenario: StaleSourceChange) {
        std::thread::sleep(std::time::Duration::from_millis(5));
        match scenario {
            StaleSourceChange::Modify => {
                fs::write(project.join("src.rs"), "pub fn changed() {}\n").expect("modify source");
            }
            StaleSourceChange::Add => {
                fs::write(project.join("new.rs"), "pub fn new_file() {}\n").expect("add source");
            }
            StaleSourceChange::Remove => {
                fs::remove_file(project.join("src.rs")).expect("remove source");
            }
        }
        git(project, &["add", "-A"]);
        git(project, &["commit", "-m", "stale source change"]);
    }

    fn add_matching_oversized_source(source: &Path, target: &Path) {
        let content =
            vec![b'x'; codestory_contracts::workspace::DEFAULT_SOURCE_FILE_BYTE_CAP as usize + 1];
        for project in [source, target] {
            fs::write(project.join("oversized.rs"), &content).expect("write oversized source");
            git(project, &["add", "oversized.rs"]);
            git(project, &["commit", "-m", "add oversized source"]);
        }
    }

    fn seed_cache(path: &Path, project: &Path) {
        let mut storage = Store::open(path).expect("open storage");
        let absolute_source = project.join("src.rs");
        let absolute_source_text = absolute_source.to_string_lossy().to_string();
        let source_mtime = fs::metadata(&absolute_source)
            .expect("source metadata")
            .modified()
            .expect("source modified")
            .duration_since(std::time::UNIX_EPOCH)
            .expect("source mtime since epoch")
            .as_millis()
            .min(i64::MAX as u128) as i64;
        storage
            .insert_nodes_batch(&[
                Node {
                    id: NodeId(1),
                    kind: NodeKind::FILE,
                    serialized_name: absolute_source_text.clone(),
                    ..Default::default()
                },
                Node {
                    id: NodeId(2),
                    kind: NodeKind::FUNCTION,
                    serialized_name: format!("{absolute_source_text}::run"),
                    qualified_name: Some(format!("{absolute_source_text}::run")),
                    file_node_id: Some(NodeId(1)),
                    start_line: Some(1),
                    end_line: Some(1),
                    ..Default::default()
                },
                Node {
                    id: NodeId(3),
                    kind: NodeKind::MODULE,
                    serialized_name: "component_report:workspace".into(),
                    qualified_name: Some("codestory::component_report::workspace".into()),
                    canonical_id: Some("codestory:component_report:workspace".into()),
                    ..Default::default()
                },
            ])
            .expect("node");
        storage
            .insert_file(&codestory_store::FileInfo {
                id: 1,
                path: PathBuf::from(&absolute_source_text),
                language: "rust".into(),
                modification_time: source_mtime,
                indexed: true,
                complete: true,
                line_count: 1,
                file_role: codestory_store::FileRole::Source,
            })
            .expect("file");
        storage
            .rebuild_search_symbol_projection_from_node_table()
            .expect("projection");
        assert_eq!(
            storage
                .get_connection()
                .query_row(
                    "SELECT COUNT(*) FROM search_symbol_projection WHERE node_id = 3",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .expect("prove component-report projection fixture"),
            1
        );
        storage
            .upsert_symbol_search_docs_batch(&[codestory_store::SymbolSearchDoc {
                node_id: NodeId(2),
                file_node_id: Some(NodeId(1)),
                kind: NodeKind::FUNCTION,
                display_name: format!("{absolute_source_text}::run"),
                qualified_name: Some(format!("{absolute_source_text}::run")),
                file_path: Some(absolute_source_text.clone()),
                start_line: Some(1),
                doc_text: format!("source file: {absolute_source_text}"),
                doc_version: 1,
                doc_hash: "symbol-doc-hash".into(),
                policy_version: "test".into(),
                source_provenance: absolute_source_text.clone(),
                updated_at_epoch_ms: 1,
            }])
            .expect("symbol docs");
        storage
            .upsert_llm_symbol_docs_batch(&[codestory_store::LlmSymbolDoc {
                node_id: NodeId(2),
                file_node_id: Some(NodeId(1)),
                kind: NodeKind::FUNCTION,
                display_name: format!("{absolute_source_text}::run"),
                qualified_name: Some(format!("{absolute_source_text}::run")),
                file_path: Some(absolute_source_text.clone()),
                start_line: Some(1),
                doc_text: format!("llm source file: {absolute_source_text}"),
                doc_version: 1,
                doc_hash: "llm-doc-hash".into(),
                embedding_profile: None,
                embedding_model: "test".into(),
                embedding_backend: None,
                embedding_dim: 1,
                doc_shape: None,
                semantic_policy_version: None,
                dense_reason: None,
                embedding: vec![1.0],
                updated_at_epoch_ms: 1,
            }])
            .expect("llm docs");
        storage
            .upsert_dense_anchor_inputs_batch(&[codestory_store::DenseAnchorInput {
                node_id: NodeId(2),
                file_node_id: Some(NodeId(1)),
                kind: NodeKind::FUNCTION,
                display_name: format!("{absolute_source_text}::run"),
                qualified_name: Some(format!("{absolute_source_text}::run")),
                file_path: Some(absolute_source_text.clone()),
                start_line: Some(1),
                end_line: Some(1),
                file_role: codestory_store::FileRole::Source,
                source_provenance: absolute_source_text.clone(),
                text: format!("dense source file: {absolute_source_text}"),
                document_hash: "dense-doc-hash".into(),
                selection_reason: "test".into(),
                policy_version: "test".into(),
                source_identity: "core:source-generation:source-run".into(),
                updated_at_epoch_ms: 1,
            }])
            .expect("dense inputs");
        storage
            .upsert_symbol_summaries_batch(&[codestory_store::SymbolSummaryRecord {
                node_id: NodeId(2),
                content_hash: "a".repeat(64),
                summary: "derived semantic summary".into(),
                model: "test".into(),
                updated_at_epoch_ms: 1,
            }])
            .expect("symbol summary");
        let publication = codestory_store::IndexPublicationRecord {
            generation: 1,
            generation_id: "source-generation".into(),
            run_id: "source-run".into(),
            mode: codestory_store::IndexPublicationMode::Full,
            published_at_epoch_ms: 1,
        };
        storage
            .publish_dense_anchor_generation(&publication, "test")
            .expect("dense publication");
        storage
            .put_index_publication(&publication)
            .expect("core publication");
        storage
            .get_connection()
            .execute_batch(
                "UPDATE file SET content_hash = 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa' WHERE id = 1;
                 INSERT INTO structural_text_unit (
                    node_id, file_id, placement_id, content_hash, source_content_hash,
                    descriptor_version, producer, evidence_tier, resolution, language, kind,
                    start_line, start_col, end_line, end_col, file_role
                 ) VALUES (
                    2, 1,
                    'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb',
                    'cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc',
                    'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                    1, 'test', 'structural_text', 'source_range_only', 'rust', 3,
                    1, 1, 1, 10, 'source'
                 );
                 INSERT INTO structural_text_projection (
                    file_id, source_content_hash, descriptor_version, producer, language,
                    file_role, unit_count, unit_digest
                 ) VALUES (
                    1,
                    'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                    1, 'test', 'rust', 'source', 1,
                    'dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd'
                 );
                 INSERT INTO structural_text_artifact_cache (
                    file_path, file_id, cache_key, source_content_hash, descriptor_version,
                    producer, artifact_digest, artifact_blob, updated_at_epoch_ms
                 ) VALUES (
                    'src.rs', 1, 'test',
                    'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                    1, 'test',
                    'eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee',
                    X'01', 1
                 );
                 INSERT INTO structural_text_unit_publication (
                    id, schema_version, complete, core_generation_id, core_run_id,
                    unit_count, unit_digest, projection_count, projection_digest,
                    descriptor_version, migration_state, published_at_epoch_ms
                 ) VALUES (
                    1, 1, 1, 'source-generation', 'source-run', 1,
                    'dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd',
                    1,
                    'ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff',
                    1, 'native_v1', 1
                 );
                 ;",
            )
            .expect("seed structural and policy state");
        let source_content_hash = format!(
            "{:x}",
            Sha256::digest(fs::read(&absolute_source).expect("read source for hash"))
        );
        storage
            .get_connection()
            .execute(
                "UPDATE file SET content_hash = ?1 WHERE id = 1",
                [&source_content_hash],
            )
            .expect("bind stored source content hash");
        let oversized = project.join("oversized.rs");
        if oversized.is_file() {
            let policy = SourceIndexPolicy::default();
            let oversized_bytes = fs::read(&oversized).expect("read oversized source");
            let oversized_hash = format!("{:x}", Sha256::digest(&oversized_bytes));
            storage
                .publish_source_policy_exclusion_generation(
                    &publication,
                    "source-project",
                    "source-workspace",
                    codestory_store::SourcePolicyExclusionPolicyIdentity::new(
                        &policy.policy_version,
                        policy.byte_cap,
                        policy.structural_unit_cap,
                    ),
                    &[
                        codestory_contracts::workspace::OversizedSourceExclusionCandidate {
                            normalized_path: "oversized.rs".into(),
                            content_hash: oversized_hash,
                            observed_size: oversized_bytes.len() as u64,
                            observed_unit_count: 0,
                            policy_version: policy.policy_version.clone(),
                            byte_cap: policy.byte_cap,
                            structural_unit_cap: policy.structural_unit_cap,
                        },
                    ],
                )
                .expect("publish real oversized source exclusion");
        }
        storage
            .upsert_index_artifact_cache(
                Path::new("src.rs"),
                &test_artifact_cache_key(),
                b"portable artifact",
            )
            .expect("artifact");
        storage
            .upsert_index_artifact_cache(
                Path::new("legacy.rs"),
                "v1:path-bound:legacy",
                b"legacy artifact",
            )
            .expect("legacy artifact");
        storage
            .upsert_retrieval_index_manifest(&codestory_store::RetrievalIndexManifest {
                project_id: codestory_retrieval::project_id_for_root(project),
                lexical_version: codestory_retrieval::LEXICAL_INDEX_VERSION.into(),
                semantic_generation: "codestory_old".into(),
                scip_revision: None,
                built_at_epoch_ms: 1,
                disk_bytes: None,
                degraded_modes_json: "[]".into(),
                embedding_backend: None,
                embedding_dim: None,
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
            })
            .expect("manifest");
        storage
            .refresh_grounding_snapshots()
            .expect("seed ready grounding snapshots");
    }

    fn table_row_count(storage: &Store, table: &str) -> i64 {
        storage
            .get_connection()
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .expect("count test table")
    }

    fn test_artifact_cache_key() -> String {
        "v2:portable-test".into()
    }
}
