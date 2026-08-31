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
    } else {
        state.errors.push(format!(
            "cache root does not exist: {}",
            cache_root.display()
        ));
    }
    state.observe_sqlite_databases()?;
    state.finalize(clean_plan)
}

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
        let record = self.file_identities.entry(identity.clone()).or_insert_with(|| {
            FileIdentityRecord {
                apparent_bytes,
                link_count: 0,
            }
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
        if record.link_count == 1 {
            let probe = std::env::temp_dir().join(format!(
                "codestory-inventory-clone-probe-{}-{relative}",
                std::process::id()
            ));
            if crate::copy_on_write::clone_file(self.cache_root.join(relative).as_path(), &probe)
                .unwrap_or(false)
            {
                let _ = std::fs::remove_file(probe);
                self.clone_sharing.push(CacheCloneSharing {
                    relative_path: relative.to_string(),
                    apparent_bytes,
                    provable: true,
                    detail: "copy-on-write clone succeeded during inventory probe".into(),
                });
            }
        }
        Ok(())
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
            .map(|group| group.apparent_bytes.saturating_mul(group.link_count.saturating_sub(1)))
            .sum();
        let clone_shared_bytes = self
            .clone_sharing
            .iter()
            .filter(|entry| entry.provable)
            .map(|entry| entry.apparent_bytes)
            .sum();

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
        let blocked_bytes = clean_plan
            .retained
            .iter()
            .filter_map(|retained| {
                self.entries
                    .iter()
                    .find(|entry| entry.relative_path == retained.relative_path)
                    .map(|entry| entry.apparent_bytes)
            })
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

fn filter_kind(entries: &[CacheInventoryEntry], kind: CacheInventoryKind) -> Vec<CacheInventoryEntry> {
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

fn classify_entry(relative: &str, path: &Path) -> CacheInventoryKind {
    let components: Vec<_> = relative.split('/').collect();
    if components.iter().any(|component| component.contains("quarantine")) {
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
    use crate::config::with_test_cache_root;
    use codestory_store::Store;
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

        let report = with_test_cache_root(cache.path(), cache_inventory).expect("inventory");
        let after = snapshot_tree(cache.path());

        assert_eq!(before, after, "inventory must not mutate the cache tree");
        assert!(report.dry_run);
        assert_eq!(report.ownership_scope, "process_cache_root");
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
    }
}
