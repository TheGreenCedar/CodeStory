//! Observation-only inventory of the process cache root.
//!
//! This module never mutates cache state. It is invoked only from the explicit
//! `cache inventory` command and must not be called from activation, status,
//! doctor, publication, or query paths.

use crate::cache_clean::{CacheCleanPlan, plan_cache_clean};
use crate::config::user_cache_root;
use anyhow::{Context, Result};
use codestory_contracts::owned_artifacts::{
    ANNOTATIONS_SIDECAR_FILE, EMBEDDED_MODEL_MATERIALIZE_LOCK_FILE,
};
use codestory_store::{SqliteDatabaseObservation, observe_sqlite_database};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

pub const CACHE_INVENTORY_SCHEMA_VERSION: u32 = 1;

const TOP_CONSUMER_LIMIT: usize = 20;
const WORKSPACE_ID_HEX_LEN: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheInventoryKind {
    ProjectCache,
    CoreGeneration,
    RetrievalGeneration,
    ModelDigest,
    VectorCache,
    Quarantine,
    Annotation,
    Temporary,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheInventoryEntry {
    pub kind: CacheInventoryKind,
    pub relative_path: String,
    pub apparent_bytes: u64,
    pub unique_bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheHardlinkGroup {
    pub native_identity: String,
    pub link_count: u64,
    pub apparent_bytes: u64,
    pub paths: Vec<String>,
}

/// One-shot report of copy-on-write clone *capability* under the cache root.
///
/// This is not a claim that extents are already shared across generations.
/// Proven sharing is reported via [`CacheInventoryReport::hardlink_deduplicated_bytes`]
/// / [`CacheInventoryReport::clone_shared_bytes`] from native file identity groups.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheCloneSharing {
    pub relative_path: String,
    pub apparent_bytes: u64,
    pub provable: bool,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheConsumer {
    pub relative_path: String,
    pub kind: CacheInventoryKind,
    pub apparent_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheInventoryReport {
    pub schema_version: u32,
    pub dry_run: bool,
    pub cache_root: String,
    pub ownership_scope: String,
    pub apparent_bytes: u64,
    pub unique_bytes: u64,
    pub hardlink_deduplicated_bytes: u64,
    pub clone_shared_bytes: u64,
    pub required_bytes: u64,
    pub reclaimable_bytes: u64,
    pub blocked_bytes: u64,
    pub entries: Vec<CacheInventoryEntry>,
    pub core_generations: Vec<CacheInventoryEntry>,
    pub retrieval_generations: Vec<CacheInventoryEntry>,
    pub models: Vec<CacheInventoryEntry>,
    pub vectors: Vec<CacheInventoryEntry>,
    pub quarantine: Vec<CacheInventoryEntry>,
    pub annotations: Vec<CacheInventoryEntry>,
    pub temporaries: Vec<CacheInventoryEntry>,
    pub unknown: Vec<CacheInventoryEntry>,
    pub hardlink_groups: Vec<CacheHardlinkGroup>,
    pub clone_sharing: Vec<CacheCloneSharing>,
    pub sqlite_databases: Vec<SqliteDatabaseObservation>,
    pub top_consumers: Vec<CacheConsumer>,
    pub clean_plan: CacheCleanPlan,
    pub errors: Vec<String>,
}

/// Build the process-wide cache inventory without mutating the cache tree.
pub fn cache_inventory() -> Result<CacheInventoryReport> {
    let cache_root = user_cache_root();
    build_cache_inventory(&cache_root)
}

fn build_cache_inventory(cache_root: &Path) -> Result<CacheInventoryReport> {
    let clean_plan = plan_cache_clean()?;
    let mut state = InventoryState::new(cache_root);
    if cache_root.is_dir() {
        state.scan_tree(cache_root, cache_root)?;
        state.probe_clone_capability_under_cache_root();
    } else {
        state.errors.push(format!(
            "cache root does not exist: {}",
            cache_root.display()
        ));
    }
    state.observe_sqlite_databases()?;
    state.finalize(clean_plan)
}

const CLONE_CAPABILITY_PROBE_DIR_PREFIX: &str = ".codestory-inventory-cow-probe-";

struct InventoryState {
    cache_root: PathBuf,
    entries: Vec<CacheInventoryEntry>,
    sqlite_paths: Vec<PathBuf>,
    file_identities: HashMap<String, FileIdentityRecord>,
    hardlink_groups: BTreeMap<String, CacheHardlinkGroup>,
    clone_sharing: Vec<CacheCloneSharing>,
    sqlite_databases: Vec<SqliteDatabaseObservation>,
    errors: Vec<String>,
}

struct FileIdentityRecord {
    apparent_bytes: u64,
    link_count: u64,
}

impl InventoryState {
    fn new(cache_root: &Path) -> Self {
        Self {
            cache_root: cache_root.to_path_buf(),
            entries: Vec::new(),
            sqlite_paths: Vec::new(),
            file_identities: HashMap::new(),
            hardlink_groups: BTreeMap::new(),
            clone_sharing: Vec::new(),
            sqlite_databases: Vec::new(),
            errors: Vec::new(),
        }
    }

    fn scan_tree(&mut self, root: &Path, dir: &Path) -> Result<()> {
        let read_dir = std::fs::read_dir(dir)
            .with_context(|| format!("read cache directory {}", dir.display()))?;
        for entry in read_dir {
            let entry = entry.with_context(|| format!("read entry under {}", dir.display()))?;
            let path = entry.path();
            let metadata = std::fs::symlink_metadata(&path)
                .with_context(|| format!("inspect {}", path.display()))?;
            if metadata.file_type().is_symlink() {
                self.push_unknown(&relative_path(root, &path)?, 0, "symlink entry");
                continue;
            }
            if metadata.is_dir() {
                self.scan_tree(root, &path)?;
                continue;
            }
            if !metadata.is_file() {
                self.push_unknown(&relative_path(root, &path)?, 0, "unsupported entry type");
                continue;
            }
            let relative = relative_path(root, &path)?;
            let apparent_bytes = metadata.len();
            self.record_file(&relative, apparent_bytes, metadata)?;
            if path.extension().is_some_and(|ext| ext == "db")
                || path.file_name().is_some_and(|name| {
                    name == "codestory.db" || name.to_string_lossy().ends_with(".sqlite3")
                })
            {
                self.sqlite_paths.push(path.clone());
            }
            let kind = classify_entry(&relative, &path);
            self.entries.push(CacheInventoryEntry {
                kind,
                relative_path: relative,
                apparent_bytes,
                unique_bytes: apparent_bytes,
                detail: None,
            });
        }
        Ok(())
    }

    fn record_file(
        &mut self,
        relative: &str,
        apparent_bytes: u64,
        metadata: std::fs::Metadata,
    ) -> Result<()> {
        let identity = native_file_identity(&metadata)?;
        let record = self
            .file_identities
            .entry(identity.clone())
            .or_insert_with(|| FileIdentityRecord {
                apparent_bytes,
                link_count: 0,
            });
        record.link_count = record.link_count.saturating_add(1);
        let group = self
            .hardlink_groups
            .entry(identity.clone())
            .or_insert_with(|| CacheHardlinkGroup {
                native_identity: identity.clone(),
                link_count: 0,
                apparent_bytes,
                paths: Vec::new(),
            });
        group.link_count = group.link_count.saturating_add(1);
        group.paths.push(relative.to_string());
        Ok(())
    }

    /// Probe CoW clone capability once under the cache root, then remove the
    /// probe tree. Never writes outside the process cache root.
    fn probe_clone_capability_under_cache_root(&mut self) {
        let probe_dir = self.cache_root.join(format!(
            "{CLONE_CAPABILITY_PROBE_DIR_PREFIX}{}",
            std::process::id()
        ));
        let relative = relative_path(&self.cache_root, &probe_dir).unwrap_or_else(|_| {
            CLONE_CAPABILITY_PROBE_DIR_PREFIX
                .trim_end_matches('-')
                .into()
        });
        let _ = std::fs::remove_dir_all(&probe_dir);
        if let Err(error) = std::fs::create_dir_all(&probe_dir) {
            self.clone_sharing.push(CacheCloneSharing {
                relative_path: relative,
                apparent_bytes: 0,
                provable: false,
                detail: format!("could not create cache-root CoW capability probe: {error}"),
            });
            return;
        }
        let source = probe_dir.join("source.bin");
        let destination = probe_dir.join("clone.bin");
        let capability = match std::fs::write(&source, b"codestory-inventory-cow-probe") {
            Ok(()) => crate::copy_on_write::clone_file(&source, &destination).unwrap_or(false),
            Err(error) => {
                let _ = std::fs::remove_dir_all(&probe_dir);
                self.clone_sharing.push(CacheCloneSharing {
                    relative_path: relative,
                    apparent_bytes: 0,
                    provable: false,
                    detail: format!("could not seed cache-root CoW capability probe: {error}"),
                });
                return;
            }
        };
        let _ = std::fs::remove_dir_all(&probe_dir);
        self.clone_sharing.push(CacheCloneSharing {
            relative_path: relative,
            apparent_bytes: 0,
            provable: capability,
            detail: if capability {
                "copy-on-write clone capability available under cache root".into()
            } else {
                "copy-on-write clone capability unavailable under cache root".into()
            },
        });
    }

    fn observe_sqlite_databases(&mut self) -> Result<()> {
        for path in self.sqlite_paths.clone() {
            match observe_sqlite_database(&path) {
                Ok(observation) => self.sqlite_databases.push(observation),
                Err(error) => self
                    .errors
                    .push(format!("observe sqlite {}: {error}", path.display())),
            }
        }
        Ok(())
    }

    fn finalize(self, clean_plan: CacheCleanPlan) -> Result<CacheInventoryReport> {
        let unique_bytes = self
            .file_identities
            .values()
            .map(|record| record.apparent_bytes)
            .sum();
        let apparent_bytes = self.entries.iter().map(|entry| entry.apparent_bytes).sum();
        let hardlink_deduplicated_bytes = self
            .hardlink_groups
            .values()
            .filter(|group| group.link_count > 1)
            .map(|group| {
                group
                    .apparent_bytes
                    .saturating_mul(group.link_count.saturating_sub(1))
            })
            .sum();
        // Proven sharing only: hardlink / native-identity groups. A successful
        // CoW capability probe is not evidence that extents are already shared.
        let clone_shared_bytes = hardlink_deduplicated_bytes;

        let mut top_consumers = self
            .entries
            .iter()
            .map(|entry| CacheConsumer {
                relative_path: entry.relative_path.clone(),
                kind: entry.kind.clone(),
                apparent_bytes: entry.apparent_bytes,
            })
            .collect::<Vec<_>>();
        top_consumers.sort_by(|left, right| {
            right
                .apparent_bytes
                .cmp(&left.apparent_bytes)
                .then_with(|| left.relative_path.cmp(&right.relative_path))
        });
        top_consumers.truncate(TOP_CONSUMER_LIMIT);

        let core_generations = filter_kind(&self.entries, CacheInventoryKind::CoreGeneration);
        let retrieval_generations =
            filter_kind(&self.entries, CacheInventoryKind::RetrievalGeneration);
        let models = filter_kind(&self.entries, CacheInventoryKind::ModelDigest);
        let vectors = filter_kind(&self.entries, CacheInventoryKind::VectorCache);
        let quarantine = filter_kind(&self.entries, CacheInventoryKind::Quarantine);
        let annotations = filter_kind(&self.entries, CacheInventoryKind::Annotation);
        let temporaries = filter_kind(&self.entries, CacheInventoryKind::Temporary);
        let unknown = filter_kind(&self.entries, CacheInventoryKind::Unknown);

        let required_bytes = self
            .entries
            .iter()
            .filter(|entry| {
                matches!(
                    entry.kind,
                    CacheInventoryKind::ProjectCache
                        | CacheInventoryKind::CoreGeneration
                        | CacheInventoryKind::RetrievalGeneration
                        | CacheInventoryKind::ModelDigest
                        | CacheInventoryKind::VectorCache
                        | CacheInventoryKind::Annotation
                )
            })
            .map(|entry| entry.apparent_bytes)
            .sum();
        let reclaimable_bytes = clean_plan.reclaimable_bytes;
        // Clean plan retains workspace/model *directories*; inventory entries
        // are files. Roll up every entry under each retained prefix.
        let blocked_bytes = self
            .entries
            .iter()
            .filter(|entry| {
                clean_plan.retained.iter().any(|retained| {
                    path_is_under_retained(&entry.relative_path, &retained.relative_path)
                })
            })
            .map(|entry| entry.apparent_bytes)
            .sum();

        Ok(CacheInventoryReport {
            schema_version: CACHE_INVENTORY_SCHEMA_VERSION,
            dry_run: true,
            cache_root: self.cache_root.display().to_string(),
            ownership_scope: "process_cache_root".into(),
            apparent_bytes,
            unique_bytes,
            hardlink_deduplicated_bytes,
            clone_shared_bytes,
            required_bytes,
            reclaimable_bytes,
            blocked_bytes,
            entries: self.entries,
            core_generations,
            retrieval_generations,
            models,
            vectors,
            quarantine,
            annotations,
            temporaries,
            unknown,
            hardlink_groups: self.hardlink_groups.into_values().collect(),
            clone_sharing: self.clone_sharing,
            sqlite_databases: self.sqlite_databases,
            top_consumers,
            clean_plan,
            errors: self.errors,
        })
    }

    fn push_unknown(&mut self, relative: &str, apparent_bytes: u64, detail: &str) {
        self.entries.push(CacheInventoryEntry {
            kind: CacheInventoryKind::Unknown,
            relative_path: relative.to_string(),
            apparent_bytes,
            unique_bytes: apparent_bytes,
            detail: Some(detail.into()),
        });
    }
}

fn filter_kind(
    entries: &[CacheInventoryEntry],
    kind: CacheInventoryKind,
) -> Vec<CacheInventoryEntry> {
    entries
        .iter()
        .filter(|entry| entry.kind == kind)
        .cloned()
        .collect()
}

fn relative_path(root: &Path, path: &Path) -> Result<String> {
    path.strip_prefix(root)
        .with_context(|| {
            format!(
                "path {} is outside cache root {}",
                path.display(),
                root.display()
            )
        })
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
}

fn path_is_under_retained(entry_relative: &str, retained_relative: &str) -> bool {
    entry_relative == retained_relative
        || entry_relative.starts_with(&format!("{retained_relative}/"))
}

fn classify_entry(relative: &str, path: &Path) -> CacheInventoryKind {
    let components: Vec<_> = relative.split('/').collect();
    if components
        .iter()
        .any(|component| component.contains("quarantine"))
    {
        return CacheInventoryKind::Quarantine;
    }
    if relative.contains(EMBEDDED_MODEL_MATERIALIZE_LOCK_FILE) {
        return CacheInventoryKind::Temporary;
    }
    if relative.ends_with(ANNOTATIONS_SIDECAR_FILE) {
        return CacheInventoryKind::Annotation;
    }
    if relative.contains("/core/generations/") && relative.ends_with("/codestory.db") {
        return CacheInventoryKind::CoreGeneration;
    }
    if relative.contains("/core/staging/") {
        return CacheInventoryKind::Temporary;
    }
    if relative.contains("embedded-models/sha256/") {
        return CacheInventoryKind::ModelDigest;
    }
    if relative.contains("vectors/")
        || relative.contains("semantic/")
        || relative.contains("lexical/")
    {
        return CacheInventoryKind::RetrievalGeneration;
    }
    if components.len() == 2
        && components[0].len() == WORKSPACE_ID_HEX_LEN
        && components[1] == "codestory.db"
    {
        return CacheInventoryKind::ProjectCache;
    }
    if path.file_name().is_some_and(|name| name == "codestory.db") {
        return CacheInventoryKind::ProjectCache;
    }
    if relative.contains("retention") || relative.contains("generation") {
        return CacheInventoryKind::RetrievalGeneration;
    }
    CacheInventoryKind::Unknown
}

fn native_file_identity(metadata: &std::fs::Metadata) -> Result<String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(format!("{}:{}", metadata.dev(), metadata.ino()))
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        Ok(format!(
            "{}:{}",
            metadata.volume_serial_number(),
            metadata.file_index().unwrap_or(0)
        ))
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = metadata;
        anyhow::bail!("native file identity is unsupported on this platform")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{RETRIEVAL_STATE_FILE, with_test_cache_root};
    use crate::retention::{GenerationRetentionMarker, write_retention_marker};
    use codestory_store::{RetrievalIndexManifest, Store};
    use sha2::{Digest, Sha256};
    use tempfile::tempdir;

    #[test]
    fn cache_inventory_is_observation_only() {
        let cache = tempdir().expect("cache root");
        let workspace_id = "00112233445566aa";
        let project_cache = cache.path().join(workspace_id);
        let storage = project_cache.join("codestory.db");
        std::fs::create_dir_all(&project_cache).expect("create project cache");
        let store = Store::open(&storage).expect("create store");
        drop(store);
        let before = snapshot_tree(cache.path());
        let before_temp = inventory_temp_probe_names();

        let report = with_test_cache_root(cache.path(), cache_inventory).expect("inventory");
        let after = snapshot_tree(cache.path());
        let after_temp = inventory_temp_probe_names();

        assert_eq!(before, after, "inventory must not mutate the cache tree");
        assert_eq!(
            before_temp, after_temp,
            "inventory must not write CoW probes into the system temp directory"
        );
        assert!(report.dry_run);
        assert_eq!(report.ownership_scope, "process_cache_root");
        assert_eq!(
            report.clone_sharing.len(),
            1,
            "inventory should report CoW capability once: {:?}",
            report.clone_sharing
        );
        assert_eq!(
            report.clone_sharing[0].apparent_bytes, 0,
            "capability probes must not claim shared bytes"
        );
        assert_eq!(
            report.clone_shared_bytes, report.hardlink_deduplicated_bytes,
            "clone_shared_bytes must equal identity-proven hardlink sharing"
        );
        assert!(
            report
                .entries
                .iter()
                .any(|entry| entry.kind == CacheInventoryKind::ProjectCache),
            "project cache should be classified: {:?}",
            report.entries
        );
        assert!(
            report
                .sqlite_databases
                .iter()
                .any(|db| db.path.ends_with("codestory.db")),
            "sqlite databases should be observed"
        );
        assert!(
            !after
                .keys()
                .any(|path| path.contains(CLONE_CAPABILITY_PROBE_DIR_PREFIX)),
            "capability probe directory must be removed: {:?}",
            after.keys().collect::<Vec<_>>()
        );
    }

    fn snapshot_tree(root: &Path) -> BTreeMap<String, String> {
        let mut entries = BTreeMap::new();
        collect_tree(root, root, &mut entries);
        entries
    }

    fn collect_tree(root: &Path, dir: &Path, entries: &mut BTreeMap<String, String>) {
        let Ok(read_dir) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in read_dir.flatten() {
            let path = entry.path();
            let relative = path
                .strip_prefix(root)
                .expect("entry below root")
                .to_string_lossy()
                .replace('\\', "/");
            let metadata = std::fs::symlink_metadata(&path).expect("inspect entry");
            if metadata.is_dir() {
                entries.insert(format!("{relative}/"), "<dir>".into());
                collect_tree(root, &path, entries);
            } else {
                let digest = Sha256::digest(std::fs::read(&path).expect("read entry"));
                entries.insert(relative, format!("{digest:x}"));
            }
        }
    }

    fn inventory_temp_probe_names() -> BTreeMap<String, u64> {
        let mut names = BTreeMap::new();
        let Ok(read_dir) = std::fs::read_dir(std::env::temp_dir()) else {
            return names;
        };
        for entry in read_dir.flatten() {
            let name = entry.file_name().to_string_lossy().replace('\\', "/");
            if name.contains("codestory-inventory-clone-probe")
                || name.contains("codestory-inventory-cow-probe")
            {
                let len = entry.metadata().map(|meta| meta.len()).unwrap_or(0);
                names.insert(name, len);
            }
        }
        names
    }

    #[test]
    fn hardlink_deduplication_is_reported() {
        let cache = tempdir().expect("cache root");
        let workspace_id = "00112233445566aa";
        let project_cache = cache.path().join(workspace_id);
        std::fs::create_dir_all(&project_cache).expect("create project cache");
        let owned = project_cache.join("codestory.db");
        std::fs::write(&owned, b"database").expect("write database");
        std::fs::hard_link(&owned, project_cache.join("alias.db")).expect("hard link");

        let report = with_test_cache_root(cache.path(), cache_inventory).expect("inventory");
        assert!(
            report.hardlink_deduplicated_bytes >= 8,
            "hardlink dedup should be reported: {:?}",
            report.hardlink_groups
        );
        assert_eq!(
            report.clone_shared_bytes, report.hardlink_deduplicated_bytes,
            "shared bytes must come from identity groups, not CoW capability probes"
        );
        assert!(
            report
                .clone_sharing
                .iter()
                .all(|entry| entry.apparent_bytes == 0),
            "capability entries must not inflate shared bytes: {:?}",
            report.clone_sharing
        );
    }

    #[test]
    fn blocked_bytes_rolls_up_retained_workspace_directory() {
        let cache = tempdir().expect("cache root");
        let worktree = tempdir().expect("live worktree");
        let workspace_id = "00112233445566aa";
        let project_cache = cache.path().join(workspace_id);
        std::fs::create_dir_all(&project_cache).expect("create project cache");
        let payload = b"retained-workspace-payload-bytes";
        std::fs::write(project_cache.join("codestory.db"), payload).expect("write database");
        std::fs::write(project_cache.join("extra.bin"), b"extra-bytes").expect("write extra");

        let marker = GenerationRetentionMarker::next(
            workspace_id,
            worktree.path(),
            RetrievalIndexManifest {
                project_id: "repo-v1-project".into(),
                lexical_version: "v1".into(),
                semantic_generation: "codestory_repo-v1-project_aaaaaaaaaaaaaaaa".into(),
                scip_revision: Some("graph-aaaaaaaaaaaaaaaa".into()),
                built_at_epoch_ms: 1,
                disk_bytes: None,
                degraded_modes_json: "[]".into(),
                embedding_backend: Some(crate::embeddings::PRODUCT_EMBEDDING_RUNTIME_ID.into()),
                embedding_dim: Some(768),
                sidecar_schema_version: Some(2),
                sidecar_input_hash: Some("aaaaaaaaaaaaaaaa".repeat(1)),
                sidecar_generation: Some("repo-v1-project-aaaaaaaaaaaaaaaa".into()),
                projection_count: Some(1),
                symbol_doc_count: Some(1),
                dense_projection_count: Some(1),
                semantic_policy_version: Some(crate::generation::SEMANTIC_POLICY_VERSION.into()),
                graph_artifact_hash: Some("graph".into()),
                dense_reason_counts_json: Some("{}".into()),
                precise_semantic_import_status: None,
                precise_semantic_import_reason: None,
                precise_semantic_import_revision: None,
                precise_semantic_import_producer: None,
            },
            None,
            1,
        )
        .expect("registration marker");
        write_retention_marker(&cache.path().join(RETRIEVAL_STATE_FILE), &marker)
            .expect("write marker");

        let before_temp = inventory_temp_probe_names();
        let report = with_test_cache_root(cache.path(), cache_inventory).expect("inventory");
        let after_temp = inventory_temp_probe_names();

        assert_eq!(
            before_temp, after_temp,
            "inventory must not leave CoW probes in system temp"
        );
        assert!(
            report
                .clean_plan
                .retained
                .iter()
                .any(|retained| retained.relative_path == workspace_id),
            "live workspace directory should be retained: {:?}",
            report.clean_plan.retained
        );
        let expected_blocked = payload.len() as u64 + b"extra-bytes".len() as u64;
        assert!(
            report.blocked_bytes >= expected_blocked,
            "blocked_bytes should roll up files under retained directory; got {} want >= {expected_blocked}; entries={:?}",
            report.blocked_bytes,
            report.entries
        );
        assert_eq!(
            path_is_under_retained("00112233445566aa/codestory.db", "00112233445566aa"),
            true
        );
        assert_eq!(
            path_is_under_retained("00112233445566aa", "00112233445566aa"),
            true
        );
        assert_eq!(
            path_is_under_retained("00112233445566ab/codestory.db", "00112233445566aa"),
            false
        );
    }
}
