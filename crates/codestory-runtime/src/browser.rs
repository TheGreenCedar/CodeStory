use codestory_contracts::api::{
    AffectedAnalysisDto, AffectedAnalysisRequest, AgentAnswerDto, AgentAskRequest,
    AgentHybridWeightsDto, AgentPacketDto, AgentPacketRequestDto, ApiError, GraphResponse,
    IndexedFilesDto, IndexedFilesRequest, LayoutDirection, ListChildrenSymbolsRequest,
    ListRootSymbolsRequest, NodeDetailsDto, NodeDetailsRequest, NodeId, NodeKind,
    NodeOccurrencesRequest, SearchHit, SearchRepoTextMode, SearchRequest, SearchResultsDto,
    SnippetContextDto, SourceOccurrenceDto, SymbolContextDto, SymbolSummaryDto, TrailCallerScope,
    TrailConfigDto, TrailContextDto, TrailDirection, TrailMode,
};
use codestory_contracts::query::{
    FilterQuery, GraphQueryAst, GraphQueryOperation, SearchQuery as BrowserSearchQuery,
    SymbolQuery, TrailQuery,
};

use crate::{
    AppController, PublicOperationService, SymbolWorkflowOutcome, SymbolWorkflowRequest,
    TargetResolution, TargetSelection, active_public_operation_cancellation, compare_ranked_hits,
    symbol_name_match_rank,
};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

/// The facade runs inside whatever public operation the transport already
/// opened. Minting a fresh flag here replaced the caller's live cancellation
/// with a permanently-false one, so a host that cancelled mid-tool-body was
/// never observed by anything the body called. Inheriting keeps the host's
/// signal reachable; an unwrapped entry (tests, embedders) still gets an
/// isolated flag rather than a shared global.
fn inherited_cancellation() -> Arc<AtomicBool> {
    active_public_operation_cancellation().unwrap_or_else(|| Arc::new(AtomicBool::new(false)))
}

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

#[cfg(feature = "benchmark-support")]
#[derive(Debug, Clone)]
pub struct BenchmarkPacketExecution {
    pub packet: AgentPacketDto,
    pub retrieval_proof: serde_json::Value,
}

