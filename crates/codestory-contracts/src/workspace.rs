use std::collections::HashMap;
use std::path::PathBuf;

/// Versioned policy that permits a verified source to remain outside parser scheduling.
pub const OVERSIZED_SOURCE_POLICY_VERSION: &str = "oversized-source-v1";
/// Parser input bound shared by workspace planning and the indexer fallback guard.
pub const DEFAULT_SOURCE_FILE_BYTE_CAP: u64 = 1_000_000;

/// Content-verified oversized source classified before parser scheduling.
///
/// Project, workspace, and core-publication identity are deliberately absent here. The
/// runtime binds those identities only when the complete candidate set is published.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct OversizedSourceExclusionCandidate {
    pub normalized_path: String,
    pub content_hash: String,
    pub observed_size: u64,
    pub policy_version: String,
    pub byte_cap: u64,
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
