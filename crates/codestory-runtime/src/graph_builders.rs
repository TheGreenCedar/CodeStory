use super::*;
use std::collections::VecDeque;

pub(super) fn graph_neighborhood(
    controller: &AppController,
    req: GraphRequest,
) -> Result<GraphResponse, ApiError> {
    let center = req.center_id.to_core()?;
    let graph_flags = app_graph_flags();

    let storage = controller.open_storage_read_only()?;

    let max_edges = req.max_edges.unwrap_or(400).min(2_000) as usize;
    let mut edges = storage
        .get_edges_for_node_id(center)
        .map_err(|e| ApiError::internal(format!("Failed to load edges: {e}")))?;

    if let Ok(Some(center_node)) = storage.get_node(center)
        && !is_structural_kind(center_node.kind)
    {
        let mut owner_ids = HashSet::new();
        for edge in &edges {
            if edge.kind != codestory_contracts::graph::EdgeKind::MEMBER {
                continue;
            }
            let (source, target) = edge.effective_endpoints();
            if target == center {
                owner_ids.insert(source);
            }
        }

        for owner_id in owner_ids {
            let owner_edges = storage
                .get_edges_for_node_id(owner_id)
                .map_err(|e| ApiError::internal(format!("Failed to load edges: {e}")))?;
            for edge in owner_edges {
                if matches!(
                    edge.kind,
                    codestory_contracts::graph::EdgeKind::INHERITANCE
                        | codestory_contracts::graph::EdgeKind::OVERRIDE
                ) {
                    edges.push(edge);
                }
            }
        }
    }

    let mut seen_edge_ids = HashSet::new();
    edges.retain(|edge| seen_edge_ids.insert(edge.id));
    edges.sort_by_key(|e| e.id.0);
    let mut truncated = false;
    if edges.len() > max_edges {
        edges.truncate(max_edges);
        truncated = true;
    }

    let mut ordered_node_ids = Vec::new();
    let mut seen = HashSet::new();
    ordered_node_ids.push(center);
    seen.insert(center);

    let mut edge_dtos = Vec::with_capacity(edges.len());
    for edge in edges {
        let edge = edge.with_effective_endpoints();
        let (source, target) = (edge.source, edge.target);

        edge_dtos.push(graph_edge_dto(edge, graph_flags));

        if seen.insert(source) {
            ordered_node_ids.push(source);
        }
        if seen.insert(target) {
            ordered_node_ids.push(target);
        }
    }

    let mut node_dtos = Vec::with_capacity(ordered_node_ids.len());
    for id in ordered_node_ids {
        let (label, kind, file_path, qualified_name, member_access) = match storage.get_node(id) {
            Ok(Some(node)) => {
                let access = storage.get_component_access(node.id).ok().flatten();
                (
                    node_display_name(&node),
                    NodeKind::from(node.kind),
                    AppController::file_path_for_node(&storage, &node)
                        .ok()
                        .flatten(),
                    node.qualified_name,
                    member_access_dto(access),
                )
            }
            _ => (id.0.to_string(), NodeKind::UNKNOWN, None, None, None),
        };

        node_dtos.push(GraphNodeDto {
            id: NodeId::from(id),
            label,
            kind,
            depth: if id == center { 0 } else { 1 },
            label_policy: Some("qualified_or_serialized".to_string()),
            badge_visible_members: None,
            badge_total_members: None,
            merged_symbol_examples: Vec::new(),
            file_path,
            qualified_name,
            member_access,
        });
    }

    let center_id = NodeId::from(center);
    let canonical_layout =
        graph_canonical::build_canonical_layout(&center_id, &node_dtos, &edge_dtos);

    Ok(GraphResponse {
        center_id,
        nodes: node_dtos,
        edges: edge_dtos,
        truncated,
        omitted_edge_count: 0,
        canonical_layout: Some(canonical_layout),
    })
}

