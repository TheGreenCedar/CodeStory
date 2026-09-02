//! Named core pin and publication protocol.
//!
//! Readers pin one complete generation. Writers stage a complete generation
//! and swap the pointer atomically. Live WAL images are never opened as
//! immutable generations. Direct pointer and rehydrate publishers are crate-
//! private; the only public write path is this transaction.

use crate::core_generation::{
    CorePublicationCommitV1, CorePublicationLayout, publish_rehydrated_generation,
};
use crate::storage_impl::{Storage as Store, StorageError};
use codestory_contracts::core_publication::{CoreGenerationIdentityV1, CorePublicationPointerV1};
use std::path::{Path, PathBuf};

/// One immutable core read. The session owns the pinned generation database.
pub struct CoreReadSession {
    storage: Store,
    pointer: CorePublicationPointerV1,
    generation_path: PathBuf,
}

impl CoreReadSession {
    /// Lock-then-pin: read the publication pointer, then open that exact
    /// immutable generation. Live WAL content fails closed.
    pub fn pin(storage_path: &Path) -> Result<Self, StorageError> {
        let layout = CorePublicationLayout::from_storage_path(storage_path)?;
        let pointer = layout.read_pointer()?.ok_or_else(|| {
            StorageError::Other(format!(
                "No core publication pointer at {}",
                layout.publication_path().display()
            ))
        })?;
        let generation_path = layout.resolve_generation_database(&pointer.active.generation_id)?;
        let storage = Store::open_immutable_generation(&generation_path)?;
        Ok(Self {
            storage,
            pointer,
            generation_path,
        })
    }

    pub fn identity(&self) -> &CoreGenerationIdentityV1 {
        &self.pointer.active
    }

    pub fn pointer(&self) -> &CorePublicationPointerV1 {
        &self.pointer
    }

    pub fn generation_path(&self) -> &Path {
        &self.generation_path
    }

    pub fn storage(&self) -> &Store {
        &self.storage
    }

    pub fn into_storage(self) -> Store {
        self.storage
    }
}

/// One recoverable core publication. Stages a complete generation, then swaps
/// the pointer. Failure leaves the previous pointer usable.
pub struct CorePublishTransaction {
    layout: CorePublicationLayout,
    staged_database: PathBuf,
}

impl CorePublishTransaction {
    pub fn begin_from_stage(
        storage_path: &Path,
        staged_database: PathBuf,
    ) -> Result<Self, StorageError> {
        Ok(Self {
            layout: CorePublicationLayout::from_storage_path(storage_path)?,
            staged_database,
        })
    }

    pub fn layout(&self) -> &CorePublicationLayout {
        &self.layout
    }

    pub fn staged_database(&self) -> &Path {
        &self.staged_database
    }

    pub fn generation_database_path(&self, generation_id: &str) -> Result<PathBuf, StorageError> {
        self.layout.generation_database_path(generation_id)
    }

    /// Install the staged database as an immutable generation. Callers that
    /// already have that generation on disk must not call this.
    pub fn install_generation(&self, generation_id: &str) -> Result<PathBuf, StorageError> {
        self.layout
            .install_staging_generation(&self.staged_database, generation_id)?;
        self.layout.generation_database_path(generation_id)
    }

    /// First publication for an empty cache (managed non-CoW 0.17 rehydrate).
    pub fn commit_rehydrate(
        self,
        target_storage_path: &Path,
    ) -> Result<CorePublicationCommitV1, StorageError> {
        publish_rehydrated_generation(&self.staged_database, target_storage_path)
    }

