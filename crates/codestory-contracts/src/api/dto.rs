use super::ids::{EdgeId, NodeId};
use super::types::{
    EdgeKind, IndexMode, LayoutDirection, MemberAccess, NodeKind, TrailCallerScope, TrailDirection,
    TrailMode,
};
use serde::{Deserialize, Serialize};
use specta::Type;

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct OpenProjectRequest {
    pub path: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, Default)]
#[serde(rename_all = "snake_case")]
pub enum GroundingBudgetDto {
    Strict,
    #[default]
    Balanced,
    Max,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct StorageStatsDto {
    // Use u32 so TS can safely represent these as `number` without BigInt.
    pub node_count: u32,
    pub edge_count: u32,
    pub file_count: u32,
    pub error_count: u32,
    #[serde(default)]
    pub fatal_error_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct ProjectSummary {
    pub root: String,
    pub stats: StorageStatsDto,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub members: Vec<WorkspaceMemberIndexDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retrieval: Option<RetrievalStateDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub freshness: Option<IndexFreshnessDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct StartIndexingRequest {
    pub mode: IndexMode,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SearchRepoTextMode {
    #[default]
    Auto,
    On,
    Off,
}

fn default_search_limit_per_source() -> u32 {
    10
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct SearchRequest {
    pub query: String,
    #[serde(default)]
    pub repo_text: SearchRepoTextMode,
    #[serde(default = "default_search_limit_per_source")]
    pub limit_per_source: u32,
    #[serde(default)]
    pub expand_search_plan: bool,
    #[serde(default)]
    pub hybrid_weights: Option<AgentHybridWeightsDto>,
    #[serde(default)]
    pub hybrid_limits: Option<SearchHybridLimitsDto>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SearchHitOrigin {
    IndexedSymbol,
    TextMatch,
}

impl SearchHitOrigin {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::IndexedSymbol => "indexed_symbol",
            Self::TextMatch => "text_match",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SearchMatchQualityDto {
    Exact,
    NormalizedExact,
    Prefix,
    Fuzzy,
    SemanticSuggestion,
    RepoText,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PacketEvidenceTierDto {
    ExactSource,
    ResolvedGraph,
    LexicalSource,
    SymbolDoc,
    ComponentReport,
    DenseSemantic,
    SyntheticSourceScan,
    GeneratedSummary,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PacketEvidenceResolutionDto {
    Resolved,
    SourceRangeOnly,
    Unresolved,
    DiagnosticOnly,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RetrievalModeDto {
    Hybrid,
    Symbolic,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RetrievalFallbackReasonDto {
    DisabledByConfig,
    MissingEmbeddingRuntime,
    MissingSemanticDocs,
    DegradedRuntime,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum SemanticModeDto {
    #[default]
    DisabledByConfig,
    DegradedRuntime,
    Enabled,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
pub struct SemanticFallbackRecordDto {
    pub query: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct RetrievalStateDto {
    pub mode: RetrievalModeDto,
    pub hybrid_configured: bool,
    pub semantic_ready: bool,
    #[serde(default)]
    pub semantic_mode: SemanticModeDto,
    pub semantic_doc_count: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_embedding: Option<EmbeddingProfileContractDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stored_embedding: Option<StoredSemanticDocsContractDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_reason: Option<RetrievalFallbackReasonDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
pub struct EmbeddingProfileContractDto {
    pub profile: String,
    pub backend: String,
    pub model_id: String,
    pub cache_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dimension: Option<u32>,
    pub doc_shape: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
pub struct StoredSemanticDocsContractDto {
    pub doc_count: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding_profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding_backend: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dimension: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doc_version: Option<u32>,
    #[serde(default)]
    pub mixed_embedding_profiles: bool,
    #[serde(default)]
    pub mixed_embedding_models: bool,
    #[serde(default)]
    pub mixed_embedding_backends: bool,
    #[serde(default)]
    pub mixed_dimensions: bool,
    #[serde(default)]
    pub mixed_doc_versions: bool,
    #[serde(default)]
    pub mixed_doc_shapes: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doc_shape: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantic_policy_version: Option<String>,
    #[serde(default)]
    pub mixed_semantic_policy_versions: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IndexFreshnessStatusDto {
    Fresh,
    Stale,
    NotChecked,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IndexFreshnessChangeKindDto {
    Changed,
    New,
    Removed,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
pub struct IndexFreshnessSampleDto {
    pub kind: IndexFreshnessChangeKindDto,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
pub struct IndexFreshnessDto {
    pub status: IndexFreshnessStatusDto,
    pub changed_file_count: u32,
    pub new_file_count: u32,
    pub removed_file_count: u32,
    pub checked_file_count: u32,
    pub indexed_file_count: u32,
    pub duration_ms: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub samples: Vec<IndexFreshnessSampleDto>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReadinessGoalDto {
    LocalNavigation,
    AgentPacketSearch,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReadinessStatusDto {
    Ready,
    RepairIndex,
    CheckIndex,
    RepairRetrieval,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
pub struct ReadinessIndexSnapshotDto {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<IndexFreshnessStatusDto>,
    #[serde(default)]
    pub error_count: u32,
    #[serde(default)]
    pub fatal_error_count: u32,
    pub changed_file_count: u32,
    pub new_file_count: u32,
    pub removed_file_count: u32,
    pub checked_file_count: u32,
    pub indexed_file_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
pub struct ReadinessSidecarSnapshotDto {
    pub retrieval_mode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub degraded_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest_generation: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest_input_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
pub struct ReadinessVerdictDto {
    pub goal: ReadinessGoalDto,
    pub status: ReadinessStatusDto,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub minimum_next: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub full_repair: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<ReadinessIndexSnapshotDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sidecar: Option<ReadinessSidecarSnapshotDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct SearchHit {
    pub node_id: NodeId,
    pub display_name: String,
    pub kind: NodeKind,
    pub file_path: Option<String>,
    pub line: Option<u32>,
    pub score: f32,
    pub origin: SearchHitOrigin,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub match_quality: Option<SearchMatchQualityDto>,
    pub resolvable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_tier: Option<PacketEvidenceTierDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_producer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution_status: Option<PacketEvidenceResolutionDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loss_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coverage_role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eligible_for_sufficiency: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score_breakdown: Option<RetrievalScoreBreakdownDto>,
}

impl SearchHit {
    pub const fn is_text_match(&self) -> bool {
        matches!(self.origin, SearchHitOrigin::TextMatch)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct RepoTextScanStatsDto {
    pub scanned_file_count: u32,
    pub scanned_byte_count: u32,
    pub skipped_large_file_count: u32,
    pub file_cap: u32,
    pub byte_cap: u32,
    pub time_cap_ms: u32,
    pub duration_ms: u32,
    pub truncated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct SearchQueryAssessmentDto {
    pub exact_symbol_hit_count: u32,
    pub weak_top_hit: bool,
    pub stale_or_missing_anchor: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo_text_fallback_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommended_next_action: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SearchPlanChannelDto {
    TypedSymbol,
    Lexical,
    Semantic,
    RepoText,
    Bridge,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SearchPlanPromotionStatusDto {
    TypedAnchor,
    Promoted,
    NeedsSourceRead,
    Ambiguous,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SearchPlanBridgeStatusDto {
    Supported,
    Partial,
    Unsupported,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SearchPlanBridgeConfidenceDto {
    High,
    Medium,
    Low,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SearchPlanBridgeEvidenceKindDto {
    SameAnchor,
    GraphPath,
    FrameworkRoute,
    ComponentUsage,
    DataCollectionUsage,
    SharedFile,
    RepoTextHint,
    SourceTruthOnly,
    IsolatedAnchors,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct SearchPlanDroppedTermDto {
    pub term: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct SearchPlanTermsDto {
    #[serde(default)]
    pub extracted: Vec<String>,
    #[serde(default)]
    pub dropped: Vec<SearchPlanDroppedTermDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct SearchPlanSubqueryDto {
    pub query: String,
    pub role: String,
    #[serde(default)]
    pub channels: Vec<SearchPlanChannelDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct SearchPlanCandidateWindowDto {
    pub channel: SearchPlanChannelDto,
    pub subquery: String,
    pub limit: u32,
    pub returned_count: u32,
    pub truncated: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub score_reasons: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct SearchPlanAnchorGroupDto {
    pub anchor: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chosen_symbol: Option<SearchHit>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub supporting_hits: Vec<SearchHit>,
    pub promotion_status: SearchPlanPromotionStatusDto,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub promotion_method: Option<String>,
    #[serde(default)]
    pub caller_count: u32,
    #[serde(default)]
    pub definition_only: bool,
    #[serde(default)]
    pub no_visible_callers: bool,
    pub confidence: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct SearchPlanBridgeDto {
    pub from_anchor: String,
    pub to_anchor: String,
    pub status: SearchPlanBridgeStatusDto,
    pub confidence: SearchPlanBridgeConfidenceDto,
    pub evidence_kind: SearchPlanBridgeEvidenceKindDto,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direction: Option<String>,
    pub node_count: u32,
    pub edge_count: u32,
    pub truncated: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct SearchPlanRejectedHitDto {
    pub display_name: String,
    pub reason: String,
    pub origin: SearchHitOrigin,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct SearchPlanNextActionDto {
    pub action: String,
    pub node_id: NodeId,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct SearchPlanDto {
    pub original_query: String,
    pub eligible: bool,
    #[serde(default)]
    pub intents: Vec<String>,
    pub terms: SearchPlanTermsDto,
    #[serde(default)]
    pub subqueries: Vec<SearchPlanSubqueryDto>,
    #[serde(default)]
    pub candidate_windows: Vec<SearchPlanCandidateWindowDto>,
    #[serde(default)]
    pub anchor_groups: Vec<SearchPlanAnchorGroupDto>,
    #[serde(default)]
    pub bridges: Vec<SearchPlanBridgeDto>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rejected_hits: Vec<SearchPlanRejectedHitDto>,
    #[serde(default)]
    pub next_actions: Vec<SearchPlanNextActionDto>,
    #[serde(default)]
    pub source_truth_checks: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct SearchResultsDto {
    pub query: String,
    pub retrieval: RetrievalStateDto,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retrieval_shadow: Option<RetrievalShadowDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub freshness: Option<IndexFreshnessDto>,
    pub limit_per_source: u32,
    pub repo_text_mode: SearchRepoTextMode,
    pub repo_text_enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query_assessment: Option<SearchQueryAssessmentDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search_plan: Option<SearchPlanDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo_text_stats: Option<RepoTextScanStatsDto>,
    #[serde(default)]
    pub suggestions: Vec<SearchHit>,
    #[serde(default)]
    pub indexed_symbol_hits: Vec<SearchHit>,
    #[serde(default)]
    pub repo_text_hits: Vec<SearchHit>,
    pub hits: Vec<SearchHit>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum IndexedFileRoleDto {
    Source,
    Test,
    Generated,
    Vendor,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct IndexedFilesRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path_contains: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<IndexedFileRoleDto>,
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct IndexedFileDto {
    pub path: String,
    pub language: String,
    pub indexed: bool,
    pub complete: bool,
    pub line_count: u32,
    pub role: IndexedFileRoleDto,
    #[serde(default)]
    pub error_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct IndexedFileLanguageCountDto {
    pub language: String,
    pub file_count: u32,
    pub support_mode: String,
    pub evidence_tier: String,
    pub claim_label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct IndexedFilesSummaryDto {
    pub file_count: u32,
    pub indexed_file_count: u32,
    #[serde(default)]
    pub filtered_file_count: u32,
    #[serde(default)]
    pub visible_file_count: u32,
    pub incomplete_file_count: u32,
    pub error_file_count: u32,
    pub truncated: bool,
    pub language_counts: Vec<IndexedFileLanguageCountDto>,
    #[serde(default)]
    pub framework_route_coverage: Vec<FrameworkRouteCoverageDto>,
    #[serde(default)]
    pub coverage_notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct IndexedFilesDto {
    pub project_root: String,
    pub usable: bool,
    pub summary: IndexedFilesSummaryDto,
    pub files: Vec<IndexedFileDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
pub struct FrameworkRouteCoverageDto {
    pub framework: String,
    pub language: String,
    pub status: String,
    #[serde(alias = "fixture_status")]
    pub coverage_evidence: String,
    pub confidence_floor: String,
    pub handler_link_support: String,
    #[serde(default)]
    pub unsupported_patterns: Vec<String>,
    #[serde(default)]
    pub known_gaps: Vec<String>,
    pub promotable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AffectedChangeKindDto {
    Added,
    Modified,
    Deleted,
    Renamed,
    Copied,
    Untracked,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct AffectedChangeRecordDto {
    pub path: String,
    pub kind: AffectedChangeKindDto,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct AffectedAnalysisRequest {
    pub changed_paths: Vec<String>,
    #[serde(default)]
    pub change_records: Vec<AffectedChangeRecordDto>,
    #[serde(default)]
    pub depth: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct AffectedSymbolDto {
    pub node_id: NodeId,
    pub display_name: String,
    pub kind: NodeKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    pub distance: u32,
    #[serde(default)]
    pub graph_depth: u32,
    pub reason: String,
    pub confidence: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct AffectedMatchedFileDto {
    pub path: String,
    pub role: IndexedFileRoleDto,
    pub indexed: bool,
    pub complete: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change_kind: Option<AffectedChangeKindDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change_status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_path: Option<String>,
    #[serde(default)]
    pub error_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct AffectedUnmatchedPathDto {
    pub path: String,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change_kind: Option<AffectedChangeKindDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change_status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct AffectedRouteDto {
    pub node_id: NodeId,
    pub display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    pub distance: u32,
    #[serde(default)]
    pub graph_depth: u32,
    pub reason: String,
    pub confidence: String,
    pub route: RouteEndpointMetadataDto,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct AffectedTestFileDto {
    pub path: String,
    pub reason: String,
    pub confidence: String,
    pub distance: u32,
    #[serde(default)]
    pub graph_depth: u32,
    pub impacted_symbol_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct AffectedAnalysisDto {
    pub project_root: String,
    pub changed_paths: Vec<String>,
    #[serde(default)]
    pub change_records: Vec<AffectedChangeRecordDto>,
    #[serde(default)]
    pub matched_files: Vec<AffectedMatchedFileDto>,
    #[serde(default)]
    pub unmatched_paths: Vec<AffectedUnmatchedPathDto>,
    pub matched_file_count: u32,
    pub depth: u32,
    pub impacted_symbols: Vec<AffectedSymbolDto>,
    #[serde(default)]
    pub impacted_routes: Vec<AffectedRouteDto>,
    pub impacted_tests: Vec<AffectedTestFileDto>,
    #[serde(default)]
    pub blind_spots: Vec<String>,
    #[serde(default)]
    pub next_commands: Vec<String>,
    #[serde(default)]
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct ListRootSymbolsRequest {
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct ListChildrenSymbolsRequest {
    pub parent_id: NodeId,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct SymbolSummaryDto {
    pub id: NodeId,
    pub label: String,
    pub kind: NodeKind,
    pub file_path: Option<String>,
    pub has_children: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct GroundingSymbolDigestDto {
    pub id: NodeId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_ref: Option<String>,
    pub label: String,
    pub kind: NodeKind,
    #[serde(default)]
    pub line: Option<u32>,
    #[serde(default)]
    pub member_count: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default)]
    pub edge_digest: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct GroundingFileDigestDto {
    pub file_path: String,
    #[serde(default)]
    pub language: Option<String>,
    pub symbol_count: u32,
    pub represented_symbol_count: u32,
    pub compressed: bool,
    pub symbols: Vec<GroundingSymbolDigestDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct GroundingCoverageBucketDto {
    pub label: String,
    pub file_count: u32,
    pub symbol_count: u32,
    #[serde(default)]
    pub sample_paths: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct GroundingCoverageDto {
    pub total_files: u32,
    pub represented_files: u32,
    pub total_symbols: u32,
    pub represented_symbols: u32,
    pub compressed_files: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct GroundingSnapshotDto {
    pub root: String,
    pub budget: GroundingBudgetDto,
    pub generated_at_epoch_ms: i64,
    pub stats: StorageStatsDto,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retrieval: Option<RetrievalStateDto>,
    pub coverage: GroundingCoverageDto,
    pub root_symbols: Vec<GroundingSymbolDigestDto>,
    pub files: Vec<GroundingFileDigestDto>,
    #[serde(default)]
    pub coverage_buckets: Vec<GroundingCoverageBucketDto>,
    #[serde(default)]
    pub notes: Vec<String>,
    #[serde(default)]
    pub recommended_queries: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct GraphRequest {
    pub center_id: NodeId,
    /// Optional cap to avoid pulling extremely dense neighborhoods into the UI.
    pub max_edges: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct GraphNodeDto {
    pub id: NodeId,
    pub label: String,
    pub kind: NodeKind,
    pub depth: u32,
    #[serde(default)]
    pub label_policy: Option<String>,
    #[serde(default)]
    pub badge_visible_members: Option<u32>,
    #[serde(default)]
    pub badge_total_members: Option<u32>,
    #[serde(default)]
    pub merged_symbol_examples: Vec<String>,
    #[serde(default)]
    pub file_path: Option<String>,
    #[serde(default)]
    pub qualified_name: Option<String>,
    #[serde(default)]
    pub member_access: Option<MemberAccess>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct GraphEdgeDto {
    pub id: EdgeId,
    pub source: NodeId,
    pub target: NodeId,
    pub kind: EdgeKind,
    #[serde(default)]
    pub confidence: Option<f32>,
    /// `certain`, `probable`, or `uncertain`.
    #[serde(default)]
    pub certainty: Option<String>,
    #[serde(default)]
    pub callsite_identity: Option<String>,
    #[serde(default)]
    pub candidate_targets: Vec<NodeId>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct GraphResponse {
    pub center_id: NodeId,
    pub nodes: Vec<GraphNodeDto>,
    pub edges: Vec<GraphEdgeDto>,
    pub truncated: bool,
    #[serde(default)]
    pub omitted_edge_count: u32,
    #[serde(default)]
    pub canonical_layout: Option<CanonicalLayoutDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct CanonicalLayoutDto {
    pub schema_version: u32,
    pub center_node_id: NodeId,
    pub nodes: Vec<CanonicalNodeDto>,
    pub edges: Vec<CanonicalEdgeDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct CanonicalNodeDto {
    pub id: NodeId,
    pub kind: NodeKind,
    pub label: String,
    pub center: bool,
    pub node_style: CanonicalNodeStyle,
    pub is_non_indexed: bool,
    pub duplicate_count: u32,
    #[serde(default)]
    pub merged_symbol_ids: Vec<NodeId>,
    pub member_count: u32,
    #[serde(default)]
    pub badge_visible_members: Option<u32>,
    #[serde(default)]
    pub badge_total_members: Option<u32>,
    #[serde(default)]
    pub members: Vec<CanonicalMemberDto>,
    pub x_rank: i32,
    pub y_rank: u32,
    pub width: f32,
    pub height: f32,
    pub is_virtual_bundle: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct CanonicalEdgeDto {
    pub id: String,
    #[serde(default)]
    pub source_edge_ids: Vec<EdgeId>,
    pub source: NodeId,
    pub target: NodeId,
    pub source_handle: String,
    pub target_handle: String,
    pub kind: EdgeKind,
    #[serde(default)]
    pub certainty: Option<String>,
    pub multiplicity: u32,
    pub family: CanonicalEdgeFamily,
    pub route_kind: CanonicalRouteKind,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct CanonicalMemberDto {
    pub id: NodeId,
    pub label: String,
    pub kind: NodeKind,
    pub visibility: CanonicalMemberVisibility,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CanonicalNodeStyle {
    Card,
    Pill,
    Bundle,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CanonicalEdgeFamily {
    Flow,
    Hierarchy,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CanonicalRouteKind {
    Direct,
    Hierarchy,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CanonicalMemberVisibility {
    Public,
    Protected,
    Private,
    Default,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct TrailConfigDto {
    pub root_id: NodeId,
    #[serde(default)]
    pub mode: TrailMode,
    #[serde(default)]
    pub target_id: Option<NodeId>,
    /// Use `0` to mean "infinite" (bounded by `max_nodes`).
    pub depth: u32,
    pub direction: TrailDirection,
    #[serde(default)]
    pub caller_scope: TrailCallerScope,
    pub edge_filter: Vec<EdgeKind>,
    #[serde(default = "default_show_utility_calls")]
    pub show_utility_calls: bool,
    #[serde(default)]
    pub hide_speculative: bool,
    #[serde(default)]
    pub story: bool,
    #[serde(default)]
    pub node_filter: Vec<NodeKind>,
    pub max_nodes: u32,
    #[serde(default = "default_layout_direction")]
    pub layout_direction: LayoutDirection,
}

const fn default_show_utility_calls() -> bool {
    false
}

const fn default_layout_direction() -> LayoutDirection {
    LayoutDirection::Horizontal
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct TrailFilterOptionsDto {
    pub node_kinds: Vec<NodeKind>,
    pub edge_kinds: Vec<EdgeKind>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct TrailContextDto {
    pub focus: NodeDetailsDto,
    pub trail: GraphResponse,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub story: Option<TrailStoryDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct TrailStoryDto {
    pub summary: String,
    pub entry_points: Vec<String>,
    pub core_flow: Vec<TrailStoryStepDto>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub runtime_flow: Vec<TrailStoryStepDto>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub data_flow: Vec<TrailStoryStepDto>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub type_structure: Vec<TrailStoryStepDto>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub utility_calls: Vec<TrailStoryStepDto>,
    pub side_effects: Vec<String>,
    pub uncertainty: Vec<String>,
    pub test_scope: Vec<String>,
    pub limits: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct TrailStoryStepDto {
    pub edge_id: String,
    pub source: String,
    pub relation: String,
    pub target: String,
    pub certainty: String,
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct NodeDetailsRequest {
    pub id: NodeId,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct NodeOccurrencesRequest {
    pub id: NodeId,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct EdgeOccurrencesRequest {
    pub id: EdgeId,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct SourceOccurrenceDto {
    pub element_id: String,
    pub kind: String,
    pub file_path: String,
    pub start_line: u32,
    pub start_col: u32,
    pub end_line: u32,
    pub end_col: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct NodeDetailsDto {
    pub id: NodeId,
    pub kind: NodeKind,
    pub display_name: String,
    pub serialized_name: String,
    pub qualified_name: Option<String>,
    pub canonical_id: Option<String>,
    pub file_path: Option<String>,
    pub start_line: Option<u32>,
    pub start_col: Option<u32>,
    pub end_line: Option<u32>,
    pub end_col: Option<u32>,
    #[serde(default)]
    pub member_access: Option<MemberAccess>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route_endpoint: Option<RouteEndpointMetadataDto>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RouteEndpointKindDto {
    FrameworkRoute,
    OpenapiEndpoint,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq)]
pub struct RouteEndpointMetadataDto {
    pub kind: RouteEndpointKindDto,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub framework: Option<String>,
    pub method: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_path: Option<String>,
    #[serde(default)]
    pub params: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_convention: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handler: Option<RouteEndpointHandlerDto>,
    #[serde(default)]
    pub provenance: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq)]
pub struct RouteEndpointHandlerDto {
    pub node_id: NodeId,
    pub display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub certainty: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct SymbolContextDto {
    pub node: NodeDetailsDto,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default)]
    pub children: Vec<SymbolSummaryDto>,
    #[serde(default)]
    pub related_hits: Vec<SearchHit>,
    #[serde(default)]
    pub edge_digest: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct ReadFileTextRequest {
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct ReadFileTextResponse {
    pub path: String,
    pub text: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum SnippetScopeDto {
    #[default]
    LineContext,
    FunctionBody,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct SnippetContextDto {
    pub node: NodeDetailsDto,
    pub path: String,
    pub line: u32,
    pub snippet: String,
    #[serde(default)]
    pub scope: SnippetScopeDto,
    #[serde(default)]
    pub requested_context: u32,
    #[serde(default)]
    pub snippet_truncated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_snippet_bytes: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truncation_guidance: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct IndexDryRunDto {
    pub root: String,
    pub storage_path: String,
    pub refresh: IndexMode,
    pub files_to_index: u32,
    pub files_to_remove: u32,
    #[serde(default)]
    pub sample_files_to_index: Vec<String>,
    #[serde(default)]
    pub sample_file_ids_to_remove: Vec<i64>,
    #[serde(default)]
    pub members: Vec<WorkspaceMemberIndexDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct WorkspaceMemberIndexDto {
    pub path: String,
    pub files_to_index: u32,
    pub indexed_files: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_count: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_count: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edge_count: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct SummaryGenerationDto {
    pub generated: u32,
    pub reused: u32,
    pub skipped: u32,
    pub endpoint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct WriteFileTextRequest {
    pub path: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct SetUiLayoutRequest {
    pub json: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentRetrievalPresetDto {
    #[default]
    Architecture,
    Callflow,
    Inheritance,
    Impact,
    Investigate,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentRetrievalPolicyModeDto {
    LatencyFirst,
    CompletenessFirst,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct AgentCustomRetrievalConfigDto {
    /// Use `0` to mean "infinite" (bounded by `max_nodes`).
    #[serde(default = "default_custom_depth")]
    pub depth: u32,
    #[serde(default = "default_custom_direction")]
    pub direction: TrailDirection,
    #[serde(default)]
    pub edge_filter: Vec<EdgeKind>,
    #[serde(default)]
    pub node_filter: Vec<NodeKind>,
    #[serde(default = "default_custom_max_nodes")]
    pub max_nodes: u32,
    #[serde(default)]
    pub include_edge_occurrences: bool,
    #[serde(default = "default_custom_enable_source_reads")]
    pub enable_source_reads: bool,
}

const fn default_custom_depth() -> u32 {
    3
}

const fn default_custom_direction() -> TrailDirection {
    TrailDirection::Both
}

const fn default_custom_max_nodes() -> u32 {
    800
}

const fn default_custom_enable_source_reads() -> bool {
    true
}

impl Default for AgentCustomRetrievalConfigDto {
    fn default() -> Self {
        Self {
            depth: default_custom_depth(),
            direction: default_custom_direction(),
            edge_filter: Vec::new(),
            node_filter: Vec::new(),
            max_nodes: default_custom_max_nodes(),
            include_edge_occurrences: false,
            enable_source_reads: default_custom_enable_source_reads(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, Default)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AgentRetrievalProfileSelectionDto {
    #[default]
    Auto,
    Preset {
        preset: AgentRetrievalPresetDto,
    },
    Custom {
        config: AgentCustomRetrievalConfigDto,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentResponseModeDto {
    #[default]
    Markdown,
    Structured,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, Default)]
pub struct AgentHybridWeightsDto {
    #[serde(default)]
    pub lexical: Option<f32>,
    #[serde(default)]
    pub semantic: Option<f32>,
    #[serde(default)]
    pub graph: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, Default)]
pub struct SearchHybridLimitsDto {
    #[serde(default)]
    pub lexical: Option<u32>,
    #[serde(default)]
    pub semantic: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct AgentAskRequest {
    pub prompt: String,
    #[serde(default)]
    pub retrieval_profile: AgentRetrievalProfileSelectionDto,
    #[serde(default)]
    pub focus_node_id: Option<NodeId>,
    #[serde(default)]
    pub max_results: Option<u32>,
    #[serde(default)]
    pub response_mode: AgentResponseModeDto,
    #[serde(default)]
    pub latency_budget_ms: Option<u32>,
    #[serde(default = "default_include_evidence")]
    pub include_evidence: bool,
    #[serde(default)]
    pub hybrid_weights: Option<AgentHybridWeightsDto>,
}

const fn default_include_evidence() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct RetrievalScoreBreakdownDto {
    pub lexical: f32,
    pub semantic: f32,
    pub graph: f32,
    pub total: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier_cap: Option<f32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub boosts: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dampening: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub final_rank_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub provenance: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct AgentCitationDto {
    pub node_id: NodeId,
    pub display_name: String,
    pub kind: NodeKind,
    pub file_path: Option<String>,
    pub line: Option<u32>,
    pub score: f32,
    #[serde(default = "default_search_hit_origin")]
    pub origin: SearchHitOrigin,
    #[serde(default = "default_citation_resolvable")]
    pub resolvable: bool,
    #[serde(default)]
    pub subgraph_id: Option<String>,
    #[serde(default)]
    pub evidence_edge_ids: Vec<EdgeId>,
    #[serde(default)]
    pub retrieval_score_breakdown: Option<RetrievalScoreBreakdownDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_tier: Option<PacketEvidenceTierDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_producer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution_status: Option<PacketEvidenceResolutionDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loss_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coverage_role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eligible_for_sufficiency: Option<bool>,
}

const fn default_search_hit_origin() -> SearchHitOrigin {
    SearchHitOrigin::IndexedSymbol
}

const fn default_citation_resolvable() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AgentResponseBlockDto {
    Markdown { markdown: String },
    Mermaid { graph_id: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct AgentResponseSectionDto {
    pub id: String,
    pub title: String,
    pub blocks: Vec<AgentResponseBlockDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GraphArtifactDto {
    Uml {
        id: String,
        title: String,
        graph: GraphResponse,
    },
    Mermaid {
        id: String,
        title: String,
        diagram: String,
        mermaid_syntax: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct AgentRetrievalSummaryFieldDto {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentRetrievalStepKindDto {
    Search,
    SemanticQueryEmbedding,
    SemanticCandidateRetrieval,
    HybridRerank,
    QueryExpansion,
    RepoTextFallback,
    TrailFilterOptions,
    Neighborhood,
    Trail,
    NodeDetails,
    NodeOccurrences,
    EdgeOccurrences,
    SourceRead,
    MermaidSynthesis,
    AnswerSynthesis,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentRetrievalStepStatusDto {
    Ok,
    Error,
    Skipped,
    Truncated,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct AgentRetrievalStepDto {
    pub kind: AgentRetrievalStepKindDto,
    pub status: AgentRetrievalStepStatusDto,
    pub duration_ms: u32,
    #[serde(default)]
    pub input: Vec<AgentRetrievalSummaryFieldDto>,
    #[serde(default)]
    pub output: Vec<AgentRetrievalSummaryFieldDto>,
    #[serde(default)]
    pub message: Option<String>,
}

/// Per-stage timing from sidecar retrieval shadow runs.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct RetrievalStageTimingDto {
    pub stage: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline_ms: Option<u32>,
    pub elapsed_ms: u32,
    #[serde(default)]
    pub candidates_added: u32,
    #[serde(default)]
    pub marginal_gain: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cancel_reason: Option<String>,
    #[serde(default)]
    pub cache_hit: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sidecar_latency_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub degraded: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stub_reason: Option<String>,
}

fn is_false(value: &bool) -> bool {
    !*value
}

fn is_zero_u32(value: &u32) -> bool {
    *value == 0
}

/// Truncated sidecar candidate row for shadow trace export.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct RetrievalCandidateSummaryDto {
    pub rank: u32,
    pub file_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol_name: Option<String>,
    pub score: f32,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub admission_status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loss_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_node_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search_hit_rank: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub final_rank: Option<u32>,
}

/// Aggregated sidecar candidate resolution labels for loss-point diagnostics.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct RetrievalCandidateResolutionCountDto {
    pub resolution: String,
    pub count: u32,
}

/// Shadow sidecar retrieval diagnostics emitted alongside packet output.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct RetrievalShadowDto {
    pub retrieval_mode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub degraded_reason: Option<String>,
    pub retrieval_total_ms: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_budget_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cancel_reason: Option<String>,
    #[serde(default)]
    pub cache_hit: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stage_timings: Vec<RetrievalStageTimingDto>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub candidates: Vec<RetrievalCandidateSummaryDto>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub would_rank: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default)]
    pub candidate_count: u32,
    #[serde(default)]
    pub resolved_hit_count: u32,
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub unresolved_candidate_count: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub candidate_resolution_counts: Vec<RetrievalCandidateResolutionCountDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
pub struct PacketSidecarQueryDiagnosticDto {
    pub query: String,
    pub retrieval_mode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sidecar_query_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_resolution_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_elapsed_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub sidecar_stage_count: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sidecar_stage_total_ms: Option<u32>,
    pub candidate_count: u32,
    pub resolved_hit_count: u32,
    pub unresolved_candidate_count: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostic: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct AgentRetrievalTraceDto {
    pub request_id: String,
    pub resolved_profile: AgentRetrievalPresetDto,
    pub policy_mode: AgentRetrievalPolicyModeDto,
    pub total_latency_ms: u32,
    #[serde(default)]
    pub sla_target_ms: Option<u32>,
    #[serde(default)]
    pub sla_missed: bool,
    #[serde(default)]
    pub semantic_fallback_count: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub semantic_fallbacks: Vec<SemanticFallbackRecordDto>,
    #[serde(default)]
    pub annotations: Vec<String>,
    pub steps: Vec<AgentRetrievalStepDto>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub packet_sidecar_diagnostics: Vec<PacketSidecarQueryDiagnosticDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retrieval_shadow: Option<RetrievalShadowDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct AgentAnswerDto {
    pub answer_id: String,
    pub prompt: String,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub freshness: Option<IndexFreshnessDto>,
    pub sections: Vec<AgentResponseSectionDto>,
    pub citations: Vec<AgentCitationDto>,
    pub subgraph_ids: Vec<String>,
    pub retrieval_version: String,
    pub graphs: Vec<GraphArtifactDto>,
    pub retrieval_trace: AgentRetrievalTraceDto,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceTypeDto {
    SearchHit,
    SymbolContext,
    Trail,
    Snippet,
    Explore,
    Bridge,
    RepoText,
    Negative,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PacketBudgetModeDto {
    Tiny,
    #[default]
    Compact,
    Standard,
    Deep,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ClaimReadinessDto {
    Anchored,
    Supported,
    Partial,
    Inferred,
    NeedsSourceRead,
    ContradictedBySource,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct EvidenceSourceLocationDto {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_start: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_end: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct EvidenceItemDto {
    pub id: String,
    pub evidence_type: EvidenceTypeDto,
    pub command: String,
    pub status: String,
    pub confidence: String,
    pub verification_status: ClaimReadinessDto,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub match_quality: Option<SearchMatchQualityDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<EvidenceSourceLocationDto>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct SourceTruthCheckDto {
    pub id: String,
    pub reason: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    pub required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct AnswerReadinessReportDto {
    pub overall_status: ClaimReadinessDto,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub safe_to_say: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inferred_claims: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub needs_verification: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub next_commands: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_truth_checks: Vec<SourceTruthCheckDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct EvidencePacketDto {
    pub packet_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub question: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<EvidenceItemDto>,
    pub readiness: AnswerReadinessReportDto,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PacketTaskClassDto {
    ArchitectureExplanation,
    BugLocalization,
    ChangeImpact,
    RouteTracing,
    SymbolOwnership,
    DataFlow,
    EditPlanning,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct PacketPlanQueryDto {
    pub query: String,
    pub purpose: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct PacketPlanDto {
    pub task_class: PacketTaskClassDto,
    pub inferred_task_class: bool,
    #[serde(default)]
    pub queries: Vec<PacketPlanQueryDto>,
    #[serde(default)]
    pub trace: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct AgentPacketRequestDto {
    pub question: String,
    #[serde(default)]
    pub budget: PacketBudgetModeDto,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_class: Option<PacketTaskClassDto>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extra_probes: Vec<String>,
    #[serde(default = "default_include_evidence")]
    pub include_evidence: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency_budget_ms: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct PacketBudgetLimitsDto {
    pub max_anchors: u32,
    pub max_files: u32,
    pub max_snippets: u32,
    pub max_trail_edges: u32,
    pub max_output_bytes: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct PacketBudgetUsageDto {
    pub anchors: u32,
    pub files: u32,
    pub snippets: u32,
    pub trail_edges: u32,
    pub output_bytes: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct PacketBudgetDto {
    pub requested: PacketBudgetModeDto,
    pub limits: PacketBudgetLimitsDto,
    pub used: PacketBudgetUsageDto,
    pub truncated: bool,
    #[serde(default)]
    pub omitted_sections: Vec<String>,
    pub next_deeper_command: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PacketSufficiencyStatusDto {
    Sufficient,
    Partial,
    #[serde(rename = "blocked", alias = "insufficient")]
    Insufficient,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct PacketClaimDto {
    pub claim: String,
    #[serde(default)]
    pub citations: Vec<AgentCitationDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coverage_role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eligible_for_sufficiency: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, Default)]
pub struct PacketCoverageReportDto {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub covered: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub provenance_labels: Vec<String>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub provenance_counts: std::collections::BTreeMap<String, u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub missing: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ineligible: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unresolved: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub budget_omitted: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct PacketSufficiencyDto {
    pub status: PacketSufficiencyStatusDto,
    #[serde(default)]
    pub covered_claims: Vec<PacketClaimDto>,
    #[serde(default)]
    pub open_next: Vec<String>,
    #[serde(default)]
    pub avoid_opening: Vec<String>,
    #[serde(default)]
    pub avoid_opening_paths: Vec<String>,
    #[serde(default)]
    pub gaps: Vec<String>,
    #[serde(default)]
    pub follow_up_commands: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coverage_report: Option<PacketCoverageReportDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct PacketRetrievalTraceSummaryDto {
    pub retrieval_trace: AgentRetrievalTraceDto,
    pub source_read_steps: u32,
    pub search_steps: u32,
    pub trail_steps: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct AgentPacketDto {
    pub packet_id: String,
    pub question: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_class: Option<PacketTaskClassDto>,
    pub plan: PacketPlanDto,
    pub answer: AgentAnswerDto,
    pub budget: PacketBudgetDto,
    pub sufficiency: PacketSufficiencyDto,
    #[serde(alias = "benchmark_trace")]
    pub retrieval_trace_summary: PacketRetrievalTraceSummaryDto,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct BookmarkCategoryDto {
    pub id: String,
    pub name: String,
}

#[cfg(test)]
mod packet_tests {
    use super::*;

    #[test]
    fn packet_request_uses_compact_budget_by_default() {
        let request: AgentPacketRequestDto =
            serde_json::from_str(r#"{"question":"explain indexing"}"#).expect("deserialize");

        assert_eq!(request.budget, PacketBudgetModeDto::Compact);
        assert!(request.include_evidence);
    }

    #[test]
    fn retrieval_shadow_serializes_snake_case_fields() {
        let shadow = RetrievalShadowDto {
            retrieval_mode: "full".to_string(),
            degraded_reason: None,
            retrieval_total_ms: 42,
            total_budget_ms: Some(1_000),
            cancel_reason: None,
            cache_hit: false,
            stage_timings: vec![RetrievalStageTimingDto {
                stage: "stage1_zoekt_lexical".to_string(),
                deadline_ms: Some(120),
                elapsed_ms: 18,
                candidates_added: 3,
                marginal_gain: 0.25,
                cancel_reason: None,
                cache_hit: false,
                sidecar_latency_ms: Some(18),
                degraded: false,
                stub_reason: None,
            }],
            candidates: vec![RetrievalCandidateSummaryDto {
                rank: 1,
                file_path: "src/lib.rs".to_string(),
                line: Some(12),
                symbol_name: Some("extension_service".to_string()),
                score: 0.9,
                source: "zoekt".to_string(),
                resolution: Some("node_unresolved".to_string()),
                admission_status: Some("unresolved".to_string()),
                loss_reason: Some("node_unresolved".to_string()),
                resolved_node_id: None,
                search_hit_rank: None,
                final_rank: None,
            }],
            would_rank: vec!["src/lib.rs".to_string()],
            error: None,
            candidate_count: 1,
            resolved_hit_count: 0,
            unresolved_candidate_count: 1,
            candidate_resolution_counts: vec![RetrievalCandidateResolutionCountDto {
                resolution: "node_unresolved".to_string(),
                count: 1,
            }],
        };
        let value = serde_json::to_value(&shadow).expect("serialize");
        assert_eq!(value["retrieval_mode"], "full");
        assert_eq!(value["retrieval_total_ms"], 42);
        assert_eq!(value["stage_timings"][0]["stage"], "stage1_zoekt_lexical");
        assert_eq!(value["candidates"][0]["source"], "zoekt");
        assert_eq!(value["candidates"][0]["line"], 12);
        assert_eq!(value["candidates"][0]["resolution"], "node_unresolved");
        assert_eq!(value["candidates"][0]["admission_status"], "unresolved");
        assert_eq!(value["candidates"][0]["loss_reason"], "node_unresolved");
        assert_eq!(value["unresolved_candidate_count"], 1);
        assert_eq!(
            value["candidate_resolution_counts"][0]["resolution"],
            "node_unresolved"
        );
        assert_eq!(value["would_rank"][0], "src/lib.rs");
        let parsed: RetrievalShadowDto = serde_json::from_value(value).expect("deserialize");
        assert_eq!(parsed.retrieval_mode, "full");
        assert_eq!(parsed.would_rank, vec!["src/lib.rs".to_string()]);
        assert_eq!(parsed.unresolved_candidate_count, 1);
    }

    #[test]
    fn packet_sidecar_query_diagnostic_serializes_timing_fields() {
        let diagnostic = PacketSidecarQueryDiagnosticDto {
            query: "StringUtils".to_string(),
            retrieval_mode: "full".to_string(),
            sidecar_query_ms: Some(17),
            candidate_resolution_ms: Some(3),
            total_elapsed_ms: Some(20),
            sidecar_stage_count: 2,
            sidecar_stage_total_ms: Some(16),
            candidate_count: 5,
            resolved_hit_count: 4,
            unresolved_candidate_count: 1,
            diagnostic: Some("sidecar candidates did not all resolve".to_string()),
        };

        let value = serde_json::to_value(&diagnostic).expect("serialize");
        assert_eq!(value["sidecar_query_ms"], 17);
        assert_eq!(value["candidate_resolution_ms"], 3);
        assert_eq!(value["total_elapsed_ms"], 20);
        assert_eq!(value["sidecar_stage_count"], 2);
        assert_eq!(value["sidecar_stage_total_ms"], 16);

        let parsed: PacketSidecarQueryDiagnosticDto =
            serde_json::from_value(value).expect("deserialize");
        assert_eq!(parsed.total_elapsed_ms, Some(20));
        assert_eq!(parsed.sidecar_stage_total_ms, Some(16));
    }

    #[test]
    fn agent_retrieval_trace_round_trips_retrieval_shadow() {
        let trace = AgentRetrievalTraceDto {
            request_id: "r1".to_string(),
            resolved_profile: AgentRetrievalPresetDto::Architecture,
            policy_mode: AgentRetrievalPolicyModeDto::LatencyFirst,
            total_latency_ms: 10,
            sla_target_ms: None,
            sla_missed: false,
            semantic_fallback_count: 0,
            semantic_fallbacks: Vec::new(),
            annotations: Vec::new(),
            steps: Vec::new(),
            packet_sidecar_diagnostics: Vec::new(),
            retrieval_shadow: Some(RetrievalShadowDto {
                retrieval_mode: "unavailable".to_string(),
                degraded_reason: Some("sidecar_unavailable".to_string()),
                retrieval_total_ms: 0,
                total_budget_ms: None,
                cancel_reason: Some("mandatory_sidecar_unavailable".to_string()),
                cache_hit: false,
                stage_timings: Vec::new(),
                candidates: Vec::new(),
                would_rank: Vec::new(),
                error: None,
                candidate_count: 0,
                resolved_hit_count: 0,
                unresolved_candidate_count: 0,
                candidate_resolution_counts: Vec::new(),
            }),
        };
        let value = serde_json::to_value(&trace).expect("serialize");
        assert_eq!(value["retrieval_shadow"]["retrieval_mode"], "unavailable");
        let parsed: AgentRetrievalTraceDto = serde_json::from_value(value).expect("deserialize");
        assert_eq!(
            parsed
                .retrieval_shadow
                .as_ref()
                .map(|shadow| shadow.retrieval_mode.as_str()),
            Some("unavailable")
        );
    }

    #[test]
    fn framework_route_coverage_uses_product_evidence_field_with_legacy_alias() {
        let coverage = FrameworkRouteCoverageDto {
            framework: "express".to_string(),
            language: "javascript/typescript".to_string(),
            status: "partial".to_string(),
            coverage_evidence: "validated_by_indexer_regression".to_string(),
            confidence_floor: "heuristic".to_string(),
            handler_link_support: "probable_when_handler_name_resolves".to_string(),
            unsupported_patterns: vec!["router composition is partial".to_string()],
            known_gaps: vec!["mounted prefixes are not globally propagated".to_string()],
            promotable: true,
        };

        let value = serde_json::to_value(&coverage).expect("serialize");
        assert_eq!(
            value["coverage_evidence"],
            "validated_by_indexer_regression"
        );
        assert!(
            value.get("fixture_status").is_none(),
            "product JSON should use coverage_evidence, not fixture_status"
        );

        let legacy: FrameworkRouteCoverageDto = serde_json::from_str(
            r#"{
                "framework":"express",
                "language":"javascript/typescript",
                "status":"partial",
                "fixture_status":"covered_by_indexer_unit_fixture",
                "confidence_floor":"heuristic",
                "handler_link_support":"probable_when_handler_name_resolves",
                "unsupported_patterns":[],
                "known_gaps":[],
                "promotable":true
            }"#,
        )
        .expect("deserialize legacy field spelling");
        assert_eq!(legacy.coverage_evidence, "covered_by_indexer_unit_fixture");
    }

    #[test]
    fn packet_sufficiency_serializes_status_as_snake_case() {
        let partial = serde_json::to_value(PacketSufficiencyDto {
            status: PacketSufficiencyStatusDto::Partial,
            covered_claims: Vec::new(),
            open_next: vec!["codestory-cli search --query runtime".to_string()],
            avoid_opening: Vec::new(),
            avoid_opening_paths: Vec::new(),
            gaps: vec!["No focused symbol selected.".to_string()],
            follow_up_commands: Vec::new(),
            coverage_report: None,
        })
        .expect("serialize");

        assert_eq!(partial["status"], "partial");

        let blocked = serde_json::to_value(PacketSufficiencyDto {
            status: PacketSufficiencyStatusDto::Insufficient,
            covered_claims: Vec::new(),
            open_next: Vec::new(),
            avoid_opening: Vec::new(),
            avoid_opening_paths: vec!["crates/codestory-cli/src/main.rs".to_string()],
            gaps: vec!["Sidecar readiness is not full.".to_string()],
            follow_up_commands: Vec::new(),
            coverage_report: None,
        })
        .expect("serialize");

        assert_eq!(blocked["status"], "blocked");
        assert_eq!(
            blocked["avoid_opening_paths"],
            serde_json::json!(["crates/codestory-cli/src/main.rs"])
        );
        let legacy: PacketSufficiencyDto = serde_json::from_str(
            r#"{
                "status": "partial",
                "covered_claims": [],
                "open_next": [],
                "avoid_opening": ["crates/codestory-cli/src/main.rs because cited"],
                "gaps": [],
                "follow_up_commands": []
            }"#,
        )
        .expect("deserialize legacy sufficiency without raw paths");
        assert!(legacy.avoid_opening_paths.is_empty());
        let legacy: PacketSufficiencyStatusDto =
            serde_json::from_str("\"insufficient\"").expect("deserialize legacy status");
        assert_eq!(legacy, PacketSufficiencyStatusDto::Insufficient);
    }

    #[test]
    fn search_plan_bridge_contract_uses_typed_snake_case_states() {
        let value = serde_json::to_value(SearchPlanBridgeDto {
            from_anchor: "router".to_string(),
            to_anchor: "handler".to_string(),
            status: SearchPlanBridgeStatusDto::Supported,
            confidence: SearchPlanBridgeConfidenceDto::High,
            evidence_kind: SearchPlanBridgeEvidenceKindDto::GraphPath,
            direction: Some("forward".to_string()),
            node_count: 2,
            edge_count: 1,
            truncated: false,
            notes: Vec::new(),
        })
        .expect("serialize");

        assert_eq!(value["status"], "supported");
        assert_eq!(value["confidence"], "high");
        assert_eq!(value["evidence_kind"], "graph_path");
        let parsed: SearchPlanBridgeDto = serde_json::from_value(value).expect("deserialize");
        assert_eq!(parsed.status, SearchPlanBridgeStatusDto::Supported);
        assert_eq!(parsed.confidence, SearchPlanBridgeConfidenceDto::High);
        assert_eq!(
            parsed.evidence_kind,
            SearchPlanBridgeEvidenceKindDto::GraphPath
        );
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct CreateBookmarkCategoryRequest {
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct UpdateBookmarkCategoryRequest {
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct BookmarkDto {
    pub id: String,
    pub category_id: String,
    pub node_id: NodeId,
    pub comment: Option<String>,
    pub node_label: String,
    pub node_kind: NodeKind,
    pub file_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct CreateBookmarkRequest {
    pub category_id: String,
    pub node_id: NodeId,
    #[serde(default)]
    pub comment: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct UpdateBookmarkRequest {
    #[serde(default)]
    pub category_id: Option<String>,
    #[serde(default, with = "::serde_with::rust::double_option")]
    pub comment: Option<Option<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct OpenDefinitionRequest {
    pub node_id: NodeId,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct OpenContainingFolderRequest {
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct SystemActionResponse {
    pub ok: bool,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct WriteFileDataUrlRequest {
    pub path: String,
    /// A `data:*;base64,...` URL (for example from `graph.toDataURL()`).
    pub data_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct WriteFileResponse {
    // Use u32 so TS can safely represent this as `number` without BigInt.
    pub bytes_written: u32,
}
