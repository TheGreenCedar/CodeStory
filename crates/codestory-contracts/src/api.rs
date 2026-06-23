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
    AffectedAnalysisDto, AffectedAnalysisRequest, AffectedChangeKindDto, AffectedChangeRecordDto,
    AffectedMatchedFileDto, AffectedRouteDto, AffectedSymbolDto, AffectedTestFileDto,
    AffectedUnmatchedPathDto, AgentAnswerDto, AgentAskRequest, AgentCitationDto,
    AgentCustomRetrievalConfigDto, AgentHybridWeightsDto, AgentPacketDto, AgentPacketRequestDto,
    AgentResponseBlockDto, AgentResponseModeDto, AgentResponseSectionDto,
    AgentRetrievalPolicyModeDto, AgentRetrievalPresetDto, AgentRetrievalProfileSelectionDto,
    AgentRetrievalStepDto, AgentRetrievalStepKindDto, AgentRetrievalStepStatusDto,
    AgentRetrievalSummaryFieldDto, AgentRetrievalTraceDto, AnswerReadinessReportDto,
    BookmarkCategoryDto, BookmarkDto, CanonicalEdgeDto, CanonicalEdgeFamily, CanonicalLayoutDto,
    CanonicalMemberDto, CanonicalMemberVisibility, CanonicalNodeDto, CanonicalNodeStyle,
    CanonicalRouteKind, ClaimReadinessDto, CreateBookmarkCategoryRequest, CreateBookmarkRequest,
    EdgeOccurrencesRequest, EmbeddingProfileContractDto, EvidenceItemDto, EvidencePacketDto,
    EvidenceSourceLocationDto, EvidenceTypeDto, FrameworkRouteCoverageDto, GraphArtifactDto,
    GraphEdgeDto, GraphNodeDto, GraphRequest, GraphResponse, GroundingBudgetDto,
    GroundingCoverageBucketDto, GroundingCoverageDto, GroundingFileDigestDto, GroundingSnapshotDto,
    GroundingSymbolDigestDto, IndexDryRunDto, IndexFreshnessChangeKindDto, IndexFreshnessDto,
    IndexFreshnessSampleDto, IndexFreshnessStatusDto, IndexedFileDto, IndexedFileLanguageCountDto,
    IndexedFileRoleDto, IndexedFilesDto, IndexedFilesRequest, IndexedFilesSummaryDto,
    ListChildrenSymbolsRequest, ListRootSymbolsRequest, NodeDetailsDto, NodeDetailsRequest,
    NodeOccurrencesRequest, OpenContainingFolderRequest, OpenDefinitionRequest, OpenProjectRequest,
    PacketBudgetDto, PacketBudgetLimitsDto, PacketBudgetModeDto, PacketBudgetUsageDto,
    PacketClaimDto, PacketCoverageReportDto, PacketEvidenceResolutionDto, PacketEvidenceTierDto,
    PacketPlanDto, PacketPlanQueryDto, PacketRetrievalTraceSummaryDto,
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
    SnippetContextDto, SnippetScopeDto, SourceOccurrenceDto, SourceTruthCheckDto,
    StartIndexingRequest, StorageStatsDto, StoredSemanticDocsContractDto, SummaryGenerationDto,
    SymbolContextDto, SymbolSummaryDto, SystemActionResponse, TrailConfigDto, TrailContextDto,
    TrailFilterOptionsDto, TrailStoryDto, TrailStoryStepDto, UpdateBookmarkCategoryRequest,
    UpdateBookmarkRequest, WorkspaceMemberIndexDto, WriteFileDataUrlRequest, WriteFileResponse,
    WriteFileTextRequest,
};
pub use errors::{ApiError, ApiErrorDetails};
pub use events::{AppEventPayload, IndexingPhaseTimings};
pub use ids::{EdgeId, NodeId};
pub use types::{
    EdgeKind, IndexMode, LayoutDirection, MemberAccess, NodeKind, TrailCallerScope, TrailDirection,
    TrailMode,
};
