//! Qdrant on-disk storage repair before sidecar bootstrap (invalid dirs, retention, stub migration).

use crate::config::{SidecarLayout, user_cache_root};
use crate::qdrant_client::QdrantClient;
use anyhow::{Context, Result};
use codestory_store::Store;
use codestory_workspace::owned_deletion::OwnedDeletionRoot;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{debug, warn};

/// Bootstrap repairs invalid storage only. Valid generation retention runs after publication.
pub const DEFAULT_QDRANT_COLLECTION_RETENTION: usize = 0;

/// Machine-readable reason when retention deletes are skipped after protection-scan errors.
pub const PRUNE_SUPPRESSED_PROTECTION_SCAN_ERROR: &str = "protection_scan_error";
pub const PRUNE_SUPPRESSED_POST_PUBLICATION_RETENTION: &str =
    "post_publication_generation_retention";

/// Inputs for manifest-aware Qdrant collection protection during bootstrap repair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootstrapStorageScope {
    pub repo_root: Option<PathBuf>,
    pub active_storage_path: Option<PathBuf>,
    pub active_cache_root: Option<PathBuf>,
    pub global_cache_root: PathBuf,
}

impl BootstrapStorageScope {
    pub fn from_parts(
        repo_root: Option<&Path>,
        active_storage_path: Option<&Path>,
        active_cache_root: Option<&Path>,
    ) -> Self {
        Self {
            repo_root: repo_root.map(Path::to_path_buf),
            active_storage_path: active_storage_path.map(Path::to_path_buf),
            active_cache_root: active_cache_root.map(Path::to_path_buf),
            global_cache_root: user_cache_root(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct ProtectionScanResult {
    protected: HashSet<String>,
    recency_by_collection: HashMap<String, i64>,
    scan_errors: Vec<String>,
    sources_scanned: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct QdrantStorageRepairReport {
    pub qdrant_reachable: bool,
    pub removed_invalid_dirs: usize,
    pub migrated_legacy_stub_markers: usize,
    pub pruned_collections: usize,
    pub protected_collections: usize,
    pub collections_seen: usize,
    pub prune_candidates: usize,
    pub overflow_protected: bool,
    pub scan_errors: Vec<String>,
    pub sources_scanned: usize,
    /// Set when retention deletes were skipped (e.g. `protection_scan_error`).
    pub prune_suppressed_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RetentionPlan {
    to_prune: Vec<String>,
    overflow_protected: bool,
}

pub fn repair_qdrant_storage(
    layout: &SidecarLayout,
    scope: &BootstrapStorageScope,
    max_keep: usize,
) -> Result<QdrantStorageRepairReport> {
    let mut scan = collect_protected_collections(scope)?;
    let protected = scan.protected.clone();
    let protected_count = protected.len();
    let qdrant = QdrantClient::new(layout);
    let probe = qdrant.list_collections_probe();

    let (removed_invalid_dirs, migrated_legacy_stub_markers) =
        if offline_disk_cleanup_when_unreachable(probe.reachable) {
            run_offline_disk_cleanup(&layout.qdrant_data_dir)?
        } else {
            (0, 0)
        };

    let prune_suppressed_reason = prune_suppressed_reason(&scan.scan_errors).or_else(|| {
        (max_keep == 0).then(|| PRUNE_SUPPRESSED_POST_PUBLICATION_RETENTION.to_string())
    });
    let suppress_prune = prune_suppressed_reason.is_some();

    let (pruned, collections_seen, prune_candidates, overflow_protected) = if suppress_prune {
        measure_retention(
            &qdrant,
            layout,
            probe.reachable,
            &protected,
            &scan.recency_by_collection,
            if max_keep == 0 { usize::MAX } else { max_keep },
        )?
    } else if probe.reachable {
        execute_retention_via_http(
            &qdrant,
            layout,
            &protected,
            &scan.recency_by_collection,
            max_keep,
            &mut scan.scan_errors,
        )?
    } else {
        execute_retention_on_disk(
            &layout.qdrant_data_dir,
            &protected,
            &scan.recency_by_collection,
            max_keep,
            &mut scan.scan_errors,
        )?
    };

    debug!(
        qdrant_reachable = probe.reachable,
        protected_collections = protected_count,
        removed_invalid_dirs,
        migrated_legacy_stub_markers,
        pruned_collections = pruned,
        collections_seen,
        prune_candidates,
        overflow_protected,
        scan_errors = scan.scan_errors.len(),
        prune_suppressed = prune_suppressed_reason.is_some(),
        "qdrant storage repair complete"
    );

    Ok(QdrantStorageRepairReport {
        qdrant_reachable: probe.reachable,
        removed_invalid_dirs,
        migrated_legacy_stub_markers,
        pruned_collections: pruned,
        protected_collections: protected_count,
        collections_seen,
        prune_candidates,
        overflow_protected,
        scan_errors: scan.scan_errors,
        sources_scanned: scan.sources_scanned,
        prune_suppressed_reason,
    })
}

fn prune_on_scan_error_override() -> bool {
    std::env::var("CODESTORY_RETRIEVAL_PRUNE_ON_SCAN_ERROR")
        .ok()
        .is_some_and(|value| {
            matches!(
                value.trim(),
                "1" | "true" | "TRUE" | "yes" | "YES" | "on" | "ON"
            )
        })
}

fn prune_suppressed_reason(scan_errors: &[String]) -> Option<String> {
    if scan_errors.is_empty() || prune_on_scan_error_override() {
        None
    } else {
        Some(PRUNE_SUPPRESSED_PROTECTION_SCAN_ERROR.to_string())
    }
}

fn run_offline_disk_cleanup(qdrant_data_dir: &Path) -> Result<(usize, usize)> {
    Ok((
        remove_invalid_collection_dirs(qdrant_data_dir)?,
        migrate_legacy_stub_markers(qdrant_data_dir)?,
    ))
}

fn offline_disk_cleanup_when_unreachable(qdrant_reachable: bool) -> bool {
    !qdrant_reachable
}

fn collect_protected_collections(scope: &BootstrapStorageScope) -> Result<ProtectionScanResult> {
    let mut result = ProtectionScanResult::default();

    let mut scanned_dbs = HashSet::new();
    scan_cache_tree(&scope.global_cache_root, &mut scanned_dbs, &mut result)?;

    if let Some(active_cache) = scope.active_cache_root.as_deref()
        && active_cache != scope.global_cache_root.as_path()
    {
        scan_cache_tree(active_cache, &mut scanned_dbs, &mut result)?;
    }

    if let Some(storage_path) = scope.active_storage_path.as_deref()
        && storage_path.is_file()
    {
        scan_manifest_db(storage_path, &mut scanned_dbs, &mut result)?;
    }

    Ok(result)
}

fn scan_cache_tree(
    cache_root: &Path,
    scanned_dbs: &mut HashSet<PathBuf>,
    result: &mut ProtectionScanResult,
) -> Result<()> {
    let flat_db = cache_root.join("codestory.db");
    if flat_db.is_file() {
        scan_manifest_db(&flat_db, scanned_dbs, result)?;
    }
    if !cache_root.is_dir() {
        return Ok(());
    }
    let entries = match std::fs::read_dir(cache_root) {
        Ok(entries) => entries,
        Err(err) => {
            result
                .scan_errors
                .push(format!("read cache root {}: {err}", cache_root.display()));
            return Ok(());
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                result.scan_errors.push(format!(
                    "read cache entry under {}: {err}",
                    cache_root.display()
                ));
                continue;
            }
        };
        let db_path = entry.path().join("codestory.db");
        if db_path.is_file() {
            scan_manifest_db(&db_path, scanned_dbs, result)?;
        }
    }
    Ok(())
}

fn scan_manifest_db(
    db_path: &Path,
    scanned_dbs: &mut HashSet<PathBuf>,
    result: &mut ProtectionScanResult,
) -> Result<()> {
    let canonical = db_path
        .canonicalize()
        .unwrap_or_else(|_| db_path.to_path_buf());
    if !scanned_dbs.insert(canonical) {
        return Ok(());
    }
    result.sources_scanned += 1;
    match Store::open(db_path) {
        Ok(storage) => match storage.list_retrieval_qdrant_collections_with_recency() {
            Ok(entries) => {
                for (collection, recency_ms) in entries {
                    result.protected.insert(collection.clone());
                    result
                        .recency_by_collection
                        .entry(collection)
                        .and_modify(|existing| *existing = (*existing).max(recency_ms))
                        .or_insert(recency_ms);
                }
            }
            Err(err) => result
                .scan_errors
                .push(format!("list manifests in {}: {err}", db_path.display())),
        },
        Err(err) => result
            .scan_errors
            .push(format!("open cache db {}: {err}", db_path.display())),
    }
    Ok(())
}

fn collection_has_config(collection_dir: &Path) -> bool {
    collection_dir.join("config.json").is_file()
        || collection_dir.join("config").join("params.json").is_file()
}

fn remove_invalid_collection_dirs(qdrant_data_dir: &Path) -> Result<usize> {
    let deletion = match OwnedDeletionRoot::open(qdrant_data_dir) {
        Ok(deletion) => deletion,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => {
            return Err(error).with_context(|| {
                format!("open owned Qdrant data root {}", qdrant_data_dir.display())
            });
        }
    };
    let collections_dir = qdrant_data_dir.join("collections");
    let Ok(entries) = std::fs::read_dir(&collections_dir) else {
        return Ok(0);
    };
    let mut removed = 0usize;
    for entry in entries.flatten() {
        let collection_dir = entry.path();
        if !collection_dir.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("codestory_") {
            continue;
        }
        if !collection_has_config(&collection_dir)
            && deletion
                .remove(Path::new("collections").join(&name).as_path())
                .with_context(|| {
                    format!(
                        "remove invalid qdrant collection dir without config {}",
                        collection_dir.display()
                    )
                })?
        {
            removed += 1;
        }
    }
    Ok(removed)
}

fn migrate_legacy_stub_markers(qdrant_data_dir: &Path) -> Result<usize> {
    let collections_dir = qdrant_data_dir.join("collections");
    let Ok(entries) = std::fs::read_dir(&collections_dir) else {
        return Ok(0);
    };
    let mut migrated = 0usize;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let legacy = QdrantClient::legacy_stub_marker_path(qdrant_data_dir, &name);
        if !legacy.is_file() {
            continue;
        }
        let new_path = QdrantClient::stub_marker_path(qdrant_data_dir, &name);
        if !new_path.is_file() {
            if let Some(parent) = new_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(&legacy, &new_path).with_context(|| {
                format!(
                    "migrate legacy qdrant stub marker {} -> {}",
                    legacy.display(),
                    new_path.display()
                )
            })?;
            migrated += 1;
        }
        let _ = std::fs::remove_file(legacy);
    }
    Ok(migrated)
}

#[derive(Debug, Clone)]
struct CollectionCandidate {
    name: String,
    rank_key: u128,
}

fn candidate_rank_key(recency_ms: Option<i64>, modified: SystemTime) -> u128 {
    if let Some(ms) = recency_ms {
        return ms.max(0) as u128;
    }
    modified
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn plan_retention(
    all: &[CollectionCandidate],
    protected: &HashSet<String>,
    max_keep: usize,
) -> RetentionPlan {
    if all.len() <= max_keep {
        return RetentionPlan {
            to_prune: Vec::new(),
            overflow_protected: false,
        };
    }
    let mut unprotected: Vec<&CollectionCandidate> = all
        .iter()
        .filter(|candidate| !protected.contains(&candidate.name))
        .collect();
    if unprotected.is_empty() {
        return RetentionPlan {
            to_prune: Vec::new(),
            overflow_protected: true,
        };
    }
    let excess = all.len().saturating_sub(max_keep);
    unprotected.sort_by_key(|candidate| candidate.rank_key);
    RetentionPlan {
        to_prune: unprotected
            .into_iter()
            .take(excess)
            .map(|candidate| candidate.name.clone())
            .collect(),
        overflow_protected: false,
    }
}

fn list_on_disk_codestory_collections(
    qdrant_data_dir: &Path,
    recency_by_collection: &HashMap<String, i64>,
) -> Result<Vec<CollectionCandidate>> {
    let collections_dir = qdrant_data_dir.join("collections");
    let Ok(entries) = std::fs::read_dir(&collections_dir) else {
        return Ok(Vec::new());
    };
    let mut candidates = Vec::new();
    for entry in entries.flatten() {
        let collection_dir = entry.path();
        if !collection_dir.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("codestory_") || !collection_has_config(&collection_dir) {
            continue;
        }
        let modified = entry
            .metadata()
            .ok()
            .and_then(|metadata| metadata.modified().ok())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        let recency = recency_by_collection.get(&name).copied();
        candidates.push(CollectionCandidate {
            name,
            rank_key: candidate_rank_key(recency, modified),
        });
    }
    Ok(candidates)
}

fn measure_retention(
    qdrant: &QdrantClient,
    layout: &SidecarLayout,
    qdrant_reachable: bool,
    protected: &HashSet<String>,
    recency_by_collection: &HashMap<String, i64>,
    max_keep: usize,
) -> Result<(usize, usize, usize, bool)> {
    let all = if qdrant_reachable {
        list_http_codestory_collections(qdrant, layout, recency_by_collection)?
    } else {
        list_on_disk_codestory_collections(&layout.qdrant_data_dir, recency_by_collection)?
    };
    let collections_seen = all.len();
    let plan = plan_retention(&all, protected, max_keep);
    Ok((
        0,
        collections_seen,
        plan.to_prune.len(),
        plan.overflow_protected,
    ))
}

fn list_http_codestory_collections(
    qdrant: &QdrantClient,
    layout: &SidecarLayout,
    recency_by_collection: &HashMap<String, i64>,
) -> Result<Vec<CollectionCandidate>> {
    let names = qdrant.list_collection_names().unwrap_or_default();
    let mut all: Vec<CollectionCandidate> = names
        .into_iter()
        .filter(|name| name.starts_with("codestory_"))
        .map(|name| {
            let dir = layout.qdrant_data_dir.join("collections").join(&name);
            let modified = std::fs::metadata(&dir)
                .ok()
                .and_then(|metadata| metadata.modified().ok())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            let recency = recency_by_collection.get(&name).copied();
            CollectionCandidate {
                name,
                rank_key: candidate_rank_key(recency, modified),
            }
        })
        .collect();
    if all.is_empty() {
        all = list_on_disk_codestory_collections(&layout.qdrant_data_dir, recency_by_collection)?;
    }
    Ok(all)
}

fn execute_retention_on_disk(
    qdrant_data_dir: &Path,
    protected: &HashSet<String>,
    recency_by_collection: &HashMap<String, i64>,
    max_keep: usize,
    repair_errors: &mut Vec<String>,
) -> Result<(usize, usize, usize, bool)> {
    let deletion = match OwnedDeletionRoot::open(qdrant_data_dir) {
        Ok(deletion) => deletion,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok((0, 0, 0, false));
        }
        Err(error) => {
            return Err(error).with_context(|| {
                format!("open owned Qdrant data root {}", qdrant_data_dir.display())
            });
        }
    };
    let all = list_on_disk_codestory_collections(qdrant_data_dir, recency_by_collection)?;
    let collections_seen = all.len();
    let plan = plan_retention(&all, protected, max_keep);
    let prune_candidates = plan.to_prune.len();
    let mut removed = 0usize;
    for name in plan.to_prune {
        match deletion.remove(Path::new("collections").join(&name).as_path()) {
            Ok(was_removed) => {
                if was_removed {
                    QdrantClient::clear_stub_marker_files(qdrant_data_dir, &name);
                    removed += 1;
                }
            }
            Err(error) => {
                warn!(
                    collection = %name,
                    %error,
                    "failed to prune stale Qdrant collection dir; continuing bootstrap repair"
                );
                repair_errors.push(format!(
                    "remove stale qdrant collection dir {name}: {error}"
                ));
            }
        }
    }
    Ok((
        removed,
        collections_seen,
        prune_candidates,
        plan.overflow_protected,
    ))
}

fn execute_retention_via_http(
    qdrant: &QdrantClient,
    layout: &SidecarLayout,
    protected: &HashSet<String>,
    recency_by_collection: &HashMap<String, i64>,
    max_keep: usize,
    repair_errors: &mut Vec<String>,
) -> Result<(usize, usize, usize, bool)> {
    let all = list_http_codestory_collections(qdrant, layout, recency_by_collection)?;
    let collections_seen = all.len();
    let plan = plan_retention(&all, protected, max_keep);
    let prune_candidates = plan.to_prune.len();
    let mut removed = 0usize;
    for name in plan.to_prune {
        match qdrant.delete_collection(&name) {
            Ok(()) => {
                removed += 1;
            }
            Err(error) => {
                warn!(
                    collection = %name,
                    %error,
                    "failed to prune stale Qdrant collection; continuing bootstrap repair"
                );
                repair_errors.push(format!("delete stale qdrant collection {name}: {error}"));
            }
        }
    }
    Ok((
        removed,
        collections_seen,
        prune_candidates,
        plan.overflow_protected,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::qdrant_client::QdrantClient;
    use crate::test_support::retrieval_manifest_fixture;
    use codestory_store::RetrievalIndexManifest;
    use std::fs;
    use std::sync::Mutex;
    use std::thread;
    use std::time::Duration;
    use tempfile::tempdir;

    static PRUNE_ENV_LOCK: Mutex<()> = Mutex::new(());

    fn lock_prune_env() -> std::sync::MutexGuard<'static, ()> {
        PRUNE_ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn offline_test_layout(qdrant_data: &tempfile::TempDir) -> SidecarLayout {
        SidecarLayout {
            qdrant_http_port: 1,
            qdrant_grpc_port: 1,
            lexical_data_dir: qdrant_data.path().join("lexical"),
            qdrant_data_dir: qdrant_data.path().to_path_buf(),
            scip_artifacts_root: qdrant_data.path().join("scip"),
            state_file: qdrant_data.path().join("state.json"),
        }
    }

    fn touch_collection(collections_dir: &Path, name: &str, with_config: bool) {
        let dir = collections_dir.join(name);
        fs::create_dir_all(&dir).expect("create collection dir");
        if with_config {
            fs::write(dir.join("config.json"), "{}").expect("write config");
        }
    }

    fn manifest_for_collection(
        project_id: &str,
        qdrant_collection: &str,
    ) -> RetrievalIndexManifest {
        let mut manifest = retrieval_manifest_fixture(project_id, "hash");
        manifest.qdrant_collection = qdrant_collection.into();
        manifest
    }

    #[test]
    fn remove_invalid_collection_dirs_drops_codestory_dirs_without_config() {
        let root = tempdir().expect("tempdir");
        let collections = root.path().join("collections");
        touch_collection(&collections, "codestory_bad", false);
        touch_collection(&collections, "codestory_good", true);
        touch_collection(&collections, "other_bad", false);
        let removed = remove_invalid_collection_dirs(root.path()).expect("cleanup");
        assert_eq!(removed, 1);
        assert!(!collections.join("codestory_bad").exists());
        assert!(collections.join("codestory_good").exists());
        assert!(collections.join("other_bad").exists());
    }

    #[test]
    fn prune_on_disk_skips_protected_collections() {
        let root = tempdir().expect("tempdir");
        let collections = root.path().join("collections");
        let mut protected = HashSet::new();
        for index in 0..65 {
            let name = format!("codestory_repo_{index:02}");
            touch_collection(&collections, &name, true);
            if index == 0 {
                protected.insert(name);
            }
            thread::sleep(Duration::from_millis(5));
        }
        let (pruned, seen, _, overflow) = execute_retention_on_disk(
            root.path(),
            &protected,
            &HashMap::new(),
            64,
            &mut Vec::new(),
        )
        .expect("prune");
        assert!(pruned > 0);
        assert_eq!(seen, 65);
        assert!(!overflow);
        assert!(collections.join("codestory_repo_00").exists());
    }

    #[test]
    fn plan_retention_reports_overflow_when_all_protected() {
        let candidates: Vec<CollectionCandidate> = (0..70)
            .map(|index| CollectionCandidate {
                name: format!("codestory_{index:02}"),
                rank_key: index as u128,
            })
            .collect();
        let protected: HashSet<String> = candidates
            .iter()
            .map(|candidate| candidate.name.clone())
            .collect();
        let plan = plan_retention(&candidates, &protected, 64);
        assert!(plan.to_prune.is_empty());
        assert!(plan.overflow_protected);
    }

    #[test]
    fn collect_protected_includes_flat_custom_cache_root() {
        let global_cache = tempdir().expect("global cache");
        let custom_cache = tempdir().expect("custom cache");
        fs::create_dir_all(custom_cache.path()).expect("mkdir custom");
        let custom_db = custom_cache.path().join("codestory.db");
        let mut storage = Store::open(&custom_db).expect("open custom db");
        storage
            .upsert_retrieval_index_manifest(&manifest_for_collection(
                "custom",
                "codestory_custom_flat",
            ))
            .expect("upsert");

        let scope = BootstrapStorageScope {
            repo_root: None,
            active_storage_path: Some(custom_db),
            active_cache_root: Some(custom_cache.path().to_path_buf()),
            global_cache_root: global_cache.path().to_path_buf(),
        };
        let scan = collect_protected_collections(&scope).expect("scan");
        assert!(scan.protected.contains("codestory_custom_flat"));
    }

    #[test]
    fn collect_protected_records_corrupt_cache_db_warning() {
        let cache_root = tempdir().expect("cache");
        let corrupt_db = cache_root.path().join("codestory.db");
        fs::write(&corrupt_db, b"not sqlite").expect("write corrupt db");
        let scope = BootstrapStorageScope {
            repo_root: None,
            active_storage_path: Some(corrupt_db.clone()),
            active_cache_root: Some(cache_root.path().to_path_buf()),
            global_cache_root: cache_root.path().to_path_buf(),
        };
        let scan = collect_protected_collections(&scope).expect("scan");
        assert!(!scan.scan_errors.is_empty());
        assert!(scan.scan_errors[0].contains("open cache db"));
    }

    #[test]
    fn protected_scan_reads_hashed_manifest_databases() {
        let cache_root = tempdir().expect("cache tempdir");
        let project_cache_a = cache_root.path().join("aaaaaaaaaaaaaaaa");
        let project_cache_b = cache_root.path().join("bbbbbbbbbbbbbbbb");
        fs::create_dir_all(&project_cache_a).expect("cache a");
        fs::create_dir_all(&project_cache_b).expect("cache b");

        let mut storage_a = Store::open(project_cache_a.join("codestory.db")).expect("open a");
        storage_a
            .upsert_retrieval_index_manifest(&manifest_for_collection("a", "codestory_from_a"))
            .expect("manifest a");

        let mut storage_b = Store::open(project_cache_b.join("codestory.db")).expect("open b");
        storage_b
            .upsert_retrieval_index_manifest(&manifest_for_collection("b", "codestory_from_b"))
            .expect("manifest b");

        let scope = BootstrapStorageScope {
            repo_root: None,
            active_storage_path: None,
            active_cache_root: None,
            global_cache_root: cache_root.path().to_path_buf(),
        };
        let scan = collect_protected_collections(&scope).expect("scan manifests");
        assert!(scan.protected.contains("codestory_from_a"));
        assert!(scan.protected.contains("codestory_from_b"));
    }

    #[test]
    fn migrate_legacy_stub_marker_creates_new_path() {
        let root = tempdir().expect("tempdir");
        let collection = "codestory_legacy";
        let legacy = QdrantClient::legacy_stub_marker_path(root.path(), collection);
        fs::create_dir_all(legacy.parent().expect("parent")).expect("mkdir");
        fs::write(&legacy, "legacy").expect("write legacy");
        let migrated = migrate_legacy_stub_markers(root.path()).expect("migrate");
        assert_eq!(migrated, 1);
        assert!(QdrantClient::stub_marker_path(root.path(), collection).is_file());
        assert!(!legacy.is_file());
    }

    #[test]
    fn is_collection_stubbed_checks_legacy_path() {
        let root = tempdir().expect("tempdir");
        let collection = "codestory_x";
        let legacy = QdrantClient::legacy_stub_marker_path(root.path(), collection);
        fs::create_dir_all(legacy.parent().expect("parent")).expect("mkdir");
        fs::write(&legacy, "stub").expect("write");
        assert!(QdrantClient::is_collection_stubbed(root.path(), collection));
    }

    #[test]
    fn collect_protected_scans_flat_and_hashed_when_both_present() {
        let cache_root = tempdir().expect("cache");
        let flat_db = cache_root.path().join("codestory.db");
        let mut flat_storage = Store::open(&flat_db).expect("open flat db");
        flat_storage
            .upsert_retrieval_index_manifest(&manifest_for_collection(
                "flat",
                "codestory_flat_layout",
            ))
            .expect("flat manifest");

        let hashed_dir = cache_root.path().join("bbbbbbbbbbbbbbbb");
        fs::create_dir_all(&hashed_dir).expect("hashed dir");
        let mut hashed_storage =
            Store::open(hashed_dir.join("codestory.db")).expect("open hashed db");
        hashed_storage
            .upsert_retrieval_index_manifest(&manifest_for_collection(
                "hashed",
                "codestory_hashed_layout",
            ))
            .expect("hashed manifest");

        let scope = BootstrapStorageScope {
            repo_root: None,
            active_storage_path: None,
            active_cache_root: None,
            global_cache_root: cache_root.path().to_path_buf(),
        };
        let scan = collect_protected_collections(&scope).expect("scan both layouts");
        assert!(scan.protected.contains("codestory_flat_layout"));
        assert!(scan.protected.contains("codestory_hashed_layout"));
    }

    #[test]
    fn scan_cache_tree_records_unreadable_cache_root_without_panic() {
        let cache_root = tempdir().expect("cache");
        let missing = cache_root.path().join("nested-missing");
        let mut scanned = HashSet::new();
        let mut result = ProtectionScanResult::default();
        scan_cache_tree(&missing, &mut scanned, &mut result).expect("missing root is ok");
        assert!(result.scan_errors.is_empty());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let unreadable = cache_root.path().join("unreadable");
            fs::create_dir_all(&unreadable).expect("mkdir unreadable");
            fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o000))
                .expect("chmod unreadable");
            scan_cache_tree(&unreadable, &mut scanned, &mut result).expect("no panic");
            assert!(
                result
                    .scan_errors
                    .iter()
                    .any(|error| error.contains("read cache root")),
                "expected read cache root scan error, got {:?}",
                result.scan_errors
            );
            fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o755))
                .expect("restore permissions");
        }
    }

    #[test]
    fn repair_suppresses_prune_when_protection_scan_errors_present() {
        let _guard = lock_prune_env();
        // SAFETY: serialized env mutation for this test only.
        unsafe {
            std::env::remove_var("CODESTORY_RETRIEVAL_PRUNE_ON_SCAN_ERROR");
        }
        let cache_root = tempdir().expect("cache");
        let corrupt_db = cache_root.path().join("codestory.db");
        fs::write(&corrupt_db, b"not sqlite").expect("write corrupt db");
        let qdrant_data = tempdir().expect("qdrant data");
        let collections = qdrant_data.path().join("collections");
        fs::create_dir_all(&collections).expect("mkdir collections");
        for index in 0..65 {
            touch_collection(&collections, &format!("codestory_prune_{index:02}"), true);
        }

        let layout = offline_test_layout(&qdrant_data);
        let scope = BootstrapStorageScope {
            repo_root: None,
            active_storage_path: Some(corrupt_db),
            active_cache_root: Some(cache_root.path().to_path_buf()),
            global_cache_root: cache_root.path().to_path_buf(),
        };

        let report = repair_qdrant_storage(&layout, &scope, 64).expect("repair");
        assert!(!report.scan_errors.is_empty());
        assert_eq!(
            report.prune_suppressed_reason.as_deref(),
            Some(PRUNE_SUPPRESSED_PROTECTION_SCAN_ERROR)
        );
        assert_eq!(report.pruned_collections, 0);
        assert!(collections.join("codestory_prune_64").exists());
    }

    #[test]
    fn bootstrap_default_defers_valid_collection_pruning_until_publication() {
        let cache_root = tempdir().expect("cache");
        let qdrant_data = tempdir().expect("qdrant data");
        let collections = qdrant_data.path().join("collections");
        for index in 0..3 {
            touch_collection(
                &collections,
                &format!("codestory_deferred_{index:02}"),
                true,
            );
        }
        let scope = BootstrapStorageScope {
            repo_root: None,
            active_storage_path: None,
            active_cache_root: Some(cache_root.path().to_path_buf()),
            global_cache_root: cache_root.path().to_path_buf(),
        };

        let report = repair_qdrant_storage(
            &offline_test_layout(&qdrant_data),
            &scope,
            DEFAULT_QDRANT_COLLECTION_RETENTION,
        )
        .expect("bootstrap repair");

        assert_eq!(report.pruned_collections, 0);
        assert_eq!(report.prune_candidates, 0);
        assert_eq!(
            report.prune_suppressed_reason.as_deref(),
            Some(PRUNE_SUPPRESSED_POST_PUBLICATION_RETENTION)
        );
        for index in 0..3 {
            assert!(
                collections
                    .join(format!("codestory_deferred_{index:02}"))
                    .is_dir()
            );
        }
    }

    #[test]
    fn prune_suppression_honors_env_override() {
        let _guard = lock_prune_env();
        // SAFETY: serialized env mutation for this test only.
        unsafe {
            std::env::set_var("CODESTORY_RETRIEVAL_PRUNE_ON_SCAN_ERROR", "1");
        }
        let cache_root = tempdir().expect("cache");
        let corrupt_db = cache_root.path().join("codestory.db");
        fs::write(&corrupt_db, b"not sqlite").expect("write corrupt db");
        let qdrant_data = tempdir().expect("qdrant data");
        let collections = qdrant_data.path().join("collections");
        fs::create_dir_all(&collections).expect("mkdir collections");
        for index in 0..65 {
            touch_collection(
                &collections,
                &format!("codestory_override_{index:02}"),
                true,
            );
        }
        let layout = offline_test_layout(&qdrant_data);
        let scope = BootstrapStorageScope {
            repo_root: None,
            active_storage_path: Some(corrupt_db),
            active_cache_root: Some(cache_root.path().to_path_buf()),
            global_cache_root: cache_root.path().to_path_buf(),
        };
        let report = repair_qdrant_storage(&layout, &scope, 64).expect("repair with override");
        assert!(report.prune_suppressed_reason.is_none());
        assert!(report.pruned_collections > 0);
        // SAFETY: test env cleanup.
        unsafe {
            std::env::remove_var("CODESTORY_RETRIEVAL_PRUNE_ON_SCAN_ERROR");
        }
    }

    #[test]
    fn offline_disk_cleanup_runs_only_when_qdrant_unreachable() {
        assert!(offline_disk_cleanup_when_unreachable(false));
        assert!(!offline_disk_cleanup_when_unreachable(true));
    }

    #[test]
    fn offline_disk_cleanup_removes_invalid_dirs_when_enabled() {
        let root = tempdir().expect("tempdir");
        let collections = root.path().join("collections");
        touch_collection(&collections, "codestory_bad", false);
        let (removed_invalid, migrated) = run_offline_disk_cleanup(root.path()).expect("cleanup");
        assert_eq!(removed_invalid, 1);
        assert_eq!(migrated, 0);
        assert!(!collections.join("codestory_bad").exists());
    }
}
