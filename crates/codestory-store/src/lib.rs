mod file_store;
mod graph_store;
mod projection_store;
mod search_doc_store;
mod snapshot_store;
mod storage_impl;
mod trail_store;

pub use file_store::FileStore;
pub use graph_store::GraphStore;
pub use projection_store::{ProjectionBatch, ProjectionStore};
pub use search_doc_store::SearchDocStore;
pub use snapshot_store::{
    SnapshotRefreshStats, SnapshotStore, StagedSnapshot, StagedSnapshotFinalizeStats,
};
pub use storage_impl::{
    CallerProjectionRemovalSummary, FileInfo, FileProjectionRemovalSummary, GroundingEdgeKindCount,
    GroundingFileSummary, GroundingNodeRecord, GroundingSnapshotMetadata, GroundingSnapshotState,
    LlmSymbolDoc, LlmSymbolDocReuseMetadata, LlmSymbolDocStats, ProjectionFlushBreakdown,
    SearchSymbolProjection, Storage as Store, StorageError, StorageOpenMode, StorageStats,
    SymbolSummaryRecord,
};
pub use trail_store::TrailStore;

impl Store {
    pub fn files(&self) -> FileStore<'_> {
        FileStore::new(self)
    }

    pub fn graph(&self) -> GraphStore<'_> {
        GraphStore::new(self)
    }

    pub fn projections(&mut self) -> ProjectionStore<'_> {
        ProjectionStore::new(self)
    }

    pub fn snapshots(&self) -> SnapshotStore<'_> {
        SnapshotStore::new(self)
    }

    pub fn trails(&self) -> TrailStore<'_> {
        TrailStore::new(self)
    }

    pub fn search_docs(&mut self) -> SearchDocStore<'_> {
        SearchDocStore::new(self)
    }
}
