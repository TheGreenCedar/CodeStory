use codestory_contracts::api::{
    AgentAnswerDto, AgentAskRequest, AgentHybridWeightsDto, ApiError, LayoutDirection,
    ListChildrenSymbolsRequest, ListRootSymbolsRequest, NodeDetailsDto, NodeDetailsRequest, NodeId,
    NodeKind, NodeOccurrencesRequest, SearchHit, SearchRepoTextMode, SearchRequest,
    SearchResultsDto, SnippetContextDto, SourceOccurrenceDto, SymbolContextDto, SymbolSummaryDto,
    TrailCallerScope, TrailConfigDto, TrailContextDto, TrailDirection, TrailMode,
};
use codestory_contracts::query::{
    FilterQuery, GraphQueryAst, GraphQueryOperation, SearchQuery as BrowserSearchQuery,
    SymbolQuery, TrailQuery,
};

use crate::AppController;

#[derive(Debug, Clone)]
pub struct BrowserQueryItem {
    pub node_id: NodeId,
    pub display_name: String,
    pub kind: NodeKind,
    pub file_path: Option<String>,
    pub line: Option<u32>,
    pub depth: Option<u32>,
    pub source: String,
}

/// Runtime-owned read-only codebase browser boundary.
///
/// This facade intentionally exposes repository lookup, grounding, and DB-first
/// ask operations only. Socket handling, stdio loops, file writes, IDE launches,
/// folders, and other system actions stay outside this boundary.
#[derive(Clone)]
pub struct ReadOnlyBrowserService {
    controller: AppController,
}

fn query_item_matches_filter(item: &BrowserQueryItem, filter: &FilterQuery) -> bool {
    filter.kind.is_none_or(|kind| item.kind == kind)
        && filter
            .depth
            .is_none_or(|depth| item.depth.unwrap_or(0) <= depth)
        && filter.file.as_deref().is_none_or(|needle| {
            item.file_path
                .as_deref()
                .is_some_and(|path| path.contains(needle))
        })
}

impl ReadOnlyBrowserService {
    pub(crate) fn new(controller: AppController) -> Self {
        Self { controller }
    }

    pub fn ask(&self, mut req: AgentAskRequest) -> Result<AgentAnswerDto, ApiError> {
        req.run_local_agent = false;
        self.controller.agent_ask(req)
    }

    pub fn search(&self, req: SearchRequest) -> Result<Vec<SearchHit>, ApiError> {
        self.controller.search(req)
    }

