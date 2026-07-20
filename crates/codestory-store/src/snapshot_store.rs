use crate::{
    CorePromotionStats, DatabaseSnapshotCopyStats, GroundingSnapshotMetadata, StorageError,
    StorageOpenMode, Store,
};
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

/// SQLite fence timings for a completed staged publication.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StagedSnapshotPublishStats {
    pub sqlite_wal_autocheckpoint_bytes: Option<u64>,
    pub sqlite_checkpoint_ms: Option<u32>,
    pub sqlite_sync_ms: Option<u32>,
    pub snapshot_copy: Option<DatabaseSnapshotCopyStats>,
    pub core_promotion: CorePromotionStats,
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
    snapshot_copy: Option<DatabaseSnapshotCopyStats>,
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

    /// Open a fresh, repeatable full-refresh stage with relaxed build writes.
    ///
    /// The consuming `publish` call installs the durable WAL checkpoint and
    /// filesystem fence before promotion can inspect or mutate live state.
    pub fn open_disposable_full_refresh(live_path: &Path) -> Result<StagedSnapshot, StorageError> {
        StagedSnapshot::open_disposable_full_refresh(live_path)
    }

    /// Clone the live database into a unique staged database in build mode.
    ///
    /// The SQLite backup primitive captures a coherent source snapshot while
    /// allowing existing live readers to remain open. Incremental writers can
    /// then mutate and finalize the clone without exposing partial graph or
    /// grounding-snapshot generations at the live path.
    pub fn clone_live_to_staged(live_path: &Path) -> Result<StagedSnapshot, StorageError> {
        StagedSnapshot::clone_live(live_path)
    }

    /// Remove a staged database and SQLite sidecars.
    pub fn discard_staged(staged_path: &Path) -> Result<(), StorageError> {
        Store::discard_staged_snapshot(staged_path)
    }

    /// Atomically promote a finalized staged database to the live path.
    pub fn promote_staged(
        staged_path: &Path,
        live_path: &Path,
    ) -> Result<CorePromotionStats, StorageError> {
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

    /// Create deferred indexes around the summary snapshot build for publish.
    pub fn finalize_staged(&self) -> Result<StagedSnapshotFinalizeStats, StorageError> {
        let pre_summary_indexes_started = Instant::now();
        self.storage
            .prepare_deferred_secondary_indexes_for_summary()?;
        let pre_summary_indexes_duration = pre_summary_indexes_started.elapsed();

        let summary_started = Instant::now();
        let node_file_rank_index_duration = self
            .storage
            .refresh_grounding_summary_snapshots_for_staged_finalize()?;
        let summary_with_mid_index_duration = summary_started.elapsed();

        let post_summary_indexes_started = Instant::now();
        self.storage
            .complete_deferred_secondary_indexes_after_summary()?;
        let post_summary_indexes_duration = post_summary_indexes_started.elapsed();

        Ok(staged_snapshot_finalize_stats(
            pre_summary_indexes_duration,
            summary_with_mid_index_duration,
            node_file_rank_index_duration,
            post_summary_indexes_duration,
        ))
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
        Ok(Self {
            path,
            store,
            snapshot_copy: None,
        })
    }

    fn open_disposable_full_refresh(live_path: &Path) -> Result<Self, StorageError> {
        let path = SnapshotStore::staged_path(live_path);
        let store = Store::open_disposable_full_build(&path)?;
        Ok(Self {
            path,
            store,
            snapshot_copy: None,
        })
    }

    fn clone_live(live_path: &Path) -> Result<Self, StorageError> {
        let path = SnapshotStore::staged_path(live_path);
        Store::discard_staged_snapshot(&path)?;
        let snapshot_copy = match Store::copy_database_snapshot(live_path, &path) {
            Ok(stats) => stats,
            Err(error) => {
                let _ = Store::discard_staged_snapshot(&path);
                return Err(error);
            }
        };
        match Store::open_with_mode(&path, StorageOpenMode::Build) {
            Ok(store) => Ok(Self {
                path,
                store,
                snapshot_copy: Some(snapshot_copy),
            }),
            Err(error) => {
                let _ = Store::discard_staged_snapshot(&path);
                Err(error)
            }
        }
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

    /// Seal, close, and promote the staged database to `live_path`.
    pub fn publish(self, live_path: &Path) -> Result<(), StorageError> {
        self.publish_with_stats(live_path).map(|_| ())
    }

    /// Seal, close, and promote while returning full-refresh SQLite timings.
    pub fn publish_with_stats(
        self,
        live_path: &Path,
    ) -> Result<StagedSnapshotPublishStats, StorageError> {
        let seal_stats = self.store.seal_disposable_full_build()?;
        let path = self.path;
        let snapshot_copy = self.snapshot_copy;
        drop(self.store);
        let core_promotion = SnapshotStore::promote_staged(&path, live_path)?;
        Ok(StagedSnapshotPublishStats {
            sqlite_wal_autocheckpoint_bytes: seal_stats
                .as_ref()
                .map(|stats| stats.wal_autocheckpoint_bytes),
            sqlite_checkpoint_ms: seal_stats.as_ref().map(|stats| stats.checkpoint_ms),
            sqlite_sync_ms: seal_stats.as_ref().map(|stats| stats.sync_ms),
            snapshot_copy,
            core_promotion,
        })
    }
}

fn clamp_u128_to_u32(value: u128) -> u32 {
    value.min(u32::MAX as u128) as u32
}

fn staged_snapshot_finalize_stats(
    pre_summary_indexes_duration: std::time::Duration,
    summary_with_mid_index_duration: std::time::Duration,
    node_file_rank_index_duration: std::time::Duration,
    post_summary_indexes_duration: std::time::Duration,
) -> StagedSnapshotFinalizeStats {
    let deferred_indexes_duration = pre_summary_indexes_duration
        .saturating_add(node_file_rank_index_duration)
        .saturating_add(post_summary_indexes_duration);
    let summary_snapshot_duration =
        summary_with_mid_index_duration.saturating_sub(node_file_rank_index_duration);
    StagedSnapshotFinalizeStats {
        deferred_indexes_ms: clamp_u128_to_u32(deferred_indexes_duration.as_millis()),
        summary_snapshot_ms: clamp_u128_to_u32(summary_snapshot_duration.as_millis()),
    }
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
    use std::sync::mpsc;
    use std::thread;
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

    fn publish_empty_source_policy(store: &mut Store, publication: &crate::IndexPublicationRecord) {
        store
            .publish_structural_text_unit_generation(publication)
            .expect("publish empty structural text unit identity");
        store
            .publish_source_policy_exclusion_generation(
                publication,
                "test-project",
                "test-workspace",
                codestory_contracts::workspace::OVERSIZED_SOURCE_POLICY_VERSION,
                codestory_contracts::workspace::DEFAULT_SOURCE_FILE_BYTE_CAP,
                &[],
            )
            .expect("publish empty source policy identity");
    }

    fn named_promotion_ms(stats: &CorePromotionStats) -> u32 {
        stats
            .lock_recovery_ms
            .saturating_add(stats.candidate_validation_ms)
            .saturating_add(stats.previous_validation_ms)
            .saturating_add(stats.rollback_backup_copy_ms.unwrap_or_default())
            .saturating_add(stats.backup_validation_ms.unwrap_or_default())
            .saturating_add(stats.prepared_journal_write_ms)
            .saturating_add(stats.prepared_journal_file_sync_ms)
            .saturating_add(stats.prepared_journal_directory_sync_ms)
            .saturating_add(stats.staged_to_live_restore_ms)
            .saturating_add(stats.promoted_validation_ms)
            .saturating_add(stats.committed_journal_ms)
            .saturating_add(stats.cleanup_ms)
    }

    fn assert_promotion_reconciles(stats: &CorePromotionStats) {
        assert_eq!(
            named_promotion_ms(stats).saturating_add(stats.unattributed_ms),
            stats.total_ms
        );
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
        let mut staged = SnapshotStore::open_staged(&live_path).expect("open staged");

        staged
            .snapshots()
            .finalize_staged()
            .expect("prepare staged publish");
        let publication = crate::IndexPublicationRecord {
            generation: 1,
            generation_id: "prepared-generation".to_string(),
            run_id: "prepared-run".to_string(),
            mode: crate::IndexPublicationMode::Full,
            published_at_epoch_ms: 1,
        };
        staged
            .store_mut()
            .put_index_publication(&publication)
            .expect("identify staged publication");
        publish_empty_source_policy(staged.store_mut(), &publication);
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
    fn full_replacement_reports_backup_and_restore_without_incremental_clone() {
        let temp = fresh_temp_root("full-replacement-telemetry");
        let live_path = temp.join("live.sqlite");
        {
            let mut live = Store::open(&live_path).expect("open previous live store");
            live.insert_files_batch(&[crate::FileInfo {
                id: 1,
                path: PathBuf::from("old.rs"),
                language: "rust".to_string(),
                modification_time: 1,
                indexed: true,
                complete: true,
                line_count: 1,
                file_role: crate::FileRole::Source,
            }])
            .expect("seed previous live file");
            let publication = crate::IndexPublicationRecord {
                generation: 1,
                generation_id: "old-generation".to_string(),
                run_id: "old-run".to_string(),
                mode: crate::IndexPublicationMode::Full,
                published_at_epoch_ms: 1,
            };
            live.put_index_publication(&publication)
                .expect("identify previous live publication");
            publish_empty_source_policy(&mut live, &publication);
        }

        let mut staged = SnapshotStore::open_staged(&live_path).expect("open replacement stage");
        staged
            .store_mut()
            .insert_files_batch(&[crate::FileInfo {
                id: 2,
                path: PathBuf::from("new.rs"),
                language: "rust".to_string(),
                modification_time: 2,
                indexed: true,
                complete: true,
                line_count: 2,
                file_role: crate::FileRole::Source,
            }])
            .expect("seed replacement file");
        let publication = crate::IndexPublicationRecord {
            generation: 2,
            generation_id: "new-generation".to_string(),
            run_id: "new-run".to_string(),
            mode: crate::IndexPublicationMode::Full,
            published_at_epoch_ms: 2,
        };
        staged
            .store_mut()
            .put_index_publication(&publication)
            .expect("identify replacement publication");
        publish_empty_source_policy(staged.store_mut(), &publication);

        let publish_stats = staged
            .publish_with_stats(&live_path)
            .expect("publish full replacement");
        assert!(publish_stats.snapshot_copy.is_none());
        assert!(
            publish_stats
                .core_promotion
                .rollback_backup_copy_ms
                .is_some()
        );
        assert!(publish_stats.core_promotion.backup_validation_ms.is_some());
        assert_eq!(
            publish_stats.core_promotion.previous_live_bytes,
            publish_stats.core_promotion.rollback_backup_bytes
        );
        assert!(
            publish_stats.core_promotion.previous_live_bytes.is_some(),
            "full replacement must report the previous live image"
        );
        assert_promotion_reconciles(&publish_stats.core_promotion);
        assert_eq!(
            publish_stats.core_promotion.candidate_bytes,
            fs::metadata(&live_path)
                .expect("read full replacement database size")
                .len()
        );

        let _ = fs::remove_dir_all(&temp);
    }

    #[test]
    fn staged_finalize_builds_summary_destination_indexes_in_required_order() {
        const DESTINATION_INDEXES: &[&str] = &[
            "idx_grounding_file_snapshot_path",
            "idx_grounding_file_snapshot_rank",
            "idx_grounding_node_snapshot_file_rank",
            "idx_grounding_node_snapshot_root_rank",
        ];

        let temp = fresh_temp_root("summary-index-order");
        let live_path = temp.join("live.sqlite");
        let mut staged = SnapshotStore::open_staged(&live_path).expect("open staged");
        staged
            .store_mut()
            .insert_files_batch(&[crate::FileInfo {
                id: 1,
                path: PathBuf::from("ordered.rs"),
                language: "rust".to_string(),
                modification_time: 1,
                indexed: true,
                complete: true,
                line_count: 1,
                file_role: crate::FileRole::Source,
            }])
            .expect("seed staged file");
        staged
            .store_mut()
            .get_connection()
            .execute_batch(
                "CREATE TRIGGER assert_summary_index_order
                 BEFORE INSERT ON grounding_file_snapshot
                 BEGIN
                   SELECT CASE WHEN NOT EXISTS (
                     SELECT 1 FROM sqlite_master
                     WHERE type = 'index'
                       AND name = 'idx_grounding_node_snapshot_file_rank'
                   ) THEN RAISE(ABORT, 'node file-rank index missing before file aggregation') END;
                   SELECT CASE WHEN EXISTS (
                     SELECT 1 FROM sqlite_master
                     WHERE type = 'index'
                       AND name IN (
                         'idx_grounding_file_snapshot_path',
                         'idx_grounding_file_snapshot_rank',
                         'idx_grounding_node_snapshot_root_rank'
                       )
                   ) THEN RAISE(ABORT, 'post-summary index built before file aggregation') END;
                 END;",
            )
            .expect("install summary ordering assertion");

        staged
            .snapshots()
            .finalize_staged()
            .expect("finalize staged summary");

        for index_name in DESTINATION_INDEXES {
            let count: i64 = staged
                .store_mut()
                .get_connection()
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = ?1",
                    [index_name],
                    |row| row.get(0),
                )
                .expect("inspect destination index");
            assert_eq!(count, 1, "missing destination index {index_name}");
        }
        assert!(
            staged
                .snapshots()
                .has_ready_summary()
                .expect("summary readiness")
        );

        staged.discard().expect("discard staged database");
        let _ = fs::remove_dir_all(&temp);
    }

    #[test]
    fn staged_finalize_timing_excludes_mid_summary_index_build() {
        let stats = staged_snapshot_finalize_stats(
            std::time::Duration::from_millis(11),
            std::time::Duration::from_millis(42),
            std::time::Duration::from_millis(13),
            std::time::Duration::from_millis(17),
        );

        assert_eq!(stats.deferred_indexes_ms, 41);
        assert_eq!(stats.summary_snapshot_ms, 29);

        let sub_millisecond_segments = staged_snapshot_finalize_stats(
            std::time::Duration::from_micros(400),
            std::time::Duration::from_micros(900),
            std::time::Duration::from_micros(400),
            std::time::Duration::from_micros(400),
        );
        assert_eq!(sub_millisecond_segments.deferred_indexes_ms, 1);
        assert_eq!(sub_millisecond_segments.summary_snapshot_ms, 0);
    }

    #[test]
    fn disposable_full_refresh_seals_wal_before_promotion() {
        let temp = fresh_temp_root("disposable-promote");
        let live_path = temp.join("live.sqlite");
        let mut staged = SnapshotStore::open_disposable_full_refresh(&live_path)
            .expect("open disposable full-refresh stage");
        let staged_path = staged.path().to_path_buf();
        let synchronous: i64 = staged
            .store_mut()
            .get_connection()
            .query_row("PRAGMA synchronous", [], |row| row.get(0))
            .expect("read disposable synchronous profile");
        assert_eq!(synchronous, 0);

        staged
            .store_mut()
            .insert_files_batch(&[crate::FileInfo {
                id: 1,
                path: PathBuf::from("disposable.rs"),
                language: "rust".to_string(),
                modification_time: 1,
                indexed: true,
                complete: true,
                line_count: 1,
                file_role: crate::FileRole::Source,
            }])
            .expect("seed disposable stage");
        let publication = crate::IndexPublicationRecord {
            generation: 1,
            generation_id: "disposable-generation".to_string(),
            run_id: "disposable-run".to_string(),
            mode: crate::IndexPublicationMode::Full,
            published_at_epoch_ms: 1,
        };
        staged
            .store_mut()
            .put_index_publication(&publication)
            .expect("identify disposable publication");
        publish_empty_source_policy(staged.store_mut(), &publication);

        let publish_stats = staged
            .publish_with_stats(&live_path)
            .expect("seal and promote disposable stage");
        assert_eq!(
            publish_stats.sqlite_wal_autocheckpoint_bytes,
            Some(64 * 1024 * 1024)
        );
        assert!(publish_stats.sqlite_checkpoint_ms.is_some());
        assert!(publish_stats.sqlite_sync_ms.is_some());
        assert!(publish_stats.snapshot_copy.is_none());
        assert!(publish_stats.core_promotion.previous_live_bytes.is_none());
        assert!(
            publish_stats
                .core_promotion
                .rollback_backup_copy_ms
                .is_none()
        );
        assert!(publish_stats.core_promotion.backup_validation_ms.is_none());
        assert!(publish_stats.core_promotion.rollback_backup_bytes.is_none());
        assert_promotion_reconciles(&publish_stats.core_promotion);
        assert_eq!(
            publish_stats.core_promotion.candidate_bytes,
            fs::metadata(&live_path)
                .expect("read promoted database size")
                .len()
        );

        let live = Store::open(&live_path).expect("open promoted live store");
        let quick_check: String = live
            .get_connection()
            .query_row("PRAGMA quick_check", [], |row| row.get(0))
            .expect("validate promoted database");
        assert_eq!(quick_check, "ok");
        assert_eq!(
            live.get_complete_index_publication()
                .expect("read live publication"),
            Some(publication)
        );
        assert_eq!(
            live.get_files().expect("read promoted files")[0].path,
            PathBuf::from("disposable.rs")
        );
        assert!(!staged_path.exists());
        assert!(!PathBuf::from(format!("{}-wal", staged_path.display())).exists());
        assert!(!PathBuf::from(format!("{}-shm", staged_path.display())).exists());

        drop(live);
        let _ = fs::remove_dir_all(&temp);
    }

    #[test]
    fn disposable_full_refresh_busy_seal_never_starts_promotion() {
        let temp = fresh_temp_root("disposable-busy");
        let live_path = temp.join("live.sqlite");
        let mut staged = SnapshotStore::open_disposable_full_refresh(&live_path)
            .expect("open disposable full-refresh stage");
        let staged_path = staged.path().to_path_buf();
        let publication = crate::IndexPublicationRecord {
            generation: 1,
            generation_id: "busy-generation".to_string(),
            run_id: "busy-run".to_string(),
            mode: crate::IndexPublicationMode::Full,
            published_at_epoch_ms: 1,
        };
        staged
            .store_mut()
            .put_index_publication(&publication)
            .expect("identify busy candidate");
        publish_empty_source_policy(staged.store_mut(), &publication);

        let reader = rusqlite::Connection::open(&staged_path).expect("open staged reader");
        reader
            .execute_batch("BEGIN DEFERRED; SELECT COUNT(*) FROM file;")
            .expect("pin staged reader snapshot");
        staged
            .store_mut()
            .insert_files_batch(&[crate::FileInfo {
                id: 2,
                path: PathBuf::from("newer.rs"),
                language: "rust".to_string(),
                modification_time: 2,
                indexed: true,
                complete: true,
                line_count: 1,
                file_role: crate::FileRole::Source,
            }])
            .expect("write after pinned snapshot");

        let error = staged
            .publish(&live_path)
            .expect_err("busy final checkpoint must block promotion");
        let message = error.to_string().to_ascii_lowercase();
        assert!(
            message.contains("seal") || message.contains("locked") || message.contains("busy"),
            "unexpected seal error: {error}"
        );
        assert!(
            !live_path.exists(),
            "seal failure must not create live state"
        );
        assert!(staged_path.exists(), "failed stage must remain inspectable");
        assert!(!live_path.with_extension("sqlite.backup").exists());
        assert!(
            !PathBuf::from(format!("{}.promotion.prepared.json", live_path.display())).exists()
        );

        drop(reader);
        Store::discard_staged_snapshot(&staged_path).expect("discard failed stage");
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
    fn snapshot_store_can_clone_live_without_mutating_it() {
        let temp = fresh_temp_root("clone-live");
        let live_path = temp.join("live.sqlite");
        {
            let mut live = Store::open(&live_path).expect("open live");
            live.insert_files_batch(&[crate::FileInfo {
                id: 1,
                path: PathBuf::from("old.rs"),
                language: "rust".to_string(),
                modification_time: 1,
                indexed: true,
                complete: true,
                line_count: 1,
                file_role: crate::FileRole::Source,
            }])
            .expect("seed live file");
        }

        let mut staged =
            SnapshotStore::clone_live_to_staged(&live_path).expect("clone live to staged");
        assert_eq!(
            staged.store_mut().get_files().expect("read staged files")[0].path,
            PathBuf::from("old.rs")
        );
        staged
            .store_mut()
            .insert_files_batch(&[crate::FileInfo {
                id: 2,
                path: PathBuf::from("new.rs"),
                language: "rust".to_string(),
                modification_time: 2,
                indexed: true,
                complete: true,
                line_count: 1,
                file_role: crate::FileRole::Source,
            }])
            .expect("mutate staged clone");

        let live = Store::open(&live_path).expect("reopen live");
        let live_paths = live
            .get_files()
            .expect("read live files")
            .into_iter()
            .map(|file| file.path)
            .collect::<Vec<_>>();
        assert_eq!(live_paths, vec![PathBuf::from("old.rs")]);

        let staged_path = staged.path().to_path_buf();
        staged.discard().expect("discard staged clone");
        assert!(!staged_path.exists());
        assert!(!PathBuf::from(format!("{}-wal", staged_path.display())).exists());
        assert!(!PathBuf::from(format!("{}-shm", staged_path.display())).exists());

        let _ = fs::remove_dir_all(&temp);
    }

    #[test]
    fn snapshot_copy_reports_bounded_database_image_bytes() {
        const PAYLOAD_BYTES: usize = 2 * 1024 * 1024;

        let temp = fresh_temp_root("copy-telemetry");
        let source_path = temp.join("source.sqlite");
        let target_path = temp.join("target.sqlite");
        {
            let source = Store::open(&source_path).expect("open copy source");
            source
                .get_connection()
                .execute_batch(
                    "CREATE TABLE bounded_copy_payload (payload BLOB NOT NULL);
                     INSERT INTO bounded_copy_payload(payload) VALUES (zeroblob(2097152));",
                )
                .expect("seed bounded copy payload");
        }
        let source_bytes = fs::metadata(&source_path)
            .expect("read source database size")
            .len();
        assert!(source_bytes >= PAYLOAD_BYTES as u64);

        let stats =
            Store::copy_database_snapshot(&source_path, &target_path).expect("copy database");

        assert_eq!(stats.source_bytes, source_bytes);
        assert_eq!(
            stats.target_bytes,
            fs::metadata(&target_path)
                .expect("read target database size")
                .len()
        );
        assert_eq!(stats.source_bytes, stats.target_bytes);
        let copied = Store::open_read_only(&target_path).expect("open copied database");
        let payload_bytes: i64 = copied
            .get_connection()
            .query_row(
                "SELECT length(payload) FROM bounded_copy_payload",
                [],
                |row| row.get(0),
            )
            .expect("read copied payload");
        assert_eq!(payload_bytes, PAYLOAD_BYTES as i64);

        let _ = fs::remove_dir_all(&temp);
    }

    #[test]
    fn failed_live_clone_leaves_no_staged_artifacts() {
        let temp = fresh_temp_root("clone-failure-cleanup");
        let missing_live_path = temp.join("missing.sqlite");

        assert!(
            SnapshotStore::clone_live_to_staged(&missing_live_path).is_err(),
            "missing live database must fail cloning"
        );

        let remaining = fs::read_dir(&temp)
            .expect("list temp root")
            .collect::<Result<Vec<_>, _>>()
            .expect("read temp entries");
        assert!(
            remaining.is_empty(),
            "unexpected staged debris: {remaining:?}"
        );

        let _ = fs::remove_dir_all(&temp);
    }

    #[test]
    fn staged_clone_publish_is_old_or_new_for_concurrent_readers() {
        let temp = fresh_temp_root("clone-publish-readers");
        let live_path = temp.join("live.sqlite");
        {
            let mut live = Store::open(&live_path).expect("open live");
            live.insert_files_batch(&[
                crate::FileInfo {
                    id: 1,
                    path: PathBuf::from("old_a.rs"),
                    language: "rust".to_string(),
                    modification_time: 1,
                    indexed: true,
                    complete: true,
                    line_count: 1,
                    file_role: crate::FileRole::Source,
                },
                crate::FileInfo {
                    id: 2,
                    path: PathBuf::from("old_b.rs"),
                    language: "rust".to_string(),
                    modification_time: 1,
                    indexed: true,
                    complete: true,
                    line_count: 1,
                    file_role: crate::FileRole::Source,
                },
            ])
            .expect("seed old generation");
            let publication = crate::IndexPublicationRecord {
                generation: 1,
                generation_id: "old-generation".to_string(),
                run_id: "old-run".to_string(),
                mode: crate::IndexPublicationMode::Full,
                published_at_epoch_ms: 1,
            };
            live.put_index_publication(&publication)
                .expect("identify old generation");
            publish_empty_source_policy(&mut live, &publication);
            live.snapshots()
                .refresh_all()
                .expect("finalize old snapshots");
        }

        let old_reader = Store::open(&live_path).expect("hold old reader");
        old_reader
            .get_connection()
            .execute_batch("BEGIN;")
            .expect("begin held read transaction");
        assert_eq!(
            old_reader.get_files().expect("establish old read snapshot")[0].path,
            PathBuf::from("old_a.rs")
        );
        let mut staged =
            SnapshotStore::clone_live_to_staged(&live_path).expect("clone old generation");
        staged
            .store_mut()
            .delete_files_batch(&[1, 2])
            .expect("remove old file in staged clone");
        staged
            .store_mut()
            .insert_files_batch(&[
                crate::FileInfo {
                    id: 3,
                    path: PathBuf::from("new_a.rs"),
                    language: "rust".to_string(),
                    modification_time: 2,
                    indexed: true,
                    complete: true,
                    line_count: 2,
                    file_role: crate::FileRole::Source,
                },
                crate::FileInfo {
                    id: 4,
                    path: PathBuf::from("new_b.rs"),
                    language: "rust".to_string(),
                    modification_time: 2,
                    indexed: true,
                    complete: true,
                    line_count: 2,
                    file_role: crate::FileRole::Source,
                },
            ])
            .expect("seed new generation");
        staged
            .snapshots()
            .refresh_all()
            .expect("finalize new snapshots");
        let publication = crate::IndexPublicationRecord {
            generation: 2,
            generation_id: "new-generation".to_string(),
            run_id: "new-run".to_string(),
            mode: crate::IndexPublicationMode::Incremental,
            published_at_epoch_ms: 2,
        };
        staged
            .store_mut()
            .put_index_publication(&publication)
            .expect("identify new generation");
        publish_empty_source_policy(staged.store_mut(), &publication);
        let staged_path = staged.path().to_path_buf();
        let racing_live_path = live_path.clone();
        let (old_observed_tx, old_observed_rx) = mpsc::channel();
        let racing_reader = thread::spawn(move || {
            let old_generation = vec![PathBuf::from("old_a.rs"), PathBuf::from("old_b.rs")];
            let new_generation = vec![PathBuf::from("new_a.rs"), PathBuf::from("new_b.rs")];
            let mut announced_old = false;
            loop {
                let reader = Store::open(&racing_live_path).expect("open racing reader");
                reader
                    .get_connection()
                    .execute_batch("BEGIN;")
                    .expect("begin racing read transaction");
                let mut paths = reader
                    .get_files()
                    .expect("read racing generation")
                    .into_iter()
                    .map(|file| file.path)
                    .collect::<Vec<_>>();
                paths.sort();
                let snapshots_ready = reader
                    .snapshots()
                    .has_ready_summary()
                    .expect("racing summary readiness")
                    && reader
                        .snapshots()
                        .has_ready_detail()
                        .expect("racing detail readiness");
                let publication = reader
                    .get_index_publication()
                    .expect("racing publication read")
                    .expect("racing publication identity");
                reader
                    .get_connection()
                    .execute_batch("COMMIT;")
                    .expect("finish racing read transaction");
                assert!(
                    snapshots_ready,
                    "racing reader observed unready snapshots for {paths:?}"
                );
                if paths == old_generation {
                    assert_eq!(publication.generation, 1);
                    if !announced_old {
                        old_observed_tx.send(()).expect("announce old generation");
                        announced_old = true;
                    }
                } else if paths == new_generation {
                    assert_eq!(publication.generation, 2);
                    assert!(announced_old, "racing reader must span publication");
                    return;
                } else {
                    panic!("racing reader observed a mixed generation: {paths:?}");
                }
            }
        });
        old_observed_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("racing reader observed old generation");
        let publish_stats = staged
            .publish_with_stats(&live_path)
            .expect("publish new generation");
        racing_reader.join().expect("racing reader");
        let snapshot_copy = publish_stats
            .snapshot_copy
            .expect("incremental clone telemetry");
        assert_eq!(snapshot_copy.source_bytes, snapshot_copy.target_bytes);
        assert_eq!(
            publish_stats.core_promotion.previous_live_bytes,
            Some(snapshot_copy.source_bytes)
        );
        assert_eq!(
            publish_stats.core_promotion.rollback_backup_bytes,
            publish_stats.core_promotion.previous_live_bytes
        );
        assert!(
            publish_stats
                .core_promotion
                .rollback_backup_copy_ms
                .is_some()
        );
        assert!(publish_stats.core_promotion.backup_validation_ms.is_some());
        assert_promotion_reconciles(&publish_stats.core_promotion);
        assert_eq!(
            publish_stats.core_promotion.candidate_bytes,
            fs::metadata(&live_path)
                .expect("read incremental promoted database size")
                .len()
        );

        let old_paths = old_reader
            .get_files()
            .expect("read held old generation")
            .into_iter()
            .map(|file| file.path)
            .collect::<Vec<_>>();
        assert_eq!(
            old_paths,
            vec![PathBuf::from("old_a.rs"), PathBuf::from("old_b.rs")]
        );
        assert_eq!(
            old_reader
                .get_index_publication()
                .expect("held reader publication")
                .expect("held reader publication identity")
                .generation,
            1
        );

        let new_reader = Store::open(&live_path).expect("open fresh reader");
        let new_paths = new_reader
            .get_files()
            .expect("read fresh generation")
            .into_iter()
            .map(|file| file.path)
            .collect::<Vec<_>>();
        assert_eq!(
            new_paths,
            vec![PathBuf::from("new_a.rs"), PathBuf::from("new_b.rs")]
        );
        assert_eq!(
            new_reader
                .get_index_publication()
                .expect("new reader publication")
                .expect("new reader publication identity")
                .generation,
            2
        );
        assert!(
            new_reader
                .snapshots()
                .has_ready_summary()
                .expect("new summary readiness")
                && new_reader
                    .snapshots()
                    .has_ready_detail()
                    .expect("new detail readiness")
        );
        assert!(!staged_path.exists());
        assert!(!PathBuf::from(format!("{}-wal", staged_path.display())).exists());
        assert!(!PathBuf::from(format!("{}-shm", staged_path.display())).exists());

        old_reader
            .get_connection()
            .execute_batch("COMMIT;")
            .expect("finish held read transaction");
        drop(old_reader);
        drop(new_reader);
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
