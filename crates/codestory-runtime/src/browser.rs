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

// This slot holds only the current measured floor refusal, never a progress
// preview. The caller clears it before each projection, including failed ones.
fn measured_packet_floor_refusal(
    packet: &AgentPacketDto,
    baseline: Option<&crate::evidence_projection_v3::PacketRepresentationBaselineV1>,
    product: &crate::PacketEvidenceProductV3,
) -> Option<crate::RepresentedPacketProductV3> {
    let binding = baseline?;
    if packet.packet_id != binding.packet.packet_id
        || packet.support != binding.packet.support
        || packet.disposition != binding.packet.disposition
        || !binding.added_source_units.is_empty()
        || !binding.resolved_gap_witnesses.is_empty()
        || product.source_issuance_seal.is_some()
    {
        return None;
    }
    let codestory_contracts::packet_projection_v3::PacketProjectionV3Dto::BudgetExceeded {
        maximum_bytes,
        required_complete_bytes,
        ..
    } = &product.projection
    else {
        return None;
    };
    if *maximum_bytes != codestory_contracts::compilation::PUBLIC_PACKET_SERIALIZED_MAX_BYTES as u64
        || required_complete_bytes <= maximum_bytes
    {
        return None;
    }
    Some(crate::RepresentedPacketProductV3 {
        packet: packet.clone(),
        product: product.clone(),
    })
}

fn take_measured_packet_floor_refusal(
    error: ApiError,
    measured: Option<crate::RepresentedPacketProductV3>,
    cancelled: bool,
) -> Result<crate::RepresentedPacketProductV3, ApiError> {
    if error.code != crate::agent::packet_compiler::REQUESTED_PAGE_CAPACITY_REFUSAL {
        return Err(error);
    }
    if cancelled {
        return Err(ApiError::new(
            "cancelled",
            "request cancelled before packet capacity refusal",
        ));
    }
    // Exhaustion alone does not establish a measured public capacity refusal.
    measured.ok_or_else(|| ApiError::internal(error.message))
}

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

/// A bounded physical-line read owned by runtime. The transport supplies only
/// a representation measurement; runtime owns source cuts and truthful receipts.
pub struct PreparedSourceRanges {
    windows: Vec<(String, u32, u32, crate::snippets::PathSourceWindow)>,
    source_budget: usize,
}

impl PreparedSourceRanges {
    fn render(&self, budget: usize) -> codestory_contracts::api::SourceRangesDto {
        let ranges = crate::snippets::render_path_source_windows(self.windows.clone(), budget);
        codestory_contracts::api::SourceRangesDto {
            requested_count: self.windows.len() as u32,
            omitted_count: ranges.iter().filter(|range| !range.complete).count() as u32,
            complete: ranges.iter().all(|range| range.complete),
            ranges,
            max_total_bytes: budget as u32,
            max_response_bytes: SOURCE_RANGES_MAX_RESPONSE_BYTES as u32,
        }
    }

    fn structured_size(
        result: &codestory_contracts::api::SourceRangesDto,
    ) -> Result<usize, ApiError> {
        serde_json::to_vec(result)
            .map(|bytes| bytes.len())
            .map_err(|error| ApiError::internal(error.to_string()))
    }

    pub fn render_default(&self) -> Result<codestory_contracts::api::SourceRangesDto, ApiError> {
        let result = self.render(self.source_budget);
        if Self::structured_size(&result)? > SOURCE_RANGES_MAX_RESPONSE_BYTES {
            return Err(ApiError::new(
                "source_response_too_large",
                "Range metadata and escaped source exceed the response bound; narrow the request",
            ));
        }
        Ok(result)
    }

