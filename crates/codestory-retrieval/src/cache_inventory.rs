//! Observation-only inventory of the process cache root.
//!
//! This module never mutates cache state. It is invoked only from the explicit
//! `cache inventory` command and must not be called from activation, status,
//! doctor, publication, or query paths.

use crate::cache_clean::{CacheCleanPlan, plan_cache_clean};
use crate::config::user_cache_root;
use anyhow::Result;
use codestory_contracts::owned_artifacts::{
    ANNOTATIONS_SIDECAR_FILE, EMBEDDED_MODEL_CACHE_DIR, EMBEDDED_MODEL_DIGEST_DIR,
    EMBEDDED_MODEL_MATERIALIZE_LOCK_FILE,
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
    /// A directory or entry the scan could not read. Its contents are absent
    /// from every byte total, so those totals are lower bounds.
    Unreadable,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheInventoryEntry {
    pub kind: CacheInventoryKind,
    pub relative_path: String,
    pub apparent_bytes: u64,
    pub unique_bytes: u64,
    /// Bytes the filesystem reports as allocated for this file, where the
    /// platform exposes that. `None` means allocation is unobservable here, not
    /// that the file occupies nothing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allocated_bytes: Option<u64>,
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

/// One file whose allocated size is smaller than its apparent size.
///
/// The shortfall is measured, but its cause is not: copy-on-write extent
/// sharing, sparse regions, and filesystem compression all produce it. Nothing
/// here claims a specific cause, and nothing here is a capability probe.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheCloneSharing {
    pub relative_path: String,
    pub apparent_bytes: u64,
    pub allocated_bytes: u64,
    pub unallocated_bytes: u64,
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
    /// Summed per-file allocation over distinct native identities, where the
    /// platform reports it. `None` means this platform exposes no allocation
    /// evidence, so no allocation claim is made at all.
    pub allocated_bytes: Option<u64>,
    pub hardlink_deduplicated_bytes: u64,
    /// Apparent bytes with no distinct allocation behind them, measured as the
    /// shortfall between apparent and allocated size. Copy-on-write sharing,
    /// sparse regions, and compression all contribute; this does not attribute
    /// the shortfall to any one of them, and it is never inferred from hardlink
    /// counts or from a write probe.
    pub clone_shared_bytes: Option<u64>,
    /// Whether an unreadable entry kept the scan from seeing the whole tree.
    /// When true every byte total below is a lower bound.
    pub partial_scan: bool,
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
    pub unreadable: Vec<CacheInventoryEntry>,
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
        state.scan_tree(cache_root, cache_root);
    } else {
        state.errors.push(format!(
            "cache root does not exist: {}",
            cache_root.display()
        ));
    }
    state.observe_sqlite_databases();
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
    partial_scan: bool,
    errors: Vec<String>,
}