    /// Install the staged generation if it is still present, then swap the
    /// publication pointer. The staged path is consumed by install; a missing
    /// stage means the generation is already installed and only the pointer
    /// is replaced.
    pub fn commit_pointer(
        self,
        active: CoreGenerationIdentityV1,
        rollback: Option<CoreGenerationIdentityV1>,
    ) -> Result<CorePublicationCommitV1, StorageError> {
        if self.staged_database.is_file() {
            self.layout
                .install_staging_generation(&self.staged_database, &active.generation_id)?;
        }
        self.layout.publish_pointer(active, rollback)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core_generation::{CORE_DATABASE_FILE, CORE_STAGING_DIRECTORY};
    use crate::storage_impl::CURRENT_SCHEMA_VERSION;
    use rusqlite::Connection;
    use tempfile::tempdir;

    fn seed_sqlite(path: &Path) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("stage parent");
        }
        let connection = Connection::open(path).expect("open seed sqlite");
        connection
            .execute_batch(&format!(
                "PRAGMA journal_mode=DELETE; PRAGMA user_version={CURRENT_SCHEMA_VERSION}; CREATE TABLE t(x INTEGER); INSERT INTO t VALUES (1);"
            ))
            .expect("seed sqlite");
        drop(connection);
    }

    #[test]
    fn pin_fails_closed_on_live_wal() {
        let dir = tempdir().expect("temp");
        let logical = dir.path().join(CORE_DATABASE_FILE);
        let layout = CorePublicationLayout::from_storage_path(&logical).expect("layout");
        let staging = layout.create_staging_database_path().expect("stage path");
        seed_sqlite(&staging);
        std::fs::write(
            staging.with_file_name(format!("{}-wal", CORE_DATABASE_FILE)),
            b"wal-bytes",
        )
        .expect("write wal");
        let error = match Store::open_immutable_generation(&staging) {
            Ok(_) => panic!("live WAL must fail closed"),
            Err(error) => error,
        };
        let message = error.to_string();
        assert!(
            message.contains("live WAL"),
            "immutable open must fail closed on WAL: {message}"
        );
        let _ = CORE_STAGING_DIRECTORY;
    }

    #[test]
    fn rehydrate_transaction_publishes_pointer_then_pin_reads_it() {
        let dir = tempdir().expect("temp");
        let logical = dir.path().join(CORE_DATABASE_FILE);
        let layout = CorePublicationLayout::from_storage_path(&logical).expect("layout");
        let staging = layout.create_staging_database_path().expect("stage path");
        seed_sqlite(&staging);
        let tx = CorePublishTransaction::begin_from_stage(&logical, staging).expect("tx");
        let pointer = tx.commit_rehydrate(&logical).expect("publish");
        let session = CoreReadSession::pin(&logical).expect("pin");
        assert_eq!(
            session.identity().generation_id,
            pointer.active.generation_id
        );
        assert_eq!(session.pointer().receipt_digest, pointer.receipt_digest);
    }

    #[test]
    fn commit_pointer_installs_the_staged_generation_then_swaps_the_pointer() {
        let dir = tempdir().expect("temp");
        let logical = dir.path().join(CORE_DATABASE_FILE);
        let layout = CorePublicationLayout::from_storage_path(&logical).expect("layout");
        let staging = layout.create_staging_database_path().expect("stage path");
        seed_sqlite(&staging);
        let identity = CoreGenerationIdentityV1 {
            generation_id: "gen-owned".to_string(),
            run_id: "run-owned".to_string(),
            logical_bytes: 1,
            published_at_epoch_ms: 1,
        };
        let tx = CorePublishTransaction::begin_from_stage(&logical, staging.clone()).expect("tx");
        let pointer = tx.commit_pointer(identity.clone(), None).expect("publish");
        assert_eq!(pointer.active.generation_id, identity.generation_id);
        assert!(!staging.is_file(), "stage must be consumed by install");
        let session = CoreReadSession::pin(&logical).expect("pin");
        assert_eq!(session.identity().generation_id, identity.generation_id);
    }

    #[test]
    fn post_replacement_directory_sync_failure_reports_committed_unconfirmed() {
        let dir = tempdir().expect("temp");
        let logical = dir.path().join(CORE_DATABASE_FILE);
        let layout = CorePublicationLayout::from_storage_path(&logical).expect("layout");
        let staging = layout.create_staging_database_path().expect("stage path");
        seed_sqlite(&staging);
        let identity = CoreGenerationIdentityV1 {
            generation_id: "gen-unconfirmed".to_string(),
            run_id: "run-unconfirmed".to_string(),
            logical_bytes: 1,
            published_at_epoch_ms: 1,
        };
        let tx = CorePublishTransaction::begin_from_stage(&logical, staging).expect("tx");
        let commit = crate::core_generation::with_core_pointer_sync_failure(
            layout.publication_path().as_path(),
            || tx.commit_pointer(identity.clone(), None),
        )
        .expect("replacement is a committed result");

        assert_eq!(commit.pointer.active, identity);
        assert_eq!(
            commit.durability,
            crate::CorePublicationDurabilityV1::Unconfirmed(
                crate::CorePublicationDurabilityReasonV1::PointerDirectorySyncFailed
            )
        );
        assert_eq!(
            layout.read_pointer().expect("read pointer").unwrap(),
            commit.pointer
        );
    }
}
