use crate::{FileInfo, StorageError, Store};
use codestory_contracts::workspace::StoredFileRecord;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Read-only file inventory facade.
///
/// The inventory is the storage half of incremental refresh planning. Callers
/// should pass records from this facade to `codestory-workspace` without
/// rewriting paths or mtimes; freshness comparisons depend on the stored file
/// ids and millisecond timestamps staying intact.
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
        self.storage
            .get_files()?
            .into_iter()
            .map(|file| {
                Ok(StoredFileRecord {
                    id: file.id,
                    path: file.path,
                    modification_time: file.modification_time,
                    indexed: file.indexed,
                })
            })
            .collect()
    }
}
