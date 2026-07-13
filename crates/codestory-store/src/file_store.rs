use crate::{FileInfo, StorageError, Store};
use codestory_contracts::workspace::StoredFileRecord;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Read-only file inventory facade.
///
/// The inventory is the storage half of incremental refresh planning. Callers
/// should pass records from this facade to `codestory-workspace` without
/// rewriting paths or mtimes; freshness comparisons depend on the stored file
/// ids, millisecond timestamps, and verified parser content hashes staying
/// intact.
pub struct FileStore<'a> {
    storage: &'a Store,
}

impl<'a> FileStore<'a> {
    pub(crate) fn new(storage: &'a Store) -> Self {
        Self { storage }
    }

    /// Return all stored file rows, including files that may need reindexing.
    pub fn get_files(&self) -> Result<Vec<FileInfo>, StorageError> {
        self.storage.get_files()
    }

    /// Return stored file rows keyed by exact requested paths.
    pub fn get_files_by_paths(
        &self,
        paths: &[PathBuf],
    ) -> Result<HashMap<PathBuf, FileInfo>, StorageError> {
        self.storage.get_files_by_paths(paths)
    }

    /// Return the stored file row for one path, if present.
    pub fn get_file_by_path(&self, path: &Path) -> Result<Option<FileInfo>, StorageError> {
        self.storage.get_file_by_path(path)
    }

    /// Return the compact inventory contract consumed by refresh planning.
    pub fn inventory(&self) -> Result<Vec<StoredFileRecord>, StorageError> {
        let content_hashes = self.storage.get_file_content_hashes()?;
        let retry_required_file_ids = self
            .storage
            .get_errors(None)?
            .into_iter()
            .filter_map(|error| error.file_id.map(|id| id.0))
            .collect::<HashSet<_>>();
        self.storage
            .get_files()?
            .into_iter()
            .map(|file| {
                Ok(StoredFileRecord {
                    id: file.id,
                    path: file.path,
                    modification_time: file.modification_time,
                    content_hash: content_hashes.get(&file.id).cloned(),
                    indexed: file.indexed,
                    complete: file.complete,
                    retry_required: !file.complete && retry_required_file_ids.contains(&file.id),
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FileRole;
    use codestory_contracts::graph::{ErrorInfo, IndexStep, NodeId};

    #[test]
    fn inventory_retries_file_errors_but_not_parser_partial_coverage() {
        let storage = Store::new_in_memory().expect("storage");
        for id in [1, 2] {
            let file = FileInfo {
                id,
                path: PathBuf::from(format!("src/{id}.rs")),
                language: "rust".into(),
                modification_time: 1,
                indexed: true,
                complete: false,
                line_count: 1,
                file_role: FileRole::Source,
            };
            storage.insert_file(&file).expect("file");
            if id == 1 {
                storage
                    .update_file_metadata(&file, Some("sha256-fixture"))
                    .expect("file hash");
            }
        }
        storage
            .insert_error(&ErrorInfo {
                message: "read failed".into(),
                file_id: Some(NodeId(2)),
                line: None,
                column: None,
                is_fatal: true,
                index_step: IndexStep::Indexing,
            })
            .expect("error");

        let inventory = storage.files().inventory().expect("inventory");
        assert!(
            !inventory
                .iter()
                .find(|file| file.id == 1)
                .unwrap()
                .retry_required
        );
        assert!(
            inventory
                .iter()
                .find(|file| file.id == 2)
                .unwrap()
                .retry_required
        );
        assert_eq!(
            inventory
                .iter()
                .find(|file| file.id == 1)
                .and_then(|file| file.content_hash.as_deref()),
            Some("sha256-fixture")
        );
    }
}
