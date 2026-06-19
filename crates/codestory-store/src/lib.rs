mod file_store;
mod projection_store;
mod snapshot_store;
mod storage_impl;

pub use file_store::FileStore;
pub use projection_store::{ProjectionBatch, ProjectionStore};
pub use snapshot_store::{
    SnapshotRefreshStats, SnapshotStore, StagedSnapshot, StagedSnapshotFinalizeStats,
};
pub use storage_impl::{
    CURRENT_SCHEMA_VERSION, CallerProjectionRemovalSummary, DenseReasonCounts, FileInfo,
    FileProjectionRemovalSummary, FileRole, GroundingEdgeKindCount, GroundingFileSummary,
    GroundingNodeRecord, GroundingSnapshotMetadata, GroundingSnapshotState, LlmSymbolDoc,
    LlmSymbolDocReuseMetadata, LlmSymbolDocStats, ProjectionFlushBreakdown, RetrievalIndexManifest,
    SearchSymbolProjection, SearchSymbolProjectionDetail, Storage as Store, StorageError,
    StorageOpenMode, StorageStats, SymbolSearchDoc, SymbolSummaryRecord,
};

impl Store {
    pub fn files(&self) -> FileStore<'_> {
        FileStore::new(self)
    }

    pub fn projections(&mut self) -> ProjectionStore<'_> {
        ProjectionStore::new(self)
    }

    pub fn snapshots(&self) -> SnapshotStore<'_> {
        SnapshotStore::new(self)
    }
}