/// Runtime-owned read-only codebase browser boundary.
///
/// This facade intentionally exposes repository lookup, grounding, and DB-first
/// ask operations only. Socket handling, stdio loops, file writes, IDE launches,
/// folders, and other system actions stay outside this boundary.
#[derive(Clone)]
pub struct ReadOnlyBrowserService {
    controller: AppController,
    public_operation: PublicOperationService,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct BrowserResolutionRank {
    exact_display: u8,
    exact_terminal: u8,
    kind_bucket: u8,
    exact_leading: u8,
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

fn browser_resolution_rank(query: &str, hit: &SearchHit) -> BrowserResolutionRank {
    let rank = symbol_name_match_rank(query, &hit.display_name);
    BrowserResolutionRank {
        exact_display: rank.exact_display,
        exact_terminal: rank.exact_terminal,
        kind_bucket: browser_resolution_kind_bucket(hit.kind),
        exact_leading: rank.exact_leading,
    }
}

fn browser_resolution_kind_bucket(kind: NodeKind) -> u8 {
    match kind {
        NodeKind::MODULE
        | NodeKind::NAMESPACE
        | NodeKind::PACKAGE
        | NodeKind::STRUCT
        | NodeKind::CLASS
        | NodeKind::INTERFACE
        | NodeKind::ENUM
        | NodeKind::UNION
        | NodeKind::TYPEDEF => 2,
        NodeKind::FUNCTION
        | NodeKind::METHOD
        | NodeKind::MACRO
        | NodeKind::FIELD
        | NodeKind::VARIABLE
        | NodeKind::GLOBAL_VARIABLE
        | NodeKind::CONSTANT
        | NodeKind::ENUM_CONSTANT => 1,
        _ => 0,
    }
}

/// One file range an agent asked to read.
#[derive(Debug, Clone)]
pub struct SourceRangeRequest {
    pub path: String,
    pub start_line: u32,
    pub end_line: u32,
}

/// Compatibility export; the wire result is owned by contracts.
pub type SourceRangeSnippet = codestory_contracts::api::SourceRangeSnippetDto;
pub const SOURCE_RANGES_MAX_ENTRIES: usize = 12;
pub const SOURCE_RANGES_MAX_LINE: u32 = 1_000_000;
pub const SOURCE_RANGES_MAX_RESPONSE_BYTES: usize = 128 * 1024;

impl ReadOnlyBrowserService {
    pub(crate) fn new(controller: AppController, public_operation: PublicOperationService) -> Self {
        Self {
            controller,
            public_operation,
        }
    }

    fn run_public<T>(
        &self,
        operation: &str,
        build: impl FnMut() -> Result<T, ApiError>,
    ) -> Result<T, ApiError> {
        self.public_operation
            .run_with_cancel(operation, inherited_cancellation(), build)
            .map(|operation| operation.value)
    }

    fn run_observational<T>(
        &self,
        operation: &str,
        build: impl FnMut() -> Result<T, ApiError>,
    ) -> Result<T, ApiError> {
        self.public_operation
            .run_observational_with_cancel(operation, inherited_cancellation(), build)
            .map(|operation| operation.value)
    }

    pub fn ask(&self, req: AgentAskRequest) -> Result<AgentAnswerDto, ApiError> {
        self.run_public("context", || self.controller.agent_ask(req.clone()))
    }

    pub fn packet(&self, req: AgentPacketRequestDto) -> Result<AgentPacketDto, ApiError> {
        if let Some(result) = self
            .public_operation
            .with_active_packet_owner(&req, || self.controller.agent_packet(req.clone()))
        {
            return result;
        }
        let _latency_scope = crate::enter_packet_latency_scope(req.latency_budget_ms);
        self.run_public("packet", || self.controller.agent_packet(req.clone()))
    }

    /// Build and select before frozen delivery using this caller's actual renderer.
    /// The existing outer packet owner retains admission, cancellation and retry.
    pub fn packet_for_representation_v3(
        &self,
        req: AgentPacketRequestDto,
        caller_id: &str,
        measure: &mut dyn FnMut(
            &codestory_contracts::packet_projection_v3::PacketProjectionV3Dto,
        ) -> Result<usize, ()>,
    ) -> Result<crate::RepresentedPacketProductV3, ApiError> {
        self.public_operation
            .active_packet_representation_context_v3()
            .ok_or_else(|| {
                ApiError::internal("Representation requires the actual fresh packet owner.")
            })?;
        let reserved = crate::PacketProjectionIdentityV3::new();
        let mut full_product = None;
        let mut accepted_product = None;
        let mut measure_packet = |packet: &AgentPacketDto| {
            let product = crate::project_packet_v3_with_identity(
                &self.public_operation,
                caller_id,
                &req,
                packet,
                &reserved,
                &mut *measure,
            )?;
            let complete_bytes = if matches!(
                &product.projection,
                codestory_contracts::packet_projection_v3::PacketProjectionV3Dto::Complete { .. }
            ) {
                Some(
                    measure(&product.projection).map_err(|_| {
                        ApiError::internal("Packet representation measurement failed.")
                    })?,
                )
            } else {
                None
            };
            if full_product.is_none() {
                full_product = Some(product.clone());
            }
            if complete_bytes.is_some_and(|bytes| {
                bytes <= codestory_contracts::compilation::PUBLIC_PACKET_SERIALIZED_MAX_BYTES
            }) {
                accepted_product = Some(product);
            }
            Ok(complete_bytes)
        };
        let packet = match self.public_operation.with_active_packet_owner(&req, || {
            self.controller
                .agent_packet_for_representation(req.clone(), &mut measure_packet)
        }) {
            Some(result) => result?,
            None => {
                return Err(ApiError::internal(
                    "Representation selection requires the current pinned packet owner.",
                ));
            }
        };
        let product = accepted_product
            .or(full_product)
            .ok_or_else(|| ApiError::internal("No validated packet proposal was projected."))?;
        Ok(crate::RepresentedPacketProductV3 { packet, product })
    }

    /// Measurement-only packet control that executes the same compiler while
    /// recording whether the dense descriptor stage actually ran.
    #[cfg(feature = "benchmark-support")]
    #[doc(hidden)]
    pub fn packet_for_benchmark(
        &self,
        req: AgentPacketRequestDto,
        include_dense_semantic: bool,
    ) -> Result<BenchmarkPacketExecution, ApiError> {
        if let Some(result) = self.public_operation.with_active_packet_owner(&req, || {
            let execution = self
                .controller
                .agent_packet_for_benchmark(req.clone(), include_dense_semantic)?;
            Ok(BenchmarkPacketExecution {
                packet: execution.packet,
                retrieval_proof: serde_json::to_value(execution.retrieval_proof).map_err(
                    |error| {
                        ApiError::internal(format!(
                            "serialize benchmark packet retrieval proof: {error}"
                        ))
                    },
                )?,
            })
        }) {
            return result;
        }
        let _latency_scope = crate::enter_packet_latency_scope(req.latency_budget_ms);
        self.run_public("packet", || {
            let execution = self
                .controller
                .agent_packet_for_benchmark(req.clone(), include_dense_semantic)?;
            Ok(BenchmarkPacketExecution {
                packet: execution.packet,
                retrieval_proof: serde_json::to_value(execution.retrieval_proof).map_err(
                    |error| {
                        ApiError::internal(format!(
                            "serialize benchmark packet retrieval proof: {error}"
                        ))
                    },
                )?,
            })
        })
    }

    pub fn search(&self, req: SearchRequest) -> Result<Vec<SearchHit>, ApiError> {
        self.run_public(crate::search_operation_name(req.repo_text), || {
            self.controller.search(req.clone())
        })
    }

    pub fn search_results(&self, req: SearchRequest) -> Result<SearchResultsDto, ApiError> {
        self.run_public(crate::search_operation_name(req.repo_text), || {
            self.controller.search_results(req.clone())
        })
    }

    pub fn resolve_indexed_symbol_candidates(
        &self,
        query: &str,
        max_results: usize,
    ) -> Result<Vec<SearchHit>, ApiError> {
        self.run_public("resolution", || {
            self.controller
                .resolve_indexed_symbol_candidates(query, max_results)
        })
    }

    pub fn resolve_target(
        &self,
        target: TargetSelection,
        file_filter: Option<&str>,
    ) -> Result<TargetResolution, ApiError> {
        let operation = match &target {
            TargetSelection::Id(_) => "graph",
            TargetSelection::Query { .. } => "resolution",
        };
        self.run_public(operation, || {
            self.controller.resolve_target(target.clone(), file_filter)
        })
    }

    pub fn resolve_source_target(
        &self,
        target: TargetSelection,
        file_filter: Option<&str>,
    ) -> Result<TargetResolution, ApiError> {
        let operation = match &target {
            TargetSelection::Id(_) => "graph",
            TargetSelection::Query { .. } => "resolution",
        };
        self.run_public(operation, || {
            self.controller
                .resolve_source_target(target.clone(), file_filter)
        })
    }

    pub fn symbol_workflow(
        &self,
        request: SymbolWorkflowRequest,
    ) -> Result<SymbolWorkflowOutcome, ApiError> {
        let operation = match &request.target {
            TargetSelection::Id(_) => "graph",
            TargetSelection::Query { .. } => "graph_assisted",
        };
        self.run_public(operation, || {
            self.controller.symbol_workflow(request.clone())
        })
    }

    pub fn indexed_files(&self, req: IndexedFilesRequest) -> Result<IndexedFilesDto, ApiError> {
        self.run_public("graph", || self.controller.indexed_files(req.clone()))
    }

    pub fn affected_analysis(
        &self,
        req: AffectedAnalysisRequest,
    ) -> Result<AffectedAnalysisDto, ApiError> {
        self.run_observational("affected", || {
            self.controller.affected_analysis(req.clone())
        })
    }

    pub fn search_hybrid(
        &self,
        req: SearchRequest,
        focus_node_id: Option<NodeId>,
        max_results: Option<u32>,
        hybrid_weights: Option<AgentHybridWeightsDto>,
    ) -> Result<Vec<SearchHit>, ApiError> {
        self.run_public("search", || {
            self.controller.search_hybrid(
                req.clone(),
                focus_node_id.clone(),
                max_results,
                hybrid_weights.clone(),
            )
        })
    }

    pub fn symbol_context(&self, node_id: NodeId) -> Result<SymbolContextDto, ApiError> {
        self.run_public("graph", || self.controller.symbol_context(node_id.clone()))
    }

    pub fn definition_context(&self, node_id: NodeId) -> Result<SymbolContextDto, ApiError> {
        self.run_public("graph", || self.controller.symbol_context(node_id.clone()))
    }

    pub fn trail_context(&self, req: TrailConfigDto) -> Result<TrailContextDto, ApiError> {
        self.run_public("graph", || self.controller.trail_context(req.clone()))
    }

    pub fn references_context(&self, req: TrailConfigDto) -> Result<TrailContextDto, ApiError> {
        self.run_public("graph", || self.controller.trail_context(req.clone()))
    }

    pub fn direct_references_graph(&self, req: TrailConfigDto) -> Result<GraphResponse, ApiError> {
        self.run_public("graph", || {
            self.controller.graph_direct_references(req.clone())
        })
    }

    /// Exact path windows share the existing public files guard. Every request
    /// receives a delivery receipt; complete windows never contain partial lines.
    /// Repeated paths scan one file once. Source snippets and metadata/wire size
    /// have separate fixed bounds, independent of question or repository identity.
    pub fn source_ranges(
        &self,
        requests: &[SourceRangeRequest],
        max_total_bytes: usize,
    ) -> Result<codestory_contracts::api::SourceRangesDto, ApiError> {
        self.run_public("files", || {
            if requests.is_empty() || requests.len() > SOURCE_RANGES_MAX_ENTRIES {
                return Err(ApiError::invalid_argument("Expected 1..=12 source ranges"));
            }
            let source_budget = max_total_bytes.min(crate::DIRECT_SNIPPET_MAX_BYTES);
            let mut grouped = std::collections::BTreeMap::new();
            for (index, request) in requests.iter().enumerate() {
                if request.start_line == 0 || request.end_line < request.start_line
                    || request.end_line > SOURCE_RANGES_MAX_LINE {
                    return Err(ApiError::invalid_argument("Source range must satisfy 1 <= start <= end <= 1000000"));
                }
                let path = self.controller.resolve_project_file_path(&request.path, false)?;
                grouped.entry(path).or_insert_with(Vec::new).push(index);
            }
            let mut prepared = (0..requests.len()).map(|_| None).collect::<Vec<_>>();
            for (path, indices) in grouped {
                let ranges = indices.iter().map(|index| {
                    (requests[*index].start_line, requests[*index].end_line)
                }).collect::<Vec<_>>();
                let path = path.to_str().ok_or_else(|| ApiError::invalid_argument("Source path is not UTF-8"))?;
                let (relative, windows) = self.controller.path_source_windows(path, &ranges, source_budget)?;
                for (index, window) in indices.into_iter().zip(windows) {
                    prepared[index] = Some((relative.clone(), window));
                }
            }
            let windows = requests.iter().zip(prepared).map(|(request, prepared)| {
                let (path, window) = prepared.ok_or_else(|| ApiError::internal("Missing source window"))?;
                Ok((path, request.start_line, request.end_line, window))
            }).collect::<Result<Vec<_>, ApiError>>()?;
            let ranges = crate::snippets::render_path_source_windows(windows, source_budget);
            let result = codestory_contracts::api::SourceRangesDto {
                requested_count: requests.len() as u32,
                omitted_count: ranges.iter().filter(|range| !range.complete).count() as u32,
                complete: ranges.iter().all(|range| range.complete),
                ranges, max_total_bytes: source_budget as u32,
                max_response_bytes: SOURCE_RANGES_MAX_RESPONSE_BYTES as u32,
            };
            let encoded = serde_json::to_vec(&result)
                .map_err(|error| ApiError::internal(error.to_string()))?;
            if encoded.len() > SOURCE_RANGES_MAX_RESPONSE_BYTES {
                return Err(ApiError::new("source_response_too_large",
                    "Range metadata and escaped source exceed the response bound; narrow the request"));
            }
            Ok(result)
        })
    }

    pub fn snippet_context(
        &self,
        node_id: NodeId,
        context: usize,
    ) -> Result<SnippetContextDto, ApiError> {
        self.run_public("source_snippet", || {
            self.controller.snippet_context(node_id.clone(), context)
        })
    }

    /// The MCP `resources/read` surface is observational: it pins the
    /// committed publication without activating or waiting, so source drift
    /// surfaces as the read's own typed error (`source_stale`) rather than an
    /// admission refusal.
    pub fn snippet_context_observational(
        &self,
        node_id: NodeId,
        context: usize,
    ) -> Result<SnippetContextDto, ApiError> {
        self.run_observational("source_snippet", || {
            self.controller.snippet_context(node_id.clone(), context)
        })
    }

    pub fn snippet_function_body_context(
        &self,
        node_id: NodeId,
        context: usize,
    ) -> Result<SnippetContextDto, ApiError> {
        self.run_public("source_snippet", || {
            self.controller
                .snippet_function_body_context(node_id.clone(), context)
        })
    }

    pub fn node_details(&self, req: NodeDetailsRequest) -> Result<NodeDetailsDto, ApiError> {
        self.run_public("graph", || self.controller.node_details(req.clone()))
    }

    pub fn node_occurrences(
        &self,
        req: NodeOccurrencesRequest,
    ) -> Result<Vec<SourceOccurrenceDto>, ApiError> {
        self.run_public("graph", || self.controller.node_occurrences(req.clone()))
    }

    pub fn list_root_symbols(
        &self,
        req: ListRootSymbolsRequest,
    ) -> Result<Vec<SymbolSummaryDto>, ApiError> {
        self.run_public("graph", || self.controller.list_root_symbols(req.clone()))
    }

    pub fn list_children_symbols(
        &self,
        req: ListChildrenSymbolsRequest,
    ) -> Result<Vec<SymbolSummaryDto>, ApiError> {
        self.run_public("graph", || {
            self.controller.list_children_symbols(req.clone())
        })
    }

    pub fn query(&self, ast: &GraphQueryAst) -> Result<Vec<BrowserQueryItem>, ApiError> {
        self.run_public("graph_assisted", || self.query_once(ast))
    }

    fn query_once(&self, ast: &GraphQueryAst) -> Result<Vec<BrowserQueryItem>, ApiError> {
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
            expand_search_plan: false,
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
        let mut hits = self
            .controller
            .resolve_indexed_symbol_candidates(query, 50)?;
        hits.sort_by(|left, right| {
            compare_ranked_hits(
                left,
                right,
                browser_resolution_rank(query, left),
                browser_resolution_rank(query, right),
            )
            .then_with(|| left.node_id.0.cmp(&right.node_id.0))
        });
        hits.into_iter().next().ok_or_else(|| {
            ApiError::not_found(format!(
                "No symbol matched query `{query}`. Run `codestory-cli search --query \"{query}\" --limit 10` to inspect candidates."
            ))
        })
    }
}
