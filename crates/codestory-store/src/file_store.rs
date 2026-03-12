use crate::{FileInfo, StorageError, Store};
use codestory_workspace::StoredFileRecord;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub struct FileStore<'a> {
    storage: &'a Store,
}

impl<'a> FileStore<'a> {
    pub(crate) fn new(storage: &'a Store) -> Self {
        Self { storage }
    }

    pub fn get_files(&self) -> Result<Vec<FileInfo>, StorageError> {
        self.storage.get_files()
    }

    pub fn get_files_by_paths(
        &self,
        paths: &[PathBuf],
    ) -> Result<HashMap<PathBuf, FileInfo>, StorageError> {
        self.storage.get_files_by_paths(paths)
    }

    pub fn get_file_by_path(&self, path: &Path) -> Result<Option<FileInfo>, StorageError> {
        self.storage.get_file_by_path(path)
    }

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
