use codestory_contracts::api::{
    AgentAnswerDto, AgentAskRequest, AgentHybridWeightsDto, ApiError, GroundingBudgetDto,
    GroundingSnapshotDto, IndexMode, IndexingPhaseTimings, NodeDetailsDto, NodeDetailsRequest,
    NodeId, OpenProjectRequest, ProjectSummary, SearchHit, SearchRequest, SnippetContextDto,
    StartIndexingRequest, SymbolContextDto, TrailConfigDto, TrailContextDto,
};

use crate::AppController;

#[derive(Clone)]
pub struct ProjectService {
    controller: AppController,
}

impl ProjectService {
    pub(crate) fn new(controller: AppController) -> Self {
        Self { controller }
    }

    pub fn open_project(&self, req: OpenProjectRequest) -> Result<ProjectSummary, ApiError> {
        self.controller.open_project(req)
    }

    pub fn open_project_with_storage_path(
        &self,
        root: std::path::PathBuf,
        storage_path: std::path::PathBuf,
    ) -> Result<ProjectSummary, ApiError> {
        self.controller
            .open_project_with_storage_path(root, storage_path)
    }

    pub fn open_project_summary_with_storage_path(
        &self,
        root: std::path::PathBuf,
        storage_path: std::path::PathBuf,
    ) -> Result<ProjectSummary, ApiError> {
        self.controller
            .open_project_summary_with_storage_path(root, storage_path)
    }

    pub fn start_indexing(&self, req: StartIndexingRequest) -> Result<(), ApiError> {
        self.controller.start_indexing(req)
    }

    pub fn run_indexing_blocking(&self, mode: IndexMode) -> Result<IndexingPhaseTimings, ApiError> {
        self.controller.run_indexing_blocking(mode)
    }

    pub fn run_indexing_blocking_without_runtime_refresh(
        &self,
        mode: IndexMode,
    ) -> Result<IndexingPhaseTimings, ApiError> {
        self.controller
            .run_indexing_blocking_without_runtime_refresh(mode)
    }
}

#[derive(Clone)]
pub struct IndexService {
    controller: AppController,
}

impl IndexService {
    pub(crate) fn new(controller: AppController) -> Self {
        Self { controller }
    }

    pub fn start_indexing(&self, req: StartIndexingRequest) -> Result<(), ApiError> {
        self.controller.start_indexing(req)
    }

    pub fn run_indexing_blocking(&self, mode: IndexMode) -> Result<IndexingPhaseTimings, ApiError> {
        self.controller.run_indexing_blocking(mode)
    }

    pub fn run_indexing_blocking_without_runtime_refresh(
        &self,
        mode: IndexMode,
    ) -> Result<IndexingPhaseTimings, ApiError> {
        self.controller
            .run_indexing_blocking_without_runtime_refresh(mode)
    }
}

#[derive(Clone)]
pub struct SearchService {
    controller: AppController,
}

impl SearchService {
    pub(crate) fn new(controller: AppController) -> Self {
        Self { controller }
    }

    pub fn search(&self, req: SearchRequest) -> Result<Vec<SearchHit>, ApiError> {
        self.controller.search(req)
    }

    pub fn search_hybrid(
        &self,
        req: SearchRequest,
        focus_node_id: Option<NodeId>,
        max_results: Option<u32>,
        hybrid_weights: Option<AgentHybridWeightsDto>,
    ) -> Result<Vec<SearchHit>, ApiError> {
        self.controller
            .search_hybrid(req, focus_node_id, max_results, hybrid_weights)
    }
}

#[derive(Clone)]
pub struct GroundingService {
    controller: AppController,
}

impl GroundingService {
    pub(crate) fn new(controller: AppController) -> Self {
        Self { controller }
    }

    pub fn grounding_snapshot(
        &self,
        budget: GroundingBudgetDto,
    ) -> Result<GroundingSnapshotDto, ApiError> {
        self.controller.grounding_snapshot(budget)
    }

    pub fn symbol_context(&self, node_id: NodeId) -> Result<SymbolContextDto, ApiError> {
        self.controller.symbol_context(node_id)
    }

    pub fn trail_context(&self, req: TrailConfigDto) -> Result<TrailContextDto, ApiError> {
        self.controller.trail_context(req)
    }

    pub fn snippet_context(
        &self,
        node_id: NodeId,
        context: usize,
    ) -> Result<SnippetContextDto, ApiError> {
        self.controller.snippet_context(node_id, context)
    }

    pub fn node_details(&self, req: NodeDetailsRequest) -> Result<NodeDetailsDto, ApiError> {
        self.controller.node_details(req)
    }
}

#[derive(Clone)]
pub struct TrailService {
    controller: AppController,
}

impl TrailService {
    pub(crate) fn new(controller: AppController) -> Self {
        Self { controller }
    }

    pub fn trail_context(&self, req: TrailConfigDto) -> Result<TrailContextDto, ApiError> {
        self.controller.trail_context(req)
    }
}

#[derive(Clone)]
pub struct AgentService {
    controller: AppController,
}

impl AgentService {
    pub(crate) fn new(controller: AppController) -> Self {
        Self { controller }
    }

    pub fn ask(&self, req: AgentAskRequest) -> Result<AgentAnswerDto, ApiError> {
        self.controller.agent_ask(req)
    }
}