pub(super) fn graph_trail(
    controller: &AppController,
    req: TrailConfigDto,
) -> Result<GraphResponse, ApiError> {
    let root_id = req.root_id.to_core()?;
    let graph_flags = app_graph_flags();
    let hide_speculative = req.hide_speculative;
    let target_id = match req.target_id {
        Some(id) => Some(id.to_core()?),
        None => None,
    };

    let config = codestory_contracts::graph::TrailConfig {
        root_id,
        mode: req.mode.into(),
        target_id,
        depth: req.depth,
        direction: req.direction.into(),
        caller_scope: req.caller_scope.into(),
        edge_filter: req.edge_filter.into_iter().map(Into::into).collect(),
        show_utility_calls: req.show_utility_calls,
        node_filter: req.node_filter.into_iter().map(Into::into).collect(),
        max_nodes: req.max_nodes.clamp(10, 100_000) as usize,
    };

    let storage = controller.open_storage_read_only()?;
    let result = storage
        .get_trail(&config)
        .map_err(|e| ApiError::internal(format!("Failed to compute trail: {e}")))?;

    let codestory_contracts::graph::TrailResult {
        nodes,
        edges,
        depth_map,
        truncated,
        omitted_edge_count,
    } = result;

    let node_kind_by_id: HashMap<
        codestory_contracts::graph::NodeId,
        codestory_contracts::graph::NodeKind,
    > = nodes.iter().map(|node| (node.id, node.kind)).collect();
    let mut visible_member_counts: HashMap<codestory_contracts::graph::NodeId, u32> =
        HashMap::new();
    for edge in &edges {
        if edge.kind != codestory_contracts::graph::EdgeKind::MEMBER {
            continue;
        }
        let (source, target) = edge.effective_endpoints();
        let source_kind = node_kind_by_id.get(&source).copied();
        let target_kind = node_kind_by_id.get(&target).copied();
        let source_is_structural = source_kind.is_some_and(is_structural_kind);
        let target_is_structural = target_kind.is_some_and(is_structural_kind);
        let host_id = if source_is_structural && !target_is_structural {
            Some(source)
        } else if target_is_structural && !source_is_structural {
            Some(target)
        } else {
            None
        };
        if let Some(host_id) = host_id {
            *visible_member_counts.entry(host_id).or_insert(0) += 1;
        }
    }

    let mut node_dtos = Vec::with_capacity(nodes.len());
    for node in nodes {
        let label = node_display_name(&node);
        let depth = depth_map.get(&node.id).copied().unwrap_or(0);
        let is_structural = is_structural_kind(node.kind);
        let badge_visible_members = if is_structural {
            Some(*visible_member_counts.get(&node.id).unwrap_or(&0))
        } else {
            None
        };
        let badge_total_members = if is_structural {
            storage
                .get_children_symbols(node.id)
                .ok()
                .map(|children| children.len() as u32)
        } else {
            None
        };
        let member_access = storage.get_component_access(node.id).ok().flatten();

        node_dtos.push(GraphNodeDto {
            id: NodeId::from(node.id),
            label,
            kind: NodeKind::from(node.kind),
            depth,
            label_policy: Some("qualified_or_serialized".to_string()),
            badge_visible_members,
            badge_total_members,
            merged_symbol_examples: Vec::new(),
            file_path: AppController::file_path_for_node(&storage, &node)?,
            qualified_name: node.qualified_name.clone(),
            member_access: member_access_dto(member_access),
        });
    }

    let mut edge_dtos = Vec::with_capacity(edges.len());
    for edge in edges {
        let edge = edge.with_effective_endpoints();
        edge_dtos.push(graph_edge_dto(edge, graph_flags));
    }

    let center_id = NodeId::from(config.root_id);
    let canonical_layout =
        graph_canonical::build_canonical_layout(&center_id, &node_dtos, &edge_dtos);

    let mut response = GraphResponse {
        center_id,
        nodes: node_dtos,
        edges: edge_dtos,
        truncated,
        omitted_edge_count,
        canonical_layout: Some(canonical_layout),
    };
    if hide_speculative {
        response = hide_speculative_trail_edges(response);
    }
    response = suppress_default_trail_noise(response);
    Ok(response)
}

