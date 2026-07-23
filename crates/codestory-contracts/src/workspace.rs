use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;

/// Versioned policy that permits a verified bounded source to remain outside scheduling.
pub const OVERSIZED_SOURCE_POLICY_VERSION: &str = "bounded-source-exclusion-v2";
/// Parser input bound shared by workspace planning and the indexer fallback guard.
pub const DEFAULT_SOURCE_FILE_BYTE_CAP: u64 = 1_000_000;
/// Structural-unit bound shared by the structural collector and exclusion publication.
pub const DEFAULT_STRUCTURAL_UNIT_CAP: u64 = 2_048;
/// Process-start override for the parser input and verified exclusion boundary.
pub const SOURCE_FILE_BYTE_CAP_ENV: &str = "CODESTORY_INDEX_SOURCE_FILE_BYTE_CAP";

/// Immutable source-index policy shared by planning, parsing, publication, and reads.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct SourceIndexPolicy {
    pub policy_version: String,
    pub byte_cap: u64,
    pub structural_unit_cap: u64,
}

impl SourceIndexPolicy {
    pub fn oversized(byte_cap: u64) -> Self {
        Self {
            policy_version: OVERSIZED_SOURCE_POLICY_VERSION.to_string(),
            byte_cap: byte_cap.max(1),
            structural_unit_cap: DEFAULT_STRUCTURAL_UNIT_CAP,
        }
    }

    fn from_process_env() -> Self {
        let byte_cap = std::env::var(SOURCE_FILE_BYTE_CAP_ENV)
            .ok()
            .and_then(|raw| raw.trim().parse::<u64>().ok())
            .filter(|cap| *cap > 0)
            .unwrap_or(DEFAULT_SOURCE_FILE_BYTE_CAP);
        Self::oversized(byte_cap)
    }
}

impl Default for SourceIndexPolicy {
    fn default() -> Self {
        Self::oversized(DEFAULT_SOURCE_FILE_BYTE_CAP)
    }
}

/// Return the source-index policy captured once for this process.
pub fn process_source_index_policy() -> &'static SourceIndexPolicy {
    static POLICY: OnceLock<SourceIndexPolicy> = OnceLock::new();
    POLICY.get_or_init(SourceIndexPolicy::from_process_env)
}

/// Content-verified oversized source classified before parser scheduling.
///
/// Project, workspace, and core-publication identity are deliberately absent here. The
/// runtime binds those identities only when the complete candidate set is published.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct OversizedSourceExclusionCandidate {
    pub normalized_path: String,
    pub content_hash: String,
    pub observed_size: u64,
    /// Zero for a byte-bound exclusion; otherwise the collector-observed unit count.
    pub observed_unit_count: u64,
    pub policy_version: String,
    pub byte_cap: u64,
    pub structural_unit_cap: u64,
}

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub enum RefreshMode {
    Incremental,
    FullRefresh,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredFileState {
    pub id: i64,
    pub path: PathBuf,
    pub modification_time: i64,
    pub content_hash: Option<String>,
    pub indexed: bool,
    pub complete: bool,
    pub retry_required: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RefreshInputs {
    pub stored_files: Vec<StoredFileState>,
    /// Complete verified exclusions from the currently published core.
    ///
    /// Structurally over-bound sources intentionally have no parser-backed
    /// file row, so refresh planning must carry their exact content identity
    /// separately to avoid rediscovering unchanged exclusions as new files.
    pub policy_exclusions: Vec<OversizedSourceExclusionCandidate>,
    pub inventory: WorkspaceInventory,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexedFileRecord {
    pub file_id: i64,
    pub modification_time: i64,
    pub content_hash: Option<String>,
    pub indexed: bool,
    pub complete: bool,
    pub retry_required: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkspaceInventory {
    files: HashMap<PathBuf, IndexedFileRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefreshPlan {
    pub mode: RefreshMode,
    pub files_to_index: Vec<PathBuf>,
    pub files_to_remove: Vec<i64>,
    pub existing_file_ids: HashMap<PathBuf, i64>,
}

impl RefreshInputs {
    pub fn inventory_map(&self) -> HashMap<PathBuf, StoredFileState> {
        if !self.stored_files.is_empty() {
            return self
                .stored_files
                .iter()
                .cloned()
                .map(|file| (file.path.clone(), file))
                .collect();
        }

        self.inventory
            .files
            .clone()
            .into_iter()
            .map(|(path, record)| {
                (
                    path.clone(),
                    StoredFileState {
                        id: record.file_id,
                        path,
                        modification_time: record.modification_time,
                        content_hash: record.content_hash,
                        indexed: record.indexed,
                        complete: record.complete,
                        retry_required: record.retry_required,
                    },
                )
            })
            .collect()
    }
}

impl WorkspaceInventory {
    pub fn from_records<T>(iter: T) -> Self
    where
        T: IntoIterator<Item = (PathBuf, IndexedFileRecord)>,
    {
        Self {
            files: iter.into_iter().collect(),
        }
    }
}

pub type BuildMode = RefreshMode;
pub type RefreshExecutionPlan = RefreshPlan;
pub type RefreshInfo = RefreshPlan;
pub type StoredFileRecord = StoredFileState;