    pub fn search_results(&self, req: SearchRequest) -> Result<SearchResultsDto, ApiError> {
        self.controller.search_results(req)
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

    pub fn symbol_context(&self, node_id: NodeId) -> Result<SymbolContextDto, ApiError> {
        self.controller.symbol_context(node_id)
    }

    pub fn definition_context(&self, node_id: NodeId) -> Result<SymbolContextDto, ApiError> {
        self.controller.symbol_context(node_id)
    }

    pub fn trail_context(&self, req: TrailConfigDto) -> Result<TrailContextDto, ApiError> {
        self.controller.trail_context(req)
    }

    pub fn references_context(&self, req: TrailConfigDto) -> Result<TrailContextDto, ApiError> {
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

    pub fn node_occurrences(
        &self,
        req: NodeOccurrencesRequest,
    ) -> Result<Vec<SourceOccurrenceDto>, ApiError> {
        self.controller.node_occurrences(req)
    }

    pub fn list_root_symbols(
        &self,
        req: ListRootSymbolsRequest,
    ) -> Result<Vec<SymbolSummaryDto>, ApiError> {
        self.controller.list_root_symbols(req)
    }

    pub fn list_children_symbols(
        &self,
        req: ListChildrenSymbolsRequest,
    ) -> Result<Vec<SymbolSummaryDto>, ApiError> {
        self.controller.list_children_symbols(req)
    }

    pub fn query(&self, ast: &GraphQueryAst) -> Result<Vec<BrowserQueryItem>, ApiError> {
        let mut items = Vec::<BrowserQueryItem>::new();
        for op in &ast.operations {
            match op {
                GraphQueryOperation::Trail(query) => {
                    items = self.query_trail_items(query)?;
                }
                GraphQueryOperation::Symbol(query) => {
                    items = self.query_symbol_items(query)?;
                }
                GraphQueryOperation::Search(query) => {
                    items = self.query_search_items(query)?;
                }
                GraphQueryOperation::Filter(filter) => {
                    items.retain(|item| query_item_matches_filter(item, filter));
                }
                GraphQueryOperation::Limit(limit) => {
                    items.truncate(limit.count as usize);
                }
            }
        }
        Ok(items)
    }

    fn query_trail_items(&self, query: &TrailQuery) -> Result<Vec<BrowserQueryItem>, ApiError> {
        let target = self.resolve_query(&query.symbol)?;
        let mut request = TrailConfigDto {
            root_id: target.node_id,
            mode: TrailMode::Neighborhood,
            target_id: None,
            depth: query.depth.unwrap_or(2),
            direction: query.direction.unwrap_or(TrailDirection::Both),
            caller_scope: TrailCallerScope::ProductionOnly,
            edge_filter: Vec::new(),
            show_utility_calls: false,
            hide_speculative: false,
            story: false,
            node_filter: Vec::new(),
            max_nodes: 120,
            layout_direction: LayoutDirection::Horizontal,
        };
        if request.depth == 0 {
            request.max_nodes = 200;
        }
        let context = self.controller.trail_context(request)?;
        Ok(context
            .trail
            .nodes
            .into_iter()
            .map(|node| BrowserQueryItem {
                node_id: node.id,
                display_name: node.label,
                kind: node.kind,
                file_path: node.file_path,
                line: None,
                depth: Some(node.depth),
                source: "trail".to_string(),
            })
            .collect())
    }

    fn query_symbol_items(&self, query: &SymbolQuery) -> Result<Vec<BrowserQueryItem>, ApiError> {
        let target = self.resolve_query(&query.query)?;
        let context = self.controller.symbol_context(target.node_id.clone())?;
        Ok(std::iter::once(BrowserQueryItem {
            node_id: context.node.id,
            display_name: context.node.display_name,
            kind: context.node.kind,
            file_path: context.node.file_path,
            line: context.node.start_line,
            depth: Some(0),
            source: "symbol".to_string(),
        })
        .chain(context.children.into_iter().map(|child| BrowserQueryItem {
            node_id: child.id,
            display_name: child.label,
            kind: child.kind,
            file_path: child.file_path,
            line: None,
            depth: Some(1),
            source: "symbol_child".to_string(),
        }))
        .collect())
    }

    fn query_search_items(
        &self,
        query: &BrowserSearchQuery,
    ) -> Result<Vec<BrowserQueryItem>, ApiError> {
        let results = self.controller.search_results(SearchRequest {
            query: query.query.clone(),
            repo_text: SearchRepoTextMode::Off,
            limit_per_source: 50,
            hybrid_weights: None,
            hybrid_limits: None,
        })?;
        Ok(results
            .indexed_symbol_hits
            .into_iter()
            .map(|hit| BrowserQueryItem {
                node_id: hit.node_id,
                display_name: hit.display_name,
                kind: hit.kind,
                file_path: hit.file_path,
                line: hit.line,
                depth: None,
                source: "search".to_string(),
            })
            .collect())
    }

    fn resolve_query(&self, query: &str) -> Result<SearchHit, ApiError> {
        let mut hits = self.controller.search_hybrid(
            SearchRequest {
                query: query.to_string(),
                repo_text: SearchRepoTextMode::Off,
                limit_per_source: 50,
                hybrid_weights: None,
                hybrid_limits: None,
            },
            None,
            Some(50),
            None,
        )?;
        hits.sort_by(|left, right| {
            right
                .score
                .total_cmp(&left.score)
                .then_with(|| left.display_name.cmp(&right.display_name))
                .then_with(|| left.node_id.0.cmp(&right.node_id.0))
        });
        hits.into_iter().next().ok_or_else(|| {
            ApiError::not_found(format!(
                "No symbol matched query `{query}`. Run `codestory-cli search --query \"{query}\" --limit 10` to inspect candidates."
            ))
        })
    }
}
