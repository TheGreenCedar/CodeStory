use crate::route_coverage::{
    RouteHandlerCandidate, compare_route_handler_candidates,
    route_endpoint_metadata_from_canonical, route_endpoint_metadata_from_openapi_label,
};
#[cfg(test)]
use crate::search_scoring::HybridSearchInstrumentation;
use crate::support::node_display_name;
use crate::symbol_query::compare_search_hits_with_project_root;
use crate::{AppController, Storage, agent, graph_builders, member_access_dto};
use codestory_contracts::api::{
    AgentAnswerDto, AgentAskRequest, AgentPacketDto, AgentPacketRequestDto, ApiError, EdgeKind,
    EdgeOccurrencesRequest, GraphRequest, GraphResponse, ListChildrenSymbolsRequest,
    ListRootSymbolsRequest, NodeDetailsDto, NodeDetailsRequest, NodeId, NodeKind,
    NodeOccurrencesRequest, RouteEndpointHandlerDto, RouteEndpointMetadataDto, SearchHit,
    SourceOccurrenceDto, SymbolSummaryDto, TrailConfigDto, TrailFilterOptionsDto,
};
use codestory_contracts::graph::Node as GraphNode;
use std::collections::{HashMap, HashSet};

impl AppController {
    pub(crate) fn cached_labels<I>(
        &self,
        ids: I,
    ) -> HashMap<codestory_contracts::graph::NodeId, String>
    where
        I: IntoIterator<Item = codestory_contracts::graph::NodeId>,
    {
        let s = self.state.lock();
        ids.into_iter()
            .filter_map(|id| s.node_names.get(&id).cloned().map(|name| (id, name)))
            .collect()
    }

    pub(crate) fn file_path_for_node(
        storage: &Storage,
        node: &codestory_contracts::graph::Node,
    ) -> Result<Option<String>, ApiError> {
        let Some(file_id) = node.file_node_id else {
            return Ok(None);
        };

        let file_node = storage
            .get_node(file_id)
            .map_err(|e| ApiError::internal(format!("Failed to load file node: {e}")))?;

        Ok(file_node.map(|file| file.serialized_name))
    }