pub(super) fn graph_direct_references(
    controller: &AppController,
    req: TrailConfigDto,
) -> Result<GraphResponse, ApiError> {
    let root_id = req.root_id.to_core()?;
    let graph_flags = app_graph_flags();
    let edge_filter = req
        .edge_filter
        .into_iter()
        .map(Into::into)
        .collect::<Vec<_>>();
    let storage = controller.open_storage_read_only()?;
    let mut edges = storage
        .get_incoming_edges_for_node_id(
            root_id,
            &edge_filter,
            req.caller_scope.into(),
            req.show_utility_calls,
        )
        .map_err(|e| ApiError::internal(format!("Failed to load incoming references: {e}")))?;
    edges.sort_by_key(|edge| edge.id.0);

    let max_nodes = req.max_nodes.clamp(10, 100_000) as usize;
    let mut selected_node_ids = Vec::with_capacity(max_nodes.min(edges.len().saturating_add(1)));
    let mut selected = HashSet::new();
    selected_node_ids.push(root_id);
    selected.insert(root_id);

    let mut retained_edges = Vec::new();
    let mut truncated = false;
    let mut omitted_edge_count = 0u32;
    for edge in edges {
        let edge = edge.with_effective_endpoints();
        let (source, target) = (edge.source, edge.target);
        if target != root_id || source == root_id {
            continue;
        }
        if !selected.contains(&source) {
            if selected_node_ids.len() >= max_nodes {
                truncated = true;
                omitted_edge_count = omitted_edge_count.saturating_add(1);
                continue;
            }
            selected.insert(source);
            selected_node_ids.push(source);
        }
        retained_edges.push(edge);
    }

    let mut node_dtos = Vec::with_capacity(selected_node_ids.len());
    for id in selected_node_ids {
        let (label, kind, file_path, qualified_name, member_access) = match storage.get_node(id) {
            Ok(Some(node)) => {
                let access = storage.get_component_access(node.id).ok().flatten();
                (
                    node_display_name(&node),
                    NodeKind::from(node.kind),
                    AppController::file_path_for_node(&storage, &node)
                        .ok()
                        .flatten(),
                    node.qualified_name,
                    member_access_dto(access),
                )
            }
            _ => (id.0.to_string(), NodeKind::UNKNOWN, None, None, None),
        };

        node_dtos.push(GraphNodeDto {
            id: NodeId::from(id),
            label,
            kind,
            depth: if id == root_id { 0 } else { 1 },
            label_policy: Some("qualified_or_serialized".to_string()),
            badge_visible_members: None,
            badge_total_members: None,
            merged_symbol_examples: Vec::new(),
            file_path,
            qualified_name,
            member_access,
        });
    }

    let edge_dtos = retained_edges
        .into_iter()
        .map(|edge| graph_edge_dto(edge, graph_flags))
        .collect::<Vec<_>>();
    let mut response = GraphResponse {
        center_id: NodeId::from(root_id),
        nodes: node_dtos,
        edges: edge_dtos,
        truncated,
        omitted_edge_count,
        canonical_layout: None,
    };
    if req.hide_speculative {
        response = hide_speculative_trail_edges(response);
    }
    Ok(suppress_default_trail_noise(response))
}

fn suppress_default_trail_noise(mut response: GraphResponse) -> GraphResponse {
    let original_edge_count = response.edges.len();
    let mut seen = HashSet::new();
    response.edges.retain(|edge| {
        if edge.source == edge.target {
            return false;
        }
        let key = (
            edge.source.clone(),
            edge.target.clone(),
            edge.kind,
            edge.callsite_identity.clone(),
        );
        seen.insert(key)
    });
    let omitted_edges = original_edge_count.saturating_sub(response.edges.len()) as u32;
    response.omitted_edge_count = response.omitted_edge_count.saturating_add(omitted_edges);

    if let Some(layout) = response.canonical_layout.as_mut() {
        let retained = response
            .edges
            .iter()
            .map(|edge| edge.id.clone())
            .collect::<HashSet<_>>();
        layout.edges.retain(|edge| {
            edge.source != edge.target
                && edge
                    .source_edge_ids
                    .iter()
                    .any(|source_edge_id| retained.contains(source_edge_id))
        });
    }

    response
}

