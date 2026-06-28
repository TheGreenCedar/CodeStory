use crate::{GroundingSnapshotMetadata, StorageError, Store};
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// Timings for rebuilding both grounding snapshot layers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapshotRefreshStats {
    pub summary_snapshot_ms: u32,
    pub detail_snapshot_ms: u32,
}

/// Timings for making a staged snapshot safe to publish.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StagedSnapshotFinalizeStats {
    pub deferred_indexes_ms: u32,
    pub summary_snapshot_ms: u32,
}

/// Derived grounding snapshot facade.
///
/// Summary and detail snapshots are cached read models over the graph tables.
/// Projection writes must invalidate them; callers publish or query them only
/// after the relevant ready-state check succeeds.
pub struct SnapshotStore<'a> {
    storage: &'a Store,
}

/// Temporary build database that can be finalized before replacing a live
/// snapshot path.
pub struct StagedSnapshot {
    path: PathBuf,
    store: Store,
}

impl<'a> SnapshotStore<'a> {
    pub(crate) fn new(storage: &'a Store) -> Self {
        Self { storage }
    }

    /// Build a unique SQLite path beside the intended live database.
    pub fn staged_path(live_path: &Path) -> PathBuf {
        let parent = live_path.parent().unwrap_or_else(|| Path::new("."));
        let stem = live_path
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("codestory");
        let extension = live_path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("sqlite");
        let unique = unique_staged_suffix();
        parent.join(format!("{stem}.staged.{unique}.{extension}"))
    }

    /// Open a fresh staged database in build mode.
    pub fn open_staged(live_path: &Path) -> Result<StagedSnapshot, StorageError> {
        StagedSnapshot::open(live_path)
    }

    /// Remove a staged database and SQLite sidecars.
    pub fn discard_staged(staged_path: &Path) -> Result<(), StorageError> {
        Store::discard_staged_snapshot(staged_path)
    }

    /// Atomically promote a finalized staged database to the live path.
    pub fn promote_staged(staged_path: &Path, live_path: &Path) -> Result<(), StorageError> {
        Store::promote_staged_snapshot(staged_path, live_path)
    }

    /// Return current derived snapshot metadata, if any has been created.
    pub fn get_metadata(&self) -> Result<Option<GroundingSnapshotMetadata>, StorageError> {
        self.storage.get_grounding_snapshot_metadata()
    }

    /// Create secondary indexes that are deferred during build-mode writes.
    pub fn create_deferred_indexes(&self) -> Result<(), StorageError> {
        self.storage.create_deferred_secondary_indexes()
    }

    /// Create deferred indexes and refresh the summary snapshot for publish.
    pub fn finalize_staged(&self) -> Result<StagedSnapshotFinalizeStats, StorageError> {
        let deferred_started = Instant::now();
        self.create_deferred_indexes()?;
        let deferred_indexes_ms = clamp_u128_to_u32(deferred_started.elapsed().as_millis());

        let summary_started = Instant::now();
        self.refresh_summary()?;
        let summary_snapshot_ms = clamp_u128_to_u32(summary_started.elapsed().as_millis());

        Ok(StagedSnapshotFinalizeStats {
            deferred_indexes_ms,
            summary_snapshot_ms,
        })
    }

    /// Return whether the summary snapshot is ready for reads.
    pub fn has_ready_summary(&self) -> Result<bool, StorageError> {
        self.storage.has_ready_grounding_summary_snapshots()
    }

    /// Return whether the detail snapshot is ready for reads.
    pub fn has_ready_detail(&self) -> Result<bool, StorageError> {
        self.storage.has_ready_grounding_detail_snapshots()
    }

    /// Refresh the repository summary snapshot from current graph tables.
    pub fn refresh_summary(&self) -> Result<(), StorageError> {
        self.storage.refresh_grounding_summary_snapshots()
    }

    /// Hydrate the detail snapshot from current graph tables.
    pub fn refresh_detail(&self) -> Result<(), StorageError> {
        self.storage.hydrate_grounding_detail_snapshots()
    }

    /// Refresh both summary and detail snapshots.
    pub fn refresh_all(&self) -> Result<(), StorageError> {
        self.storage.refresh_grounding_snapshots()
    }

    /// Refresh both snapshot layers and return timing stats.
    pub fn refresh_all_with_stats(&self) -> Result<SnapshotRefreshStats, StorageError> {
        let summary_started = Instant::now();
        self.refresh_summary()?;
        let summary_snapshot_ms = clamp_u128_to_u32(summary_started.elapsed().as_millis());

        let detail_started = Instant::now();
        self.refresh_detail()?;
        let detail_snapshot_ms = clamp_u128_to_u32(detail_started.elapsed().as_millis());

        Ok(SnapshotRefreshStats {
            summary_snapshot_ms,
            detail_snapshot_ms,
        })
    }

