use anyhow::{Context, Result, bail};
use codestory_store::{CURRENT_SCHEMA_VERSION, Store};
use codestory_workspace::{
    RefreshInputs, WorkspaceInventory, WorkspaceInventoryOutcome, WorkspaceManifest,
};
use serde::Serialize;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

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
/// `preserved_scope` and `retrieval` explain the contract boundary: SQLite graph/search/doc rows
/// may be reused after rebasing, but sidecar directories and retrieval manifests must be rebuilt
/// or revalidated for the target worktree.
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
    pub rebased_path_bound_rows: usize,
    pub preserved_scope: String,
    pub retrieval_status: String,
    pub retrieval_reason: String,
    pub retrieval_next_command: Option<String>,
    pub retrieval: String,
    pub next_commands: Vec<String>,
}

/// Copy a compatible cache, rebase path-bound rows, and invalidate copied retrieval manifests.
///
/// Skipped results are intentional safety outcomes, not hard failures. They preserve correctness
/// when cache identity, freshness, or directory boundaries are not strong enough.
pub fn rehydrate_cache(request: CacheRehydrateRequest<'_>) -> Result<CacheRehydrateOutput> {
    let source_db = request.source_cache_dir.join("codestory.db");
    let target_db = request.target_cache_dir.join("codestory.db");
    let rebuild = rebuild_commands(request.target_project);

    if request.source_cache_dir == request.target_cache_dir {
        return Ok(skipped(
            request,
            "source and target cache dirs are identical",
            rebuild,
        ));
    }
    if !source_db.is_file() {
        return Ok(skipped(
            request,
            "source cache has no codestory.db",
            rebuild,
        ));
    }
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
        Some(match super::IndexWriterGuard::try_acquire(&source_db) {
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
        })
    };
    let _target_writer_guard = if request.dry_run {
        None
    } else {
        Some(match super::IndexWriterGuard::try_acquire(&target_db) {
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
        })
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

    let schema_version = Store::database_schema_version(&source_db)
        .with_context(|| format!("read source cache schema {}", source_db.display()))?;
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
        let storage = Store::open(&source_db).context("open source cache for stats")?;
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

    let source_freshness = match source_cache_freshness(request.source_project, &source_db) {
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

    if !request.dry_run {
        copy_dir_recursive(request.source_cache_dir, request.target_cache_dir).with_context(
            || {
                format!(
                    "copy cache {} -> {}",
                    request.source_cache_dir.display(),
                    request.target_cache_dir.display()
                )
            },
        )?;
    }
    let mut invalidated_retrieval_manifests = 0;
    let mut invalidated_index_artifact_rows = 0;
    let mut rebased_path_bound_rows = 0;
    if !request.dry_run {
        let mut storage = Store::open(&target_db).context("open copied target cache")?;
        invalidated_retrieval_manifests = storage
            .clear_retrieval_index_manifests()
            .context("invalidate copied retrieval manifests")?;
        let (rebased_rows, invalidated_artifacts) = storage
            .rebase_rehydrated_path_bound_cache(request.source_project, request.target_project)
            .context("rebase copied path-bound cache rows")?;
        rebased_path_bound_rows = rebased_rows;
        invalidated_index_artifact_rows = invalidated_artifacts;
    }

    Ok(CacheRehydrateOutput {
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
        invalidated_index_artifact_rows,
        rebased_path_bound_rows,
        preserved_scope: "sqlite_graph_search_docs_rebased_v2_index_artifacts_preserved".into(),
        retrieval_status: retrieval_rehydrate_status(request.dry_run),
        retrieval_reason: retrieval_rehydrate_reason(),
        retrieval_next_command: Some(retrieval_next_command(request.target_project)),
        retrieval: retrieval_rehydrate_policy(request.dry_run),
        next_commands: rehydrate_next_commands(request.target_project),
    })
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
    let workspace = WorkspaceManifest::open(project.to_path_buf())
        .with_context(|| format!("open source workspace {}", project.display()))?;
    let storage = Store::open(source_db).context("open source cache for freshness")?;
    if storage
        .has_incomplete_incremental_run()
        .context("inspect source cache incomplete index marker")?
    {
        bail!("source cache has an incomplete incremental index run");
    }
    let refresh = workspace
        .build_execution_outcome(&RefreshInputs {
            stored_files: storage.files().inventory()?,
            inventory: WorkspaceInventory::default(),
        })
        .context("build source cache refresh plan")?;
    if refresh.inventory_outcome != WorkspaceInventoryOutcome::Complete {
        bail!(
            "source workspace inventory is {:?}; cache freshness cannot be proven",
            refresh.inventory_outcome
        );
    }
    let plan = refresh.plan;
    Ok(SourceCacheFreshness {
        changed_or_new_files: plan.files_to_index.len(),
        removed_files: plan.files_to_remove.len(),
    })
}

fn git_identity(project: &Path) -> Result<GitIdentity> {
    let dirty = git_output(project, &["status", "--porcelain"])?;
    if !dirty.trim().is_empty() {
        bail!("git worktree is dirty: {}", project.display());
    }
    let remote = git_output(project, &["config", "--get", "remote.origin.url"])?;
    let remote = remote.trim();
    if remote.is_empty() {
        bail!("git remote origin is missing: {}", project.display());
    }
    let tree = git_output(project, &["rev-parse", "HEAD^{tree}"])?;
    Ok(GitIdentity {
        remote: remote.to_string(),
        tree: tree.trim().to_string(),
    })
}

fn git_output(project: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(project)
        .args(args)
        .output()
        .with_context(|| format!("run git in {}", project.display()))?;
    if !output.status.success() {
        bail!(
            "git {} failed in {}: {}",
            args.join(" "),
            project.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn target_cache_has_contents(path: &Path) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    for entry in
        fs::read_dir(path).with_context(|| format!("read target cache dir {}", path.display()))?
    {
        if entry?.file_name() != "codestory.index-writer.lock" {
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

fn copy_dir_recursive(source: &Path, target: &Path) -> Result<()> {
    fs::create_dir_all(target)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let source_path = entry.path();
        let target_path = target.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_recursive(&source_path, &target_path)?;
        } else if entry.file_name() == "codestory.db" {
            Store::copy_database_snapshot(&source_path, &target_path)?;
        } else if matches!(
            entry.file_name().to_string_lossy().as_ref(),
            "codestory.db-wal" | "codestory.db-shm" | "codestory.index-writer.lock"
        ) {
            // SQLite backup snapshots the DB; live sidecars and process locks are never portable.
            continue;
        } else {
            fs::copy(&source_path, &target_path)?;
        }
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
        rebased_path_bound_rows: 0,
        preserved_scope: "none".into(),
        retrieval_status: "not_rehydrated".into(),
        retrieval_reason: "normal index and retrieval rebuild required".into(),
        retrieval_next_command: None,
        retrieval: "not rehydrated; normal index/retrieval rebuild required".into(),
        next_commands,
    }
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
        format!("codestory-cli doctor --project {project}"),
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
    "cache rehydrate copies SQLite graph/search/doc state only; sidecar manifests and Lexical/Qdrant/SCIP artifacts must be rebuilt or revalidated for the target worktree".into()
}

fn retrieval_rehydrate_policy(dry_run: bool) -> String {
    let action = if dry_run {
        "would be invalidated"
    } else {
        "invalidated"
    };
    format!(
        "path-bound SQLite graph/search/doc rows rebased; portable v2 index artifact rows preserved; retrieval manifests {action} because cache rehydrate copies SQLite cache state only; Lexical/Qdrant/SCIP sidecar directories live outside the copied cache and must be revalidated by retrieval index before reuse"
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
    use codestory_contracts::graph::{Node, NodeId, NodeKind};
    use std::path::PathBuf;
    use tempfile::tempdir;

    #[test]
    fn rehydrate_rebases_path_bound_rows_without_source_root_leakage() {
        let Some((source_project, target_project)) = matching_git_projects() else {
            return;
        };
        let source_cache = tempdir().expect("source cache");
        let target_cache = tempdir().expect("target cache");
        let target_cache_path = target_cache.path().join("empty");
        fs::create_dir_all(&target_cache_path).expect("create lock-only target cache");
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
        assert!(target_cache_path.join("codestory.db").is_file());
        assert!(
            target_cache_path
                .join("codestory.index-writer.lock")
                .exists(),
            "the target owns its persistent writer lock after rehydrate"
        );
        assert_eq!(output.invalidated_retrieval_manifests, 1);
        assert_eq!(output.invalidated_index_artifact_rows, 1);
        assert!(output.rebased_path_bound_rows > 0);
        assert_eq!(
            output.preserved_scope,
            "sqlite_graph_search_docs_rebased_v2_index_artifacts_preserved"
        );
        assert_eq!(output.retrieval_status, "invalidated_requires_rebuild");
        assert!(
            output
                .retrieval_reason
                .contains("SQLite graph/search/doc state only"),
            "rehydrate output should distinguish SQLite reuse from sidecar readiness: {}",
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
            output
                .retrieval
                .contains("retrieval manifests invalidated because cache rehydrate copies SQLite cache state only"),
            "rehydrate output should name the fail-closed sidecar rebuild reason: {}",
            output.retrieval
        );
        let storage = Store::open(target_cache_path.join("codestory.db")).expect("open target");
        assert!(
            storage
                .list_retrieval_qdrant_collections()
                .expect("list manifests")
                .is_empty()
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
        assert_eq!(
            storage
                .get_index_artifact_cache(Path::new("src.rs"), &target_cache_key)
                .expect("target artifact cache lookup"),
            Some(b"portable artifact".to_vec())
        );
        assert!(
            storage
                .get_index_artifact_cache(Path::new("legacy.rs"), "v1:path-bound:legacy")
                .expect("legacy artifact cache lookup")
                .is_none()
        );
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

    fn matching_git_projects() -> Option<(tempfile::TempDir, tempfile::TempDir)> {
        if Command::new("git").arg("--version").output().is_err() {
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

    fn git(project: &Path, args: &[&str]) {
        let output = Command::new("git")
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
                qdrant_collection: "codestory_old".into(),
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
    }

    fn test_artifact_cache_key() -> String {
        "v2:portable-test".into()
    }
}
