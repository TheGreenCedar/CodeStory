//! SQLite persistence facades for CodeStory graph, search, and snapshot state.
//!
//! `Store` owns the schema connection. The smaller facade types expose the
//! pipeline contracts most callers need: file inventory for refresh planning,
//! projection flushing for indexer output, and derived snapshot lifecycle for
//! read-heavy grounding views. The store layer persists evidence; it does not
//! upgrade structural source proof into parser-backed graph evidence.

mod annotations;
mod core_generation;
mod file_store;
mod projection_store;
mod snapshot_store;
mod sqlite_path;
mod storage_impl;

pub use annotations::{
    ANNOTATION_SCHEMA_VERSION, AnchorDiscrimination, AnnotationBookmark, AnnotationCategory,
    AnnotationError, AnnotationExport, AnnotationExportBookmark, AnnotationExportCategory,
    AnnotationResolution, AnnotationStore, BookmarkAnchorEvidence, BookmarkAnchorInput,
    CoreAnchorCandidate, CoreAnchorIndex, LegacyAnnotationSnapshot, LegacyBookmarkRow,
    NativeRootBinding, OrphanReason, ResolutionStatus, anchor_evidence, legacy_bookmark_uuid,
    resolve_bookmark,
};
pub use core_generation::{
    CORE_DATABASE_FILE, CORE_DIRECTORY, CORE_GENERATIONS_DIRECTORY, CORE_PUBLICATION_FILE,
    CORE_STAGING_DIRECTORY, CorePublicationLayout, core_database_exists,
    resolve_core_database_path, resolve_core_generation_database_path,
};
pub use file_store::FileStore;
pub use projection_store::{ProjectionBatch, ProjectionStore};
pub use snapshot_store::{
    SnapshotRefreshStats, SnapshotStore, StagedSnapshot, StagedSnapshotFinalizeStats,
    StagedSnapshotPublishStats,
};
pub use storage_impl::{
    BUILD_EDGE_SEED_BATCH_SIZE, BatchProjectionRemovalSummary, BoundRetrievalIndexManifest,
    BoundedRawCallEdges, BuildNodeLookup, CURRENT_SCHEMA_VERSION, CallerProjectionRemovalSummary,
    CorePromotionStats, DENSE_ANCHOR_MIGRATION_STATE_NATIVE,
    DENSE_ANCHOR_PUBLICATION_SCHEMA_VERSION, DatabaseSnapshotCopyStats, DenseAnchorContentIdentity,
    DenseAnchorInput, DenseAnchorInputReuseMetadata, DenseAnchorInputStats,
    DenseAnchorPublicationManifest, DenseAnchorPublicationValidation, DenseReasonCounts,
    ExactCallEdgeProjection, FileContentHash, FileInfo, FileProjectionRemovalSummary, FileRole,
    GroundingCallDegree, GroundingEdgeKindCount, GroundingFileSummary, GroundingNodeRecord,
    GroundingSnapshotMetadata, GroundingSnapshotState, IndexArtifactCacheEntry,
    IndexArtifactCacheReader, IndexArtifactCacheWrite, IndexPublicationMode,
    IndexPublicationRecord, LlmSymbolDoc, LlmSymbolDocReuseMetadata, LlmSymbolDocStats,
    ProjectionFlushBreakdown, ProjectionPersistenceFamilyStats, ProjectionPersistenceStats,
    PromotedValidation, ProofResolutionPublication, RehydratedCacheRebaseStats,
    RetrievalCoreGenerationBinding, RetrievalIndexManifest, RetrievalIndexRollbackRecord,
    SOURCE_POLICY_EXCLUSION_PUBLICATION_SCHEMA_VERSION, STRUCTURAL_TEXT_UNIT_DESCRIPTOR_VERSION,
    STRUCTURAL_TEXT_UNIT_MIGRATION_STATE_NATIVE, STRUCTURAL_TEXT_UNIT_PUBLICATION_SCHEMA_VERSION,
    SearchSymbolProjection, SearchSymbolProjectionDetail, SourcePolicyExclusionManifest,
    SourcePolicyExclusionPolicyIdentity, SourcePolicyExclusionRecord, Storage as Store,
    StorageError, StorageOpenMode, StorageStats, StoredVectorEncoding,
    StructuralTextArtifactCacheWrite, StructuralTextProjection,
    StructuralTextPublicationCompatibility, StructuralTextUnit,
    StructuralTextUnitPublicationManifest, SymbolSearchDoc, SymbolSummaryRecord,
    UnownedProjectionRemovalSummary, seal_call_resolution_fact, stored_vector_encoding,
    structural_text_unit_digest,
};
#[cfg(debug_assertions)]
pub use storage_impl::{
    BashStoreResolutionWork, bash_store_resolution_work, reset_bash_store_resolution_work,
    reset_store_replay_work, store_replay_work,
};
pub(crate) use storage_impl::{
    ProofResolutionPublicationValidation, StructuralTextPublicationValidation,
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