    fn occurrence_kind_label(kind: codestory_contracts::graph::OccurrenceKind) -> &'static str {
        match kind {
            codestory_contracts::graph::OccurrenceKind::DEFINITION => "definition",
            codestory_contracts::graph::OccurrenceKind::REFERENCE => "reference",
            codestory_contracts::graph::OccurrenceKind::DECLARATION => "declaration",
            codestory_contracts::graph::OccurrenceKind::MACRO_DEFINITION => "macro_definition",
            codestory_contracts::graph::OccurrenceKind::MACRO_REFERENCE => "macro_reference",
            codestory_contracts::graph::OccurrenceKind::UNKNOWN => "unknown",
        }
    }

    fn to_source_occurrence_dto(
        storage: &Storage,
        occurrence: codestory_contracts::graph::Occurrence,
    ) -> Result<Option<SourceOccurrenceDto>, ApiError> {
        let file_node = storage
            .get_node(occurrence.location.file_node_id)
            .map_err(|e| {
                ApiError::internal(format!("Failed to resolve occurrence file node: {e}"))
            })?;
        let Some(file_node) = file_node else {
            return Ok(None);
        };

        Ok(Some(SourceOccurrenceDto {
            element_id: occurrence.element_id.to_string(),
            kind: Self::occurrence_kind_label(occurrence.kind).to_string(),
            file_path: file_node.serialized_name,
            start_line: occurrence.location.start_line,
            start_col: occurrence.location.start_col,
            end_line: occurrence.location.end_line,
            end_col: occurrence.location.end_col,
        }))
    }

    pub(crate) fn symbol_summary_for_node(
        storage: &Storage,
        labels_by_id: &HashMap<codestory_contracts::graph::NodeId, String>,
        node: codestory_contracts::graph::Node,
    ) -> Result<SymbolSummaryDto, ApiError> {
        let has_children = !storage
            .get_children_symbols(node.id)
            .map_err(|e| ApiError::internal(format!("Failed to load child symbols: {e}")))?
            .is_empty();

        let label = labels_by_id
            .get(&node.id)
            .cloned()
            .unwrap_or_else(|| node_display_name(&node));

        Ok(SymbolSummaryDto {
            id: NodeId::from(node.id),
            label,
            kind: NodeKind::from(node.kind),
            file_path: Self::file_path_for_node(storage, &node)?,
            has_children,
        })
    }

    pub(crate) fn dedupe_symbol_nodes(
        nodes: Vec<codestory_contracts::graph::Node>,
        labels_by_id: &HashMap<codestory_contracts::graph::NodeId, String>,
    ) -> Vec<codestory_contracts::graph::Node> {
        let mut seen = HashSet::new();
        let mut deduped = Vec::with_capacity(nodes.len());

        for node in nodes {
            let label = labels_by_id
                .get(&node.id)
                .cloned()
                .unwrap_or_else(|| node_display_name(&node));
            let key = (node.kind as i32, label, node.file_node_id);
            if seen.insert(key) {
                deduped.push(node);
            }
        }

        deduped
    }

    /// Resolve DB/index-backed symbol candidates for read commands.
    ///
    /// This intentionally bypasses mandatory sidecar product search so symbol,
    /// snippet, trail, and graph-query target resolution can work from an
    /// already-open indexed store. Product search and packet evidence must use
    /// the sidecar-primary search paths instead.
    pub fn resolve_indexed_symbol_candidates(
        &self,
        query: &str,
        max_results: usize,
    ) -> Result<Vec<SearchHit>, ApiError> {
        self.ensure_search_state()?;
        let storage = self.open_storage_read_only()?;
        let (matches, node_names) = {
            let mut s = self.state.lock();
            let engine = s.search_engine.as_mut().ok_or_else(|| {
                ApiError::invalid_argument("Search engine not initialized. Open a project first.")
            })?;
            (
                engine.search_symbol_with_scores(query),
                s.node_names.clone(),
            )
        };

        let mut hits = matches
            .into_iter()
            .map(|(id, score)| Self::build_search_hit(&storage, &node_names, id, score))
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        let project_root = self.require_project_root().ok();
        hits.sort_by(|left, right| {
            compare_search_hits_with_project_root(project_root.as_deref(), query, left, right)
        });
        hits.truncate(max_results.clamp(1, 50));
        Ok(hits)
    }

    pub fn list_root_symbols(
        &self,
        req: ListRootSymbolsRequest,
    ) -> Result<Vec<SymbolSummaryDto>, ApiError> {
        self.ensure_search_state()?;
        let storage = self.open_storage_read_only()?;

        let mut roots = storage
            .get_root_symbols()
            .map_err(|e| ApiError::internal(format!("Failed to load root symbols: {e}")))?;
        roots.sort_by_cached_key(node_display_name);

        let labels_by_id = self.cached_labels(roots.iter().map(|node| node.id));
        roots = Self::dedupe_symbol_nodes(roots, &labels_by_id);

        let limit = req.limit.unwrap_or(300).clamp(1, 2_000) as usize;
        if roots.len() > limit {
            roots.truncate(limit);
        }

        roots
            .into_iter()
            .map(|node| Self::symbol_summary_for_node(&storage, &labels_by_id, node))
            .collect()
    }

    pub fn list_children_symbols(
        &self,
        req: ListChildrenSymbolsRequest,
    ) -> Result<Vec<SymbolSummaryDto>, ApiError> {
        self.ensure_search_state()?;
        let parent_id = req.parent_id.to_core()?;
        let storage = self.open_storage_read_only()?;

        let mut children = storage
            .get_children_symbols(parent_id)
            .map_err(|e| ApiError::internal(format!("Failed to load child symbols: {e}")))?;
        children.sort_by_cached_key(node_display_name);

        let labels_by_id = self.cached_labels(children.iter().map(|node| node.id));
        children = Self::dedupe_symbol_nodes(children, &labels_by_id);
        children
            .into_iter()
            .map(|node| Self::symbol_summary_for_node(&storage, &labels_by_id, node))
            .collect()
    }

    /// Build an answer from indexed source and sidecar-primary retrieval.
    ///
    /// Degraded sidecar state is reported through retrieval diagnostics or an error rather than
    /// silently substituting legacy search as answer-quality proof.
    pub fn agent_ask(&self, req: AgentAskRequest) -> Result<AgentAnswerDto, ApiError> {
        agent::retrieval_primary::with_stable_retrieval_publication(self, "agent answer", || {
            agent::agent_ask(self, req.clone())
        })
    }

    pub fn begin_packet_retrieval(&self) {
        let _ = self;
    }

    #[cfg(test)]
    #[allow(dead_code)]
    pub(crate) fn take_hybrid_instrumentation(&self) -> Option<HybridSearchInstrumentation> {
        self.state.lock().last_hybrid_instrumentation.take()
    }

    /// Build an evidence packet with sufficiency, diagnostics, and budget metadata.
    ///
    /// Packet sufficiency is a runtime judgment over resolved evidence. Full-mode sidecar
    /// candidates that fail symbol resolution remain diagnostics and do not become supported
    /// claims merely because retrieval returned them.
    pub fn agent_packet(&self, req: AgentPacketRequestDto) -> Result<AgentPacketDto, ApiError> {
        agent::retrieval_primary::with_stable_retrieval_publication(self, "packet output", || {
            agent::agent_packet(self, req.clone())
        })
    }

    pub fn graph_neighborhood(&self, req: GraphRequest) -> Result<GraphResponse, ApiError> {
        graph_builders::graph_neighborhood(self, req)
    }

    pub fn graph_trail(&self, req: TrailConfigDto) -> Result<GraphResponse, ApiError> {
        graph_builders::graph_trail(self, req)
    }

    pub fn graph_direct_references(&self, req: TrailConfigDto) -> Result<GraphResponse, ApiError> {
        graph_builders::graph_direct_references(self, req)
    }

    pub fn graph_trail_filter_options(&self) -> Result<TrailFilterOptionsDto, ApiError> {
        let storage = self.open_storage_read_only()?;
        let node_kinds = storage
            .get_present_node_kinds()
            .map_err(|e| ApiError::internal(format!("Failed to load node kinds: {e}")))?
            .into_iter()
            .map(NodeKind::from)
            .collect::<Vec<_>>();
        let edge_kinds = storage
            .get_present_edge_kinds()
            .map_err(|e| ApiError::internal(format!("Failed to load edge kinds: {e}")))?
            .into_iter()
            .map(EdgeKind::from)
            .collect::<Vec<_>>();
        Ok(TrailFilterOptionsDto {
            node_kinds,
            edge_kinds,
        })
    }

    pub fn node_details(&self, req: NodeDetailsRequest) -> Result<NodeDetailsDto, ApiError> {
        let id = req.id.to_core()?;

        let storage = self.open_storage_read_only()?;

        let node = storage
            .get_node(id)
            .map_err(|e| ApiError::internal(format!("Failed to query node: {e}")))?
            .ok_or_else(|| ApiError::not_found(format!("Node not found: {id}")))?;

        let display_name = self
            .state
            .lock()
            .node_names
            .get(&node.id)
            .cloned()
            .unwrap_or_else(|| {
                node.qualified_name
                    .clone()
                    .unwrap_or_else(|| node.serialized_name.clone())
            });

        let file_path = match node.file_node_id {
            Some(file_id) => match storage.get_node(file_id) {
                Ok(Some(file_node)) => Some(file_node.serialized_name),
                _ => None,
            },
            None => None,
        };

        let route_endpoint =
            self.route_endpoint_metadata(&storage, &node, file_path.as_deref(), &display_name);
        let structural_unit = storage.get_structural_text_unit(node.id).map_err(|error| {
            ApiError::internal(format!(
                "Failed to query structural evidence metadata: {error}"
            ))
        })?;
        let openapi_endpoint = node
            .canonical_id
            .as_deref()
            .is_some_and(|value| value.starts_with("openapi:endpoint:"));

        Ok(NodeDetailsDto {
            id: NodeId::from(node.id),
            kind: NodeKind::from(node.kind),
            display_name,
            serialized_name: node.serialized_name,
            qualified_name: node.qualified_name,
            canonical_id: node.canonical_id,
            file_path,
            start_line: node.start_line,
            start_col: node.start_col,
            end_line: node.end_line,
            end_col: node.end_col,
            evidence_tier: structural_unit
                .as_ref()
                .map(|_| codestory_contracts::api::PacketEvidenceTierDto::StructuralText)
                .or_else(|| {
                    openapi_endpoint
                        .then_some(codestory_contracts::api::PacketEvidenceTierDto::ExactSource)
                }),
            evidence_producer: structural_unit
                .as_ref()
                .map(|unit| unit.producer.clone())
                .or_else(|| openapi_endpoint.then(|| "openapi_endpoint_schema".to_string())),
            resolution_status: (structural_unit.is_some() || openapi_endpoint)
                .then_some(codestory_contracts::api::PacketEvidenceResolutionDto::SourceRangeOnly),
            member_access: member_access_dto(storage.get_component_access(node.id).ok().flatten()),
            route_endpoint,
        })
    }

    pub(crate) fn route_endpoint_metadata(
        &self,
        storage: &Storage,
        node: &GraphNode,
        source_file: Option<&str>,
        display_name: &str,
    ) -> Option<RouteEndpointMetadataDto> {
        let canonical_id = node.canonical_id.as_deref()?;
        let mut metadata = if let Some(raw) = canonical_id.strip_prefix("route_endpoint:") {
            route_endpoint_metadata_from_canonical(raw, node, source_file).ok()?
        } else {
            let label = canonical_id.strip_prefix("openapi:endpoint:")?;
            route_endpoint_metadata_from_openapi_label(label, node, source_file)?
        };

        if metadata.handler.is_none() {
            metadata.handler = self.route_endpoint_handler(storage, node);
        }
        if metadata.handler.is_some()
            && !metadata
                .provenance
                .iter()
                .any(|entry| entry == "graph:handler_edge")
        {
            metadata.provenance.push("graph:handler_edge".to_string());
        }
        if metadata.source_file.is_none() {
            metadata.source_file = source_file.map(ToOwned::to_owned);
        }
        if metadata.line.is_none() {
            metadata.line = node.start_line;
        }
        if metadata.provenance.is_empty() {
            metadata.provenance.push(display_name.to_string());
        }
        Some(metadata)
    }

    fn route_endpoint_handler(
        &self,
        storage: &Storage,
        route_node: &GraphNode,
    ) -> Option<RouteEndpointHandlerDto> {
        let edges = storage.get_edges().ok()?;
        let mut candidates = edges
            .into_iter()
            .filter(|edge| {
                edge.kind == codestory_contracts::graph::EdgeKind::CALL
                    && edge.effective_source() == route_node.id
            })
            .filter_map(|edge| {
                let target = storage.get_node(edge.effective_target()).ok().flatten()?;
                let terminal = target
                    .qualified_name
                    .as_deref()
                    .unwrap_or(&target.serialized_name)
                    .rsplit([':', '.', '#'])
                    .next()
                    .unwrap_or(&target.serialized_name)
                    .to_ascii_lowercase();
                if matches!(
                    terminal.as_str(),
                    "get" | "post" | "put" | "patch" | "delete" | "head" | "options" | "route"
                ) {
                    return None;
                }
                Some(RouteHandlerCandidate { edge, target })
            })
            .collect::<Vec<_>>();
        candidates.sort_by(compare_route_handler_candidates);
        let RouteHandlerCandidate { edge, target } = candidates.into_iter().next()?;
        let display_name = self
            .state
            .lock()
            .node_names
            .get(&target.id)
            .cloned()
            .unwrap_or_else(|| {
                target
                    .qualified_name
                    .clone()
                    .unwrap_or_else(|| target.serialized_name.clone())
            });
        let file_path = target.file_node_id.and_then(|file_id| {
            storage
                .get_node(file_id)
                .ok()
                .flatten()
                .map(|file_node| file_node.serialized_name)
        });
        Some(RouteEndpointHandlerDto {
            node_id: NodeId::from(target.id),
            display_name,
            file_path,
            line: target.start_line,
            certainty: edge
                .certainty
                .map(|certainty| certainty.as_str().to_string()),
            confidence: edge.confidence,
        })
    }

    pub fn node_occurrences(
        &self,
        req: NodeOccurrencesRequest,
    ) -> Result<Vec<SourceOccurrenceDto>, ApiError> {
        let id = req.id.to_core()?;
        let storage = self.open_storage_read_only()?;
        let mut occurrences = storage
            .get_occurrences_for_node(id)
            .map_err(|e| ApiError::internal(format!("Failed to load node occurrences: {e}")))?
            .into_iter()
            .filter_map(|occurrence| {
                Self::to_source_occurrence_dto(&storage, occurrence).transpose()
            })
            .collect::<Result<Vec<_>, ApiError>>()?;

        occurrences.sort_by(|left, right| {
            left.file_path
                .cmp(&right.file_path)
                .then(left.start_line.cmp(&right.start_line))
                .then(left.start_col.cmp(&right.start_col))
                .then(left.end_line.cmp(&right.end_line))
                .then(left.end_col.cmp(&right.end_col))
        });
        Ok(occurrences)
    }

    pub fn edge_occurrences(
        &self,
        req: EdgeOccurrencesRequest,
    ) -> Result<Vec<SourceOccurrenceDto>, ApiError> {
        let id = req.id.to_core()?;
        let storage = self.open_storage_read_only()?;
        let mut occurrences = storage
            .get_occurrences_for_element(id.0)
            .map_err(|e| ApiError::internal(format!("Failed to load edge occurrences: {e}")))?
            .into_iter()
            .filter_map(|occurrence| {
                Self::to_source_occurrence_dto(&storage, occurrence).transpose()
            })
            .collect::<Result<Vec<_>, ApiError>>()?;

        occurrences.sort_by(|left, right| {
            left.file_path
                .cmp(&right.file_path)
                .then(left.start_line.cmp(&right.start_line))
                .then(left.start_col.cmp(&right.start_col))
                .then(left.end_line.cmp(&right.end_line))
                .then(left.end_col.cmp(&right.end_col))
        });
        Ok(occurrences)
    }
}