fn hide_speculative_trail_edges(mut response: GraphResponse) -> GraphResponse {
    let original_edge_count = response.edges.len();
    let retained_edges = response
        .edges
        .into_iter()
        .filter(|edge| !is_speculative_trail_edge(edge))
        .collect::<Vec<_>>();

    let mut adjacency = HashMap::new();
    for edge in &retained_edges {
        adjacency
            .entry(edge.source.clone())
            .or_insert_with(Vec::new)
            .push(edge.target.clone());
        adjacency
            .entry(edge.target.clone())
            .or_insert_with(Vec::new)
            .push(edge.source.clone());
    }

    let mut reachable = HashSet::new();
    let mut queue = VecDeque::new();
    reachable.insert(response.center_id.clone());
    queue.push_back(response.center_id.clone());
    while let Some(node_id) = queue.pop_front() {
        if let Some(next_nodes) = adjacency.get(&node_id) {
            for next in next_nodes {
                if reachable.insert(next.clone()) {
                    queue.push_back(next.clone());
                }
            }
        }
    }

    response.nodes.retain(|node| reachable.contains(&node.id));
    response.edges = retained_edges
        .into_iter()
        .filter(|edge| reachable.contains(&edge.source) && reachable.contains(&edge.target))
        .collect();
    let omitted_edges = original_edge_count.saturating_sub(response.edges.len()) as u32;
    response.omitted_edge_count = response.omitted_edge_count.saturating_add(omitted_edges);

    if let Some(layout) = response.canonical_layout.as_mut() {
        let retained = response
            .edges
            .iter()
            .map(|edge| edge.id.clone())
            .collect::<HashSet<_>>();
        layout.nodes.retain(|node| reachable.contains(&node.id));
        layout.edges.retain(|edge| {
            edge.source_edge_ids
                .iter()
                .any(|source_edge_id| retained.contains(source_edge_id))
                && reachable.contains(&edge.source)
                && reachable.contains(&edge.target)
        });
    }

    response
}

pub(crate) fn is_speculative_trail_edge(edge: &GraphEdgeDto) -> bool {
    if is_speculative_certainty_label(edge.certainty.as_deref()) {
        return true;
    }
    is_runtime_bridge_edge(edge.kind)
        && (is_probable_certainty_label(edge.certainty.as_deref())
            || edge.confidence.is_some_and(|confidence| {
                confidence < codestory_contracts::graph::ResolutionCertainty::CERTAIN_MIN
            }))
}

fn is_speculative_certainty_label(certainty: Option<&str>) -> bool {
    matches!(
        certainty.map(|value| value.to_ascii_lowercase()).as_deref(),
        Some("uncertain" | "speculative")
    )
}

fn is_probable_certainty_label(certainty: Option<&str>) -> bool {
    certainty
        .map(|value| value.eq_ignore_ascii_case("probable"))
        .unwrap_or(false)
}