struct FileIdentityRecord {
    apparent_bytes: u64,
    allocated_bytes: Option<u64>,
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
            partial_scan: false,
            errors: Vec::new(),
        }
    }

    /// Record one entry the scan could not read and continue. An unreadable
    /// subtree is a scoped hole in the observation, never a proof that the
    /// subtree is empty or reclaimable, so it also marks the scan partial.
    fn push_unreadable(&mut self, relative: String, detail: String) {
        self.partial_scan = true;
        self.errors.push(format!("{relative}: {detail}"));
        self.entries.push(CacheInventoryEntry {
            kind: CacheInventoryKind::Unreadable,
            relative_path: relative,
            apparent_bytes: 0,
            unique_bytes: 0,
            allocated_bytes: None,
            detail: Some(detail),
        });
    }

    fn scan_tree(&mut self, root: &Path, dir: &Path) {
        let dir_relative = display_relative(root, dir);
        let read_dir = match std::fs::read_dir(dir) {
            Ok(read_dir) => read_dir,
            Err(error) => {
                self.push_unreadable(dir_relative, format!("read cache directory: {error}"));
                return;
            }
        };
        for entry in read_dir {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    self.push_unreadable(
                        dir_relative.clone(),
                        format!("read entry under directory: {error}"),
                    );
                    continue;
                }
            };
            let path = entry.path();
            let relative = display_relative(root, &path);
            let metadata = match std::fs::symlink_metadata(&path) {
                Ok(metadata) => metadata,
                Err(error) => {
                    self.push_unreadable(relative, format!("inspect entry: {error}"));
                    continue;
                }
            };
            if metadata.file_type().is_symlink() {
                self.push_unknown(&relative, 0, "symlink entry");
                continue;
            }
            if metadata.is_dir() {
                self.scan_tree(root, &path);
                continue;
            }
            if !metadata.is_file() {
                self.push_unknown(&relative, 0, "unsupported entry type");
                continue;
            }
            let apparent_bytes = metadata.len();
            let allocated_bytes = allocated_file_bytes(&metadata);
            match native_file_identity(&metadata) {
                Ok(identity) => {
                    self.record_file(&relative, apparent_bytes, allocated_bytes, identity)
                }
                Err(error) => {
                    self.push_unreadable(relative.clone(), format!("native identity: {error}"));
                    continue;
                }
            }
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
                unique_bytes: allocated_bytes.unwrap_or(apparent_bytes),
                allocated_bytes,
                detail: None,
            });
        }
    }

    fn record_file(
        &mut self,
        relative: &str,
        apparent_bytes: u64,
        allocated_bytes: Option<u64>,
        identity: String,
    ) {
        let record = self
            .file_identities
            .entry(identity.clone())
            .or_insert_with(|| FileIdentityRecord {
                apparent_bytes,
                allocated_bytes,
                link_count: 0,
            });
        record.link_count = record.link_count.saturating_add(1);
        if let Some(allocated) = allocated_bytes
            && allocated < apparent_bytes
            && record.link_count == 1
        {
            self.clone_sharing.push(CacheCloneSharing {
                relative_path: relative.to_string(),
                apparent_bytes,
                allocated_bytes: allocated,
                unallocated_bytes: apparent_bytes.saturating_sub(allocated),
                detail: "apparent size exceeds allocated size; the filesystem is sharing, sparsifying, or compressing these bytes".into(),
            });
        }
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
    }

    fn observe_sqlite_databases(&mut self) {
        for path in self.sqlite_paths.clone() {
            match observe_sqlite_database(&path) {
                Ok(observation) => self.sqlite_databases.push(observation),
                Err(error) => self
                    .errors
                    .push(format!("observe sqlite {}: {error}", path.display())),
            }
        }
    }

    fn finalize(self, clean_plan: CacheCleanPlan) -> Result<CacheInventoryReport> {
        let apparent_unique_bytes: u64 = self
            .file_identities
            .values()
            .map(|record| record.apparent_bytes)
            .sum();
        // Allocation is all-or-nothing evidence: one file without it makes the
        // total a claim the platform cannot support, so the whole figure drops
        // to `None` rather than silently mixing apparent and allocated bytes.
        let allocated_bytes = self
            .file_identities
            .values()
            .map(|record| record.allocated_bytes)
            .try_fold(0_u64, |total, allocated| {
                allocated.map(|allocated| total.saturating_add(allocated))
            });
        let unique_bytes = allocated_bytes.unwrap_or(apparent_unique_bytes);
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
        // Measured allocation shortfall over distinct files. Hardlink counts
        // prove aliasing, not extent sharing, so they never feed this figure.
        let clone_shared_bytes =
            allocated_bytes.map(|allocated| apparent_unique_bytes.saturating_sub(allocated));

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
        let unreadable = filter_kind(&self.entries, CacheInventoryKind::Unreadable);
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
            // Inventory only ever plans, so it reports the plan's own mode
            // rather than asserting a mode of its own.
            dry_run: clean_plan.dry_run,
            cache_root: self.cache_root.display().to_string(),
            ownership_scope: "process_cache_root".into(),
            apparent_bytes,
            unique_bytes,
            allocated_bytes,
            hardlink_deduplicated_bytes,
            clone_shared_bytes,
            partial_scan: self.partial_scan,
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
            unreadable,
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
            allocated_bytes: None,
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

/// The cache-root-relative label for one path. A path outside the root can only
/// come from the root itself, which labels as the empty string; everything else
/// falls back to the absolute path rather than dropping the entry.
fn display_relative(root: &Path, path: &Path) -> String {
    match path.strip_prefix(root) {
        Ok(relative) if relative.as_os_str().is_empty() => ".".into(),
        Ok(relative) => relative.to_string_lossy().replace('\\', "/"),
        Err(_) => path.to_string_lossy().replace('\\', "/"),
    }
}

/// Bytes the filesystem has actually allocated for one file, where the platform
/// reports it. Windows exposes no allocation size through `std`, so allocation
/// evidence is simply absent there rather than approximated.
fn allocated_file_bytes(metadata: &std::fs::Metadata) -> Option<u64> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Some(metadata.blocks().saturating_mul(512))
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        None
    }
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
    if relative.contains(&format!(
        "{EMBEDDED_MODEL_CACHE_DIR}/{EMBEDDED_MODEL_DIGEST_DIR}/"
    )) {
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
        assert!(
            !report.partial_scan,
            "a fully readable cache root must not report a partial scan: {:?}",
            report.errors
        );
        assert!(
            report
                .clone_sharing
                .iter()
                .all(|entry| entry.unallocated_bytes > 0
                    && entry.allocated_bytes < entry.apparent_bytes),
            "clone sharing rows must carry a measured allocation shortfall: {:?}",
            report.clone_sharing
        );
        if cfg!(unix) {
            let clone_shared = report
                .clone_shared_bytes
                .expect("unix reports allocation evidence");
            assert_eq!(
                clone_shared,
                report
                    .clone_sharing
                    .iter()
                    .map(|entry| entry.unallocated_bytes)
                    .sum::<u64>(),
                "shared bytes must be the measured shortfall, not a hardlink count"
            );
            assert!(
                report.allocated_bytes.is_some(),
                "unix inventory must report allocation"
            );
        } else {
            assert_eq!(
                report.clone_shared_bytes, None,
                "a platform without allocation evidence must make no sharing claim"
            );
        }
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
            !after.keys().any(|path| path.contains("probe")),
            "inventory must not create a probe under the cache root at all: {:?}",
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
        // Two names for eight bytes is aliasing, not extent sharing. The
        // previous report equated the two and claimed shared bytes that the
        // filesystem had never shared.
        assert_ne!(
            report.clone_shared_bytes,
            Some(report.hardlink_deduplicated_bytes),
            "hardlink aliasing must not be restated as clone sharing"
        );
        assert!(
            report
                .clone_sharing
                .iter()
                .all(|entry| entry.allocated_bytes < entry.apparent_bytes),
            "every sharing row must rest on measured allocation: {:?}",
            report.clone_sharing
        );
    }

    /// A directory the scan cannot open is a hole in the observation. Aborting
    /// the whole inventory hides everything else, and skipping it silently
    /// would let an unreadable subtree read as empty.
    #[cfg(unix)]
    #[test]
    fn an_unreadable_subtree_is_scoped_rather_than_fatal() {
        use std::os::unix::fs::PermissionsExt;

        let cache = tempdir().expect("cache root");
        let workspace_id = "00112233445566aa";
        let project_cache = cache.path().join(workspace_id);
        std::fs::create_dir_all(&project_cache).expect("create project cache");
        std::fs::write(project_cache.join("codestory.db"), b"readable-payload")
            .expect("write readable database");
        let locked = cache.path().join("locked");
        std::fs::create_dir_all(&locked).expect("create locked directory");
        std::fs::write(locked.join("hidden.bin"), b"hidden").expect("write hidden payload");
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000))
            .expect("remove directory read permission");

        let report = with_test_cache_root(cache.path(), cache_inventory);
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755))
            .expect("restore directory permission");
        let report = report.expect("an unreadable subtree must not fail the inventory");

        assert!(
            report.partial_scan,
            "an unreadable subtree makes every byte total a lower bound"
        );
        assert!(
            report
                .unreadable
                .iter()
                .any(|entry| entry.relative_path == "locked"),
            "the unreadable directory must be scoped as its own entry: {:?}",
            report.unreadable
        );
        assert!(
            report
                .entries
                .iter()
                .any(|entry| entry.relative_path == format!("{workspace_id}/codestory.db")),
            "the readable remainder of the tree must still be inventoried: {:?}",
            report.entries
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