    /// Mark derived snapshots dirty after projection changes.
    pub fn invalidate_derived(&self) -> Result<(), StorageError> {
        self.storage.invalidate_grounding_snapshots()
    }
}

impl StagedSnapshot {
    fn open(live_path: &Path) -> Result<Self, StorageError> {
        let path = SnapshotStore::staged_path(live_path);
        let store = Store::open_build(&path)?;
        Ok(Self { path, store })
    }

    /// Return the staged database path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Borrow the mutable staged store for build writes.
    pub fn store_mut(&mut self) -> &mut Store {
        &mut self.store
    }

    /// Access snapshot operations for the staged store.
    pub fn snapshots(&self) -> SnapshotStore<'_> {
        self.store.snapshots()
    }

    /// Close and remove the staged database.
    pub fn discard(self) -> Result<(), StorageError> {
        let path = self.path;
        drop(self.store);
        SnapshotStore::discard_staged(&path)
    }

    /// Close and promote the staged database to `live_path`.
    pub fn publish(self, live_path: &Path) -> Result<(), StorageError> {
        let path = self.path;
        drop(self.store);
        SnapshotStore::promote_staged(&path, live_path)
    }
}

fn clamp_u128_to_u32(value: u128) -> u32 {
    value.min(u32::MAX as u128) as u32
}

fn unique_staged_suffix() -> String {
    let epoch_ns = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    format!("{}-{}", std::process::id(), epoch_ns)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::GroundingSnapshotState;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn fresh_temp_root(label: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "codestory-store-{label}-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("create temp root");
        root
    }

    #[test]
    fn snapshot_store_refresh_and_invalidate_cycle_is_visible_through_metadata() {
        let store = Store::new_in_memory().expect("in-memory store");

        assert!(
            store
                .snapshots()
                .get_metadata()
                .expect("read initial metadata")
                .is_none(),
            "new stores should not materialize snapshot metadata before refresh"
        );

        store
            .snapshots()
            .refresh_all_with_stats()
            .expect("refresh all snapshots");
        let refreshed = store
            .snapshots()
            .get_metadata()
            .expect("read refreshed metadata")
            .expect("metadata row");
        assert_eq!(refreshed.summary_state, GroundingSnapshotState::Ready);
        assert_eq!(refreshed.detail_state, GroundingSnapshotState::Ready);

        store
            .snapshots()
            .invalidate_derived()
            .expect("invalidate snapshots");
        let invalidated = store
            .snapshots()
            .get_metadata()
            .expect("read invalidated metadata")
            .expect("metadata row");
        assert_eq!(invalidated.summary_state, GroundingSnapshotState::Dirty);
        assert_eq!(invalidated.detail_state, GroundingSnapshotState::Dirty);
    }

    #[test]
    fn snapshot_store_can_prepare_and_promote_staged_publish() {
        let temp = fresh_temp_root("promote");
        let live_path = temp.join("live.sqlite");
        let staged = SnapshotStore::open_staged(&live_path).expect("open staged");

        staged
            .snapshots()
            .finalize_staged()
            .expect("prepare staged publish");
        staged.publish(&live_path).expect("promote staged snapshot");

        let live = Store::open(&live_path).expect("open live store");
        let metadata = live
            .snapshots()
            .get_metadata()
            .expect("read metadata")
            .expect("metadata row");
        assert_eq!(metadata.summary_state, GroundingSnapshotState::Ready);
        assert_eq!(metadata.detail_state, GroundingSnapshotState::Dirty);

        let _ = fs::remove_dir_all(&temp);
    }

    #[test]
    fn snapshot_store_can_discard_staged_files() {
        let temp = fresh_temp_root("discard");
        let live_path = temp.join("discard.sqlite");
        let staged = SnapshotStore::open_staged(&live_path).expect("open staged");
        let staged_path = staged.path().to_path_buf();
        assert!(
            staged_path.exists(),
            "staged database should exist before discard"
        );

        staged.discard().expect("discard staged snapshot");
        assert!(!staged_path.exists(), "staged database should be removed");

        let _ = fs::remove_dir_all(&temp);
    }

    #[test]
    fn staged_paths_are_unique_per_open() {
        let temp = fresh_temp_root("unique");
        let live_path = temp.join("live.sqlite");

        let staged_a = SnapshotStore::open_staged(&live_path).expect("open staged a");
        let staged_b = SnapshotStore::open_staged(&live_path).expect("open staged b");

        assert_ne!(staged_a.path(), staged_b.path());

        staged_a.discard().expect("discard staged a");
        staged_b.discard().expect("discard staged b");

        let _ = fs::remove_dir_all(&temp);
    }
}
