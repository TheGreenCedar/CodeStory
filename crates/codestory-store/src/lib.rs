//! SQLite persistence facades for CodeStory graph, search, and snapshot state.
//!
//! `Store` owns the schema connection. The smaller facade types expose the
//! pipeline contracts most callers need: file inventory for refresh planning,
//! projection flushing for indexer output, and derived snapshot lifecycle for
//! read-heavy grounding views. The store layer persists evidence; it does not
//! upgrade structural source proof into parser-backed graph evidence.

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
    CURRENT_SCHEMA_VERSION, CallerProjectionRemovalSummary, DENSE_ANCHOR_MIGRATION_STATE_NATIVE,
    DENSE_ANCHOR_PUBLICATION_SCHEMA_VERSION, DenseAnchorInput, DenseAnchorInputReuseMetadata,
    DenseAnchorPublicationManifest, DenseReasonCounts, FileContentHash, FileInfo,
    FileProjectionRemovalSummary, FileRole, GroundingEdgeKindCount, GroundingFileSummary,
    GroundingNodeRecord, GroundingSnapshotMetadata, GroundingSnapshotState, IndexPublicationMode,
    IndexPublicationRecord, LlmSymbolDoc, LlmSymbolDocReuseMetadata, LlmSymbolDocStats,
    ProjectionFlushBreakdown, RetrievalIndexManifest, RetrievalIndexRollbackRecord,
    SearchSymbolProjection, SearchSymbolProjectionDetail, Storage as Store, StorageError,
    StorageOpenMode, StorageStats, SymbolSearchDoc, SymbolSummaryRecord,
};

impl Store {
    /// Access stored file inventory used by workspace refresh planning.
    pub fn files(&self) -> FileStore<'_> {
        FileStore::new(self)
    }

    /// Access graph/search projection writes for indexer output.
    pub fn projections(&mut self) -> ProjectionStore<'_> {
        ProjectionStore::new(self)
    }

    /// Access derived grounding snapshot lifecycle operations.
    pub fn snapshots(&self) -> SnapshotStore<'_> {
        SnapshotStore::new(self)
    }
}
