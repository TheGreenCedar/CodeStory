//! API-facing DTOs and mirrored enum types.
//!
//! These exports are the boundary between the runtime and external callers such
//! as CLI JSON, UI bindings, and generated TypeScript types. The Rust names may
//! look close to the core graph model, but the serialized forms are product
//! contracts: preserve field names, serde aliases, defaults, and enum casing
//! unless every caller can migrate in lockstep.

mod dto;
mod errors;
mod events;
mod ids;
mod types;

pub use dto::{
    AffectedAnalysisBoundsDto, AffectedAnalysisCompletenessDto, AffectedAnalysisDto,
    AffectedAnalysisInput, AffectedAnalysisRequest, AffectedChangeKindDto, AffectedChangeRecordDto,
    AffectedFollowUpDto, AffectedFollowUpInvocationDto, AffectedInputClassificationDto,
    AffectedMatchedFileDto, AffectedRouteDto, AffectedSymbolDto, AffectedTestFileDto,
    AffectedUncoveredInputDto, AffectedUnmatchedPathDto, AgentAnswerDto, AgentAskRequest,
    AgentCitationDto, AgentCustomRetrievalConfigDto, AgentHybridWeightsDto, AgentPacketDto,
    AgentPacketRequestDto, AgentResponseBlockDto, AgentResponseModeDto, AgentResponseSectionDto,
    AgentRetrievalPolicyModeDto, AgentRetrievalPresetDto, AgentRetrievalProfileSelectionDto,
    AgentRetrievalStepDto, AgentRetrievalStepKindDto, AgentRetrievalStepStatusDto,
    AgentRetrievalSummaryFieldDto, AgentRetrievalTraceDto, BookmarkCategoryDto, BookmarkDto,
    CanonicalEdgeDto, CanonicalEdgeFamily, CanonicalLayoutDto, CanonicalMemberDto,
    CanonicalMemberVisibility, CanonicalNodeDto, CanonicalNodeStyle, CanonicalRouteKind,
    ClaimReadinessDto, CreateBookmarkCategoryRequest, CreateBookmarkRequest,
    EMBEDDING_VECTOR_PRODUCER_EVIDENCE_VERSION, EdgeOccurrencesRequest, EmbeddingEngineIdentityDto,
    EmbeddingExecutionEvidenceDto, EmbeddingModelIdentityDto, EmbeddingProducerIdentityDto,
    EmbeddingProfileContractDto, EmbeddingVectorEvidenceCompatibilityDto,
    EmbeddingVectorEvidenceMigrationDispositionDto, EmbeddingVectorProducerEvidenceDto,
    EmbeddingVectorPublicationIdentityDto, EmbeddingVectorSemanticsDto, FrameworkRouteCoverageDto,
    GraphArtifactDto, GraphEdgeDto, GraphNodeDto, GraphRequest, GraphResponse, GroundingBudgetDto,
    GroundingCoverageBucketDto, GroundingCoverageDto, GroundingFileDigestDto, GroundingSnapshotDto,
    GroundingSymbolDigestDto, IndexDryRunDto, IndexFreshnessChangeKindDto, IndexFreshnessDto,
    IndexFreshnessSampleDto, IndexFreshnessStatusDto, IndexPublicationDto, IndexPublicationModeDto,
    IndexedFileDto, IndexedFileIncompleteReasonCountDto, IndexedFileLanguageCountDto,
    IndexedFileRoleDto, IndexedFilesDto, IndexedFilesRequest, IndexedFilesSummaryDto,
    ListChildrenSymbolsRequest, ListRootSymbolsRequest, NodeDetailsDto, NodeDetailsRequest,
    NodeOccurrencesRequest, OpenContainingFolderRequest, OpenDefinitionRequest, OpenProjectRequest,
    PacketBudgetDto, PacketBudgetLimitsDto, PacketBudgetModeDto, PacketBudgetUsageDto,
    PacketClaimDto, PacketCoverageReportDto, PacketEvidenceResolutionDto, PacketEvidenceTierDto,
    PacketPlanDto, PacketPlanQueryDto, PacketProofStatusDto, PacketRetrievalTraceSummaryDto,
    PacketSidecarQueryDiagnosticDto, PacketSufficiencyDto, PacketSufficiencyStatusDto,
    PacketTaskClassDto, ProjectSummary, ReadFileTextRequest, ReadFileTextResponse,
    ReadinessGoalDto, ReadinessIndexSnapshotDto, ReadinessSetupSnapshotDto,
    ReadinessSidecarSnapshotDto, ReadinessStatusDto, ReadinessVerdictDto, RepoTextScanStatsDto,
    RetrievalCandidateResolutionCountDto, RetrievalCandidateSummaryDto, RetrievalFallbackReasonDto,
    RetrievalModeDto, RetrievalScoreBreakdownDto, RetrievalShadowDto, RetrievalStageTimingDto,
    RetrievalStateDto, RouteEndpointHandlerDto, RouteEndpointKindDto, RouteEndpointMetadataDto,
    SearchHit, SearchHitOrigin, SearchHybridLimitsDto, SearchMatchQualityDto,
    SearchPlanAnchorGroupDto, SearchPlanBridgeConfidenceDto, SearchPlanBridgeDto,
    SearchPlanBridgeEvidenceKindDto, SearchPlanBridgeStatusDto, SearchPlanCandidateWindowDto,
    SearchPlanChannelDto, SearchPlanDroppedTermDto, SearchPlanDto, SearchPlanNextActionDto,
    SearchPlanPromotionStatusDto, SearchPlanRejectedHitDto, SearchPlanSubqueryDto,
    SearchPlanTermsDto, SearchQueryAssessmentDto, SearchRepoTextMode, SearchRequest,
    SearchResultsDto, SemanticFallbackRecordDto, SemanticModeDto, SetUiLayoutRequest,
    SnippetContextDto, SnippetScopeDto, SourceOccurrenceDto, StartIndexingRequest, StorageStatsDto,
    StoredSemanticDocsContractDto, SummaryGenerationDto, SymbolContextDto, SymbolSummaryDto,
    SystemActionResponse, TrailConfigDto, TrailContextDto, TrailFilterOptionsDto, TrailStoryDto,
    TrailStoryStepDto, UpdateBookmarkCategoryRequest, UpdateBookmarkRequest,
    WorkspaceMemberIndexDto, WriteFileDataUrlRequest, WriteFileResponse, WriteFileTextRequest,
};
pub use errors::{
    ApiError, ApiErrorDetails, COMMAND_FAILURE_SCHEMA_VERSION, CommandFailureEnvelope,
    EmbeddingCapacityPressureDto,
};
pub use events::{AppEventPayload, IndexingPhaseTimings};
pub use ids::{EdgeId, NodeId};
pub use types::{
    EdgeKind, IndexMode, LayoutDirection, MemberAccess, NodeKind, TrailCallerScope, TrailDirection,
    TrailMode,
};