    /// Keep all receipts and whole lines, using only the prepared read. Search
    /// bounded source budgets and retain a measured fitting page. Receipt metadata
    /// can change non-monotonically, so this is not a maximum-packing claim.
    /// The zero-source page must fit or delivery refuses.
    pub fn render_for_representation(
        &self,
        maximum_bytes: usize,
        mut measure: impl FnMut(&codestory_contracts::api::SourceRangesDto) -> Result<usize, ApiError>,
    ) -> Result<codestory_contracts::api::SourceRangesDto, ApiError> {
        let mut fits =
            |result: &codestory_contracts::api::SourceRangesDto| -> Result<bool, ApiError> {
                Ok(
                    Self::structured_size(result)? <= SOURCE_RANGES_MAX_RESPONSE_BYTES
                        && measure(result)? <= maximum_bytes,
                )
            };
        let full = self.render(self.source_budget);
        if fits(&full)? {
            return Ok(full);
        }
        let mut selected = self.render(0);
        if !fits(&selected)? {
            return Err(ApiError::new(
                "source_response_budget_too_small",
                "Allowance cannot fit source range receipts; narrow the request or increase it",
            ));
        }
        let mut lower = 1;
        let mut upper = self.source_budget.saturating_sub(1);
        while lower <= upper {
            let budget = lower + (upper - lower) / 2;
            let result = self.render(budget);
            if fits(&result)? {
                selected = result;
                lower = budget + 1;
            } else {
                upper = budget - 1;
            }
        }
        Ok(selected)
    }
}

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
        let mut bounded_measure =
            crate::evidence_projection_v3::bounded_packet_transport_measure_v3(measure);
        let measure = &mut bounded_measure;
        let reserved = crate::PacketProjectionIdentityV3::new();
        let mut full_product = None;
        let mut accepted_product = None;
        let mut baseline_product: Option<(AgentPacketDto, crate::PacketEvidenceProductV3)> = None;
        let mut measured_floor_refusal = None;
        let mut issued_record: Option<
            Option<crate::agent::packet_served_sources::ServedSourceRecord>,
        > = None;
        let mut measure_packet = |packet: &AgentPacketDto,
                                  baseline: Option<
            &crate::evidence_projection_v3::PacketRepresentationBaselineV1,
        >| {
            // A later refused or failed preview cannot reuse an earlier capacity frame.
            measured_floor_refusal = None;
            let product = crate::project_packet_v3_with_identity(
                &self.public_operation,
                caller_id,
                &req,
                packet,
                &reserved,
                &mut *measure,
            )?;
            if issued_record.is_none() {
                issued_record = Some(crate::agent::packet_served_sources::load(
                    &self.controller,
                    caller_id,
                    &req,
                )?);
            }
            measured_floor_refusal = measured_packet_floor_refusal(packet, baseline, &product);
            if issued_record
                .as_ref()
                .and_then(|r| r.as_ref())
                .is_some_and(|record| {
                    !crate::agent::packet_served_sources::projection_preserves(
                        record,
                        &product.projection,
                    )
                })
            {
                return Ok(None);
            }
            if let Some(binding) = baseline {
                if baseline_product.is_none() {
                    let projected = if packet.support == binding.packet.support
                        && packet.disposition == binding.packet.disposition
                    {
                        product.clone()
                    } else {
                        crate::project_packet_v3_with_identity(
                            &self.public_operation,
                            caller_id,
                            &req,
                            &binding.packet,
                            &reserved,
                            &mut *measure,
                        )?
                    };
                    baseline_product = Some((binding.packet.clone(), projected));
                }
                let (base_packet, base_product) = baseline_product.as_ref().unwrap();
                if base_packet.packet_id != binding.packet.packet_id
                    || base_packet.support != binding.packet.support
                    || base_packet.disposition != binding.packet.disposition
                {
                    return Err(ApiError::internal(
                        "Same-request baseline changed during joint selection.",
                    ));
                }
                if !crate::evidence_projection_v3::packet_projection_preserves_baseline_v3(
                    &base_product.projection,
                    &product.projection,
                    &binding.resolved_gap_witnesses,
                    &binding.added_source_units,
                ) {
                    return Ok(None);
                }
            }
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
            self.controller.agent_packet_for_representation(
                req.clone(),
                caller_id,
                &mut measure_packet,
            )
        }) {
            Some(Ok(packet)) => packet,
            Some(Err(error)) => {
                let mut refused = take_measured_packet_floor_refusal(
                    error,
                    measured_floor_refusal,
                    inherited_cancellation().load(std::sync::atomic::Ordering::Acquire),
                )?;
                // The outer public operation still owns source/publication revalidation.
                // Refusal cannot escape a changed or missing pinned packet owner.
                self.public_operation
                    .active_packet_representation_context_v3()
                    .ok_or_else(|| {
                        ApiError::new(
                            "publication_changed",
                            "packet owner changed before capacity refusal",
                        )
                    })?;
                if let Some(Some(record)) = issued_record {
                    refused.product.protected_source_witnesses = record.witnesses;
                }
                return Ok(refused);
            }
            None => {
                return Err(ApiError::internal(
                    "Representation selection requires the current pinned packet owner.",
                ));
            }
        };
        let mut product = accepted_product
            .or(full_product)
            .ok_or_else(|| ApiError::internal("No validated packet proposal was projected."))?;
        if let Some(Some(record)) = issued_record {
            product.protected_source_witnesses = record.witnesses;
        }
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
        let operation = "source_snippet";
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
            self.build_source_ranges(requests, max_total_bytes)?
                .render_default()
        })
    }

    /// Retain one validated source read for revision-specific delivery sizing.
    /// Rendering never reads the workspace again or changes readiness.
    pub fn prepare_source_ranges(
        &self,
        requests: &[SourceRangeRequest],
        max_total_bytes: usize,
    ) -> Result<PreparedSourceRanges, ApiError> {
        self.run_public("files", || {
            self.build_source_ranges(requests, max_total_bytes)
        })
    }

    fn build_source_ranges(
        &self,
        requests: &[SourceRangeRequest],
        max_total_bytes: usize,
    ) -> Result<PreparedSourceRanges, ApiError> {
        if requests.is_empty() || requests.len() > SOURCE_RANGES_MAX_ENTRIES {
            return Err(ApiError::invalid_argument("Expected 1..=12 source ranges"));
        }
        let source_budget = max_total_bytes.min(crate::DIRECT_SNIPPET_MAX_BYTES);
        let mut grouped = std::collections::BTreeMap::new();
        for (index, request) in requests.iter().enumerate() {
            if request.start_line == 0
                || request.end_line < request.start_line
                || request.end_line > SOURCE_RANGES_MAX_LINE
            {
                return Err(ApiError::invalid_argument(
                    "Source range must satisfy 1 <= start <= end <= 1000000",
                ));
            }
            let path = self
                .controller
                .resolve_project_file_path(&request.path, false)?;
            grouped.entry(path).or_insert_with(Vec::new).push(index);
        }
        let mut prepared = (0..requests.len()).map(|_| None).collect::<Vec<_>>();
        for (path, indices) in grouped {
            let ranges = indices
                .iter()
                .map(|index| (requests[*index].start_line, requests[*index].end_line))
                .collect::<Vec<_>>();
            let path = path
                .to_str()
                .ok_or_else(|| ApiError::invalid_argument("Source path is not UTF-8"))?;
            let (relative, windows) =
                self.controller
                    .path_source_windows(path, &ranges, source_budget)?;
            for (index, window) in indices.into_iter().zip(windows) {
                prepared[index] = Some((relative.clone(), window));
            }
        }
        let windows = requests
            .iter()
            .zip(prepared)
            .map(|(request, prepared)| {
                let (path, window) =
                    prepared.ok_or_else(|| ApiError::internal("Missing source window"))?;
                Ok((path, request.start_line, request.end_line, window))
            })
            .collect::<Result<Vec<_>, ApiError>>()?;
        Ok(PreparedSourceRanges {
            windows,
            source_budget,
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

    pub fn list_root_symbols_page(
        &self,
        offset: usize,
        limit: usize,
    ) -> Result<(Vec<SymbolSummaryDto>, usize, String), ApiError> {
        self.run_public("graph", || {
            self.controller.list_root_symbols_page(offset, limit)
        })
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

#[cfg(test)]
mod source_response_budget_tests {
    use super::*;
    fn prepared(source: &str, count: usize) -> PreparedSourceRanges {
        let windows = crate::snippets::read_path_source_windows(
            std::io::Cursor::new(source.as_bytes()),
            &vec![(1, 100); count],
            65536,
        )
        .unwrap();
        PreparedSourceRanges {
            windows: windows
                .into_iter()
                .map(|w| ("source.rs".into(), 1, 100, w))
                .collect(),
            source_budget: 65536,
        }
    }
    #[test]
    fn source_response_budget_reads_the_documented_batch_allowance() {
        use sha2::Digest;
        let root = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let source = (1..=160)
            .map(|line| format!("entry {line}: {}\r\n", "é🦀 \\\"".repeat(12)))
            .collect::<String>();
        assert!(source.len() > 8192); // Exceeds the observed old halving cut.
        std::fs::write(root.path().join("source.txt"), &source).unwrap();
        std::fs::write(root.path().join("second.txt"), "last\n").unwrap();
        let controller = AppController::new_with_owned_cache_root(cache.path());
        controller.state.lock().project_root = Some(root.path().to_path_buf());
        let operation = PublicOperationService::new(controller.clone());
        let browser = ReadOnlyBrowserService::new(controller, operation);
        let requests = [
            SourceRangeRequest {
                path: "source.txt".into(),
                start_line: 1,
                end_line: 160,
            },
            SourceRangeRequest {
                path: "second.txt".into(),
                start_line: 1,
                end_line: 1,
            },
        ];
        let page = browser.build_source_ranges(&requests, usize::MAX).unwrap();
        assert_eq!(page.source_budget, crate::DIRECT_SNIPPET_MAX_BYTES);
        let complete = page.render_default().unwrap();
        assert!(complete.complete);
        assert_eq!(complete.ranges.len(), 2);
        assert_eq!(complete.ranges[0].delivered_end_line, Some(160));
        assert_eq!(
            complete.ranges[0].source_range_sha256,
            Some(format!("{:x}", sha2::Sha256::digest(source.as_bytes())))
        );
        assert_eq!(complete.ranges[1].delivered_end_line, Some(1));
        // The smaller explicit source allowance still cuts whole physical lines.
        let small = browser
            .build_source_ranges(&requests, 4096)
            .unwrap()
            .render_default()
            .unwrap();
        assert!(!small.complete);
        assert!(small.ranges.iter().map(|r| r.snippet.len()).sum::<usize>() <= 4096);
        assert!(small.ranges[0].next_start_line.is_some());
        // Sizing a prepared page never reopens changed workspace files.
        std::fs::write(root.path().join("source.txt"), "replacement\n").unwrap();
        let mirrored_size = |dto: &codestory_contracts::api::SourceRangesDto| {
            serde_json::to_vec(&serde_json::json!({
                "content": [{"type": "text", "text": serde_json::to_string(dto).unwrap()}],
                "structuredContent": dto,
            }))
            .map(|bytes| bytes.len())
            .map_err(|e| ApiError::internal(e.to_string()))
        };
        let allowance = mirrored_size(&complete).unwrap();
        let delivered = page
            .render_for_representation(allowance, mirrored_size)
            .unwrap();
        assert_eq!(
            serde_json::to_value(delivered).unwrap(),
            serde_json::to_value(complete).unwrap()
        );
    }

    #[test]
    fn source_response_budget_retains_a_fitting_page_between_halving_steps() {
        let source = (1..=700)
            .map(|line| format!("entry {line}: {}\n", "é🦀 \\\"".repeat(4)))
            .collect::<String>();
        let windows = crate::snippets::read_path_source_windows(
            std::io::Cursor::new(source.as_bytes()),
            &[(1, 700); 2],
            crate::DIRECT_SNIPPET_MAX_BYTES,
        )
        .unwrap();
        let page = PreparedSourceRanges {
            windows: windows
                .into_iter()
                .map(|window| ("source.txt".into(), 1, 700, window))
                .collect(),
            source_budget: crate::DIRECT_SNIPPET_MAX_BYTES,
        };
        let measure = |dto: &codestory_contracts::api::SourceRangesDto| {
            serde_json::to_vec(&serde_json::json!({
                "content": [{"type": "text", "text": serde_json::to_string(dto).unwrap()}],
                "structuredContent": dto,
            }))
            .map(|bytes| bytes.len())
            .map_err(|e| ApiError::internal(e.to_string()))
        };
        let known_fitting = page.render(24_000);
        let allowance = measure(&known_fitting).unwrap();
        assert!(measure(&page.render(32_768)).unwrap() > allowance);
        let expected_source = known_fitting
            .ranges
            .iter()
            .map(|r| r.snippet.len())
            .sum::<usize>();
        let mut calls = 0;
        let fitted = page
            .render_for_representation(allowance, |dto| {
                calls += 1;
                measure(dto)
            })
            .unwrap();
        assert!(calls <= 18, "bounded source-budget search: {calls}");
        assert!(measure(&fitted).unwrap() <= allowance);
        assert!(
            PreparedSourceRanges::structured_size(&fitted).unwrap()
                <= SOURCE_RANGES_MAX_RESPONSE_BYTES
        );
        assert!(fitted.ranges.iter().map(|r| r.snippet.len()).sum::<usize>() >= expected_source);
        assert_eq!(fitted.ranges.len(), 2);
        assert!(
            fitted
                .ranges
                .iter()
                .all(|r| r.snippet.is_empty() || r.snippet.ends_with("```"))
        );
        for range in fitted.ranges.iter().filter(|r| !r.complete) {
            assert_eq!(
                range.next_start_line,
                Some(range.delivered_end_line.unwrap_or(0) + 1)
            );
        }
    }

    #[test]
    fn source_response_budget_propagates_measurement_failure() {
        let page = prepared("line\n", 1);
        let error = page
            .render_for_representation(4096, |_| {
                Err(ApiError::new(
                    "measurement_failed",
                    "cannot measure caller result",
                ))
            })
            .unwrap_err();
        assert_eq!(error.code, "measurement_failed");
    }

    #[test]
    fn source_response_budget_defaults_exact_tiny_and_receipts() {
        let source = "alpha\r\nβ🦀 \\\"\nlast";
        let page = prepared(source, 2);
        let original = page.render_default().unwrap();
        let measure = |dto: &codestory_contracts::api::SourceRangesDto| {
            PreparedSourceRanges::structured_size(dto)
        };
        let exact = measure(&original).unwrap();
        let full = page.render_for_representation(exact, measure).unwrap();
        assert_eq!(
            serde_json::to_value(full).unwrap(),
            serde_json::to_value(original).unwrap()
        );
        let cut = page.render_for_representation(exact - 1, measure).unwrap();
        assert!(measure(&cut).unwrap() < exact);
        assert_eq!(cut.ranges.len(), 2);
        // A smaller max_total_bytes numeral can save bytes without dropping
        // source. Exact-minus-one need not imply an incomplete source window.
        let partial_limit = measure(&page.render(0)).unwrap() + 1;
        let partial = page
            .render_for_representation(partial_limit, measure)
            .unwrap();
        assert!(measure(&partial).unwrap() <= partial_limit);
        assert!(!partial.complete);
        for range in cut.ranges.iter().filter(|r| !r.complete) {
            assert!(range.next_start_line.is_some());
            assert!(range.snippet.is_empty() || range.snippet.ends_with("```"));
        }
        assert_eq!(
            page.render_for_representation(1, measure).unwrap_err().code,
            "source_response_budget_too_small"
        );
    }
    #[test]
    fn source_response_budget_keeps_zero_source_receipts_and_structured_escape_cap() {
        let page = prepared(&format!("{}\n", "\0".repeat(30000)), 3);
        assert_eq!(
            page.render_default().unwrap_err().code,
            "source_response_too_large"
        );
        let bounded = page.render_for_representation(524288, |_| Ok(1)).unwrap();
        assert!(
            PreparedSourceRanges::structured_size(&bounded).unwrap()
                <= SOURCE_RANGES_MAX_RESPONSE_BYTES
        );
        assert_eq!(bounded.ranges.len(), 3);
        assert!(!bounded.complete);
        assert!(bounded.ranges.iter().all(|r| r.snippet.is_empty()
            && r.next_start_line == Some(1)
            && r.delivered_start_line.is_none()
            && r.source_range_sha256.is_none()));
    }
}

#[cfg(test)]
mod packet_capacity_refusal_tests {
    use super::*;
    use crate::evidence_projection_v3::{
        PacketRepresentationBaselineV1, representation_budget_test_support as fixture,
    };
    use codestory_contracts::compilation::*;
    use codestory_contracts::packet_projection_v3::PacketProjectionV3Dto;

    fn floor() -> (AgentPacketRequestDto, AgentPacketDto) {
        let request: AgentPacketRequestDto =
            serde_json::from_value(serde_json::json!({"question":"Read source"})).unwrap();
        let input = PacketCompilationInputV1 {
            contract_version: PACKET_COMPILATION_CONTRACT_VERSION_V1,
            publication: PacketCompilationPublicationV1 {
                project_id: "project-1".into(),
                core_generation_id: "core-generation-1".into(),
                retrieval_generation: Some("retrieval-generation-1".into()),
            },
            admissions: vec![PacketAdmissionReceiptV1 {
                packet_ordinal: 0,
                stable_identity: "node:1".into(),
                score_version: "test".into(),
                reserved_source_bytes: 512,
                origin: PacketAdmissionOriginV1::Retrieval,
            }],
            sources: vec![PacketHydratedSourceRangeV1 {
                callable_body: None,
                stable_identity: "node:1".into(),
                path: "source.rs".into(),
                symbol: Some("operation".into()),
                start_line: 1,
                end_line: 1,
                source: "```text\n>1 | original();\n```".into(),
                delivery: Some(PacketSourceDeliveryV1 {
                    requested_range: PacketSourceRangeV1 {
                        start_line: 1,
                        end_line: 1,
                    },
                    source_sha256: "a".repeat(64),
                    remaining_source_bytes: 0,
                    next_start_line: None,
                    blocked_line_rendered_bytes: None,
                }),
                parser_completeness: PacketParserCompletenessV1::Complete,
            }],
            relations: Vec::new(),
            ambiguities: Vec::new(),
            admission_gaps: Vec::new(),
        };
        (request.clone(), fixture::default_packet(input, &request).0)
    }

    fn baseline(packet: &AgentPacketDto) -> PacketRepresentationBaselineV1 {
        PacketRepresentationBaselineV1 {
            packet: packet.clone(),
            resolved_gap_witnesses: Default::default(),
            added_source_units: Vec::new(),
        }
    }

    fn budget_product(
        request: &AgentPacketRequestDto,
        packet: &AgentPacketDto,
    ) -> crate::PacketEvidenceProductV3 {
        let projected = fixture::project_packet(request, packet, |projection| {
            Ok(
                if matches!(projection, PacketProjectionV3Dto::Complete { .. }) {
                    16_385
                } else {
                    serde_json::to_vec(projection).unwrap().len()
                },
            )
        })
        .unwrap();
        assert!(matches!(
            projected.projection,
            PacketProjectionV3Dto::BudgetExceeded { .. }
        ));
        projected
    }

    #[test]
    fn measured_floor_budget_refusal_has_no_source_issuance() {
        let (request, packet) = floor();
        let product = budget_product(&request, &packet);
        let measured =
            measured_packet_floor_refusal(&packet, Some(&baseline(&packet)), &product).unwrap();
        let returned = take_measured_packet_floor_refusal(
            ApiError::new(
                crate::agent::packet_compiler::REQUESTED_PAGE_CAPACITY_REFUSAL,
                "capacity exhausted",
            ),
            Some(measured),
            false,
        )
        .unwrap();
        assert_eq!(
            returned.product, product,
            "return the exact measured public refusal"
        );
        assert_eq!(
            returned.packet.support, packet.support,
            "private protected floor remains intact"
        );
        assert!(returned.product.source_issuance_seal.is_none());
        let cache = tempfile::tempdir().unwrap();
        let old_record = cache.path().join("original-served-record");
        std::fs::write(&old_record, b"unchanged parent").unwrap();
        let controller = AppController::new_with_owned_cache_root(cache.path());
        crate::agent::packet_served_sources::commit(
            &controller,
            "codestory-cli",
            &returned.product,
        )
        .unwrap();
        assert_eq!(std::fs::read(&old_record).unwrap(), b"unchanged parent");
        assert_eq!(
            std::fs::read_dir(cache.path()).unwrap().count(),
            1,
            "refusal creates no history"
        );
    }

    #[test]
    fn complete_source_loss_and_changed_floor_cannot_become_capacity_refusal() {
        let (request, packet) = floor();
        let binding = baseline(&packet);
        let complete = fixture::project_packet(&request, &packet, |_| Ok(0)).unwrap();
        assert!(measured_packet_floor_refusal(&packet, Some(&binding), &complete).is_none());
        let product = budget_product(&request, &packet);
        assert!(measured_packet_floor_refusal(&packet, None, &product).is_none());
        let mut changed = packet.clone();
        changed.support[0].snippet = Some("replaced source".into());
        assert!(measured_packet_floor_refusal(&changed, Some(&binding), &product).is_none());
        changed.support.clear();
        assert!(measured_packet_floor_refusal(&changed, Some(&binding), &product).is_none());
        let mut added = baseline(&packet);
        added.added_source_units.push(packet.support[0].clone());
        assert!(measured_packet_floor_refusal(&packet, Some(&added), &product).is_none());
    }

    #[test]
    fn capacity_refusal_never_masks_other_errors_or_reuses_a_missing_preview() {
        let (request, packet) = floor();
        let product = budget_product(&request, &packet);
        for code in [
            "internal",
            "cancelled",
            "publication_changed",
            "invalid_argument",
        ] {
            let error = ApiError::new(code, "original failure");
            let measured =
                measured_packet_floor_refusal(&packet, Some(&baseline(&packet)), &product);
            assert_eq!(
                take_measured_packet_floor_refusal(error.clone(), measured, false).unwrap_err(),
                error
            );
        }
        let capacity = ApiError::new(
            crate::agent::packet_compiler::REQUESTED_PAGE_CAPACITY_REFUSAL,
            "capacity exhausted",
        );
        assert_eq!(
            take_measured_packet_floor_refusal(capacity.clone(), None, false).unwrap_err(),
            ApiError::internal(capacity.message.clone())
        );
        let measured = measured_packet_floor_refusal(&packet, Some(&baseline(&packet)), &product);
        assert_eq!(
            take_measured_packet_floor_refusal(capacity, measured, true)
                .unwrap_err()
                .code,
            "cancelled"
        );
    }
}
