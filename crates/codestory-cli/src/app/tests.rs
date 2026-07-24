use super::*;
use crate::display::{clean_path_string, relative_path};
use crate::runtime::{cache_root_for_project, fnv1a_hex};
use crate::sidecar_runtime;
use codestory_contracts::api::{
    AgentAnswerDto, AgentCitationDto, AgentRetrievalPolicyModeDto, AgentRetrievalPresetDto,
    AgentRetrievalTraceDto, CorePromotionTimings, DatabaseSnapshotCopyTimings, EdgeId, EdgeKind,
    FullRefreshWallTimings, GraphEdgeDto, GraphNodeDto, GraphResponse, IndexDryRunDto, IndexMode,
    IndexedFileDto, IndexedFileIncompleteReasonCountDto, IndexedFileRoleDto, IndexedFilesDto,
    IndexedFilesSummaryDto, IndexingPhaseTimings, NodeDetailsDto, NodeId, PacketBudgetDto,
    PacketBudgetLimitsDto, PacketBudgetUsageDto, PacketClaimDto, PacketPlanDto, PacketPlanQueryDto,
    PacketRetrievalTraceSummaryDto, PacketSufficiencyDto, ProjectSummary,
    ProjectionPersistenceFamilyTimings, ProjectionPersistenceTimings, RetrievalModeDto,
    RetrievalStateDto, SearchHit, SearchHitOrigin, SemanticModeDto, SourcePolicyExclusionDto,
    StorageStatsDto, TrailContextDto,
};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::tempdir;

mod contracts;
mod drill;
mod lifecycle;
mod rendering;
mod test_support;