fn is_runtime_bridge_edge(kind: EdgeKind) -> bool {
    matches!(kind, EdgeKind::CALL | EdgeKind::MACRO_USAGE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use codestory_contracts::api::{
        CanonicalEdgeDto, CanonicalEdgeFamily, CanonicalLayoutDto, CanonicalRouteKind,
    };

    fn edge(id: i64, source: &str, target: &str) -> GraphEdgeDto {
        GraphEdgeDto {
            id: codestory_contracts::api::EdgeId(id.to_string()),
            source: NodeId(source.to_string()),
            target: NodeId(target.to_string()),
            kind: EdgeKind::CALL,
            confidence: Some(1.0),
            certainty: Some("certain".to_string()),
            callsite_identity: Some(format!("{source}->{target}")),
            candidate_targets: Vec::new(),
        }
    }

    fn edge_with_certainty(
        id: i64,
        source: &str,
        target: &str,
        certainty: Option<&str>,
        confidence: Option<f32>,
    ) -> GraphEdgeDto {
        let mut edge = edge(id, source, target);
        edge.certainty = certainty.map(str::to_string);
        edge.confidence = confidence;
        edge
    }

    fn canonical_edge(
        id: i64,
        source: &str,
        target: &str,
        certainty: Option<&str>,
    ) -> CanonicalEdgeDto {
        CanonicalEdgeDto {
            id: id.to_string(),
            source_edge_ids: vec![codestory_contracts::api::EdgeId(id.to_string())],
            source: NodeId(source.to_string()),
            target: NodeId(target.to_string()),
            source_handle: "source-node".to_string(),
            target_handle: "target-node".to_string(),
            kind: EdgeKind::CALL,
            certainty: certainty.map(str::to_string),
            multiplicity: 1,
            family: CanonicalEdgeFamily::Flow,
            route_kind: CanonicalRouteKind::Direct,
        }
    }

    #[test]
    fn suppress_default_trail_noise_removes_self_edges_and_duplicates() {
        let response = GraphResponse {
            center_id: NodeId("a".to_string()),
            nodes: Vec::new(),
            edges: vec![
                edge(1, "a", "a"),
                edge(2, "a", "b"),
                edge(3, "a", "b"),
                edge(4, "b", "a"),
            ],
            truncated: false,
            omitted_edge_count: 0,
            canonical_layout: None,
        };

        let filtered = suppress_default_trail_noise(response);

        assert_eq!(filtered.edges.len(), 2);
        assert_eq!(filtered.omitted_edge_count, 2);
        assert!(
            filtered.edges.iter().all(|edge| edge.source != edge.target),
            "self edges should be suppressed by default: {filtered:#?}"
        );
    }

    #[test]
    fn hide_speculative_trail_edges_removes_probable_and_low_confidence_edges() {
        let response = GraphResponse {
            center_id: NodeId("a".to_string()),
            nodes: Vec::new(),
            edges: vec![
                edge_with_certainty(1, "a", "b", Some("probable"), Some(0.70)),
                edge_with_certainty(2, "a", "c", None, Some(0.54)),
                edge_with_certainty(3, "a", "d", Some("certain"), Some(0.85)),
                {
                    let mut edge = edge_with_certainty(4, "a", "e", Some("probable"), Some(0.70));
                    edge.kind = EdgeKind::USAGE;
                    edge
                },
            ],
            truncated: false,
            omitted_edge_count: 0,
            canonical_layout: None,
        };

        let filtered = hide_speculative_trail_edges(response);

        assert_eq!(filtered.edges.len(), 2);
        assert!(
            filtered
                .edges
                .iter()
                .any(|edge| edge.target == NodeId("d".to_string()))
        );
        assert!(
            filtered
                .edges
                .iter()
                .any(|edge| edge.target == NodeId("e".to_string()))
        );
        assert_eq!(filtered.omitted_edge_count, 2);
    }

    #[test]
    fn hide_speculative_trail_edges_filters_canonical_layout_by_retained_source_edges() {
        let response = GraphResponse {
            center_id: NodeId("a".to_string()),
            nodes: Vec::new(),
            edges: vec![
                edge_with_certainty(1, "a", "b", Some("probable"), Some(0.70)),
                edge_with_certainty(2, "a", "c", Some("certain"), Some(0.85)),
                edge_with_certainty(3, "c", "b", Some("certain"), Some(0.85)),
            ],
            truncated: false,
            omitted_edge_count: 0,
            canonical_layout: Some(CanonicalLayoutDto {
                schema_version: 1,
                center_node_id: NodeId("a".to_string()),
                nodes: Vec::new(),
                edges: vec![
                    canonical_edge(1, "a", "b", Some("probable")),
                    canonical_edge(2, "a", "c", Some("certain")),
                    canonical_edge(3, "c", "b", Some("certain")),
                ],
            }),
        };

        let filtered = hide_speculative_trail_edges(response);

        let retained_edge_ids = filtered
            .edges
            .iter()
            .map(|edge| edge.id.clone())
            .collect::<HashSet<_>>();
        assert!(!retained_edge_ids.contains(&codestory_contracts::api::EdgeId("1".to_string())));
        let layout = filtered
            .canonical_layout
            .as_ref()
            .expect("canonical layout should remain available");
        assert!(
            layout.edges.iter().all(|edge| edge
                .source_edge_ids
                .iter()
                .all(|source_edge_id| retained_edge_ids.contains(source_edge_id))),
            "canonical layout leaked a suppressed edge: {layout:#?}"
        );
        assert_eq!(layout.edges.len(), 2);
        assert_eq!(filtered.omitted_edge_count, 1);
    }
}
