use super::*;

pub(super) fn graph_neighborhood(
    controller: &AppController,
    req: GraphRequest,
) -> Result<GraphResponse, ApiError> {
    let center = req.center_id.to_core()?;
    let graph_flags = app_graph_flags();

    let storage = controller.open_storage()?;

    let max_edges = req.max_edges.unwrap_or(400).min(2_000) as usize;
    let mut edges = storage
        .get_edges_for_node_id(center)
        .map_err(|e| ApiError::internal(format!("Failed to load edges: {e}")))?;

    if let Ok(Some(center_node)) = storage.get_node(center)
        && !is_structural_kind(center_node.kind)
    {
        let mut owner_ids = HashSet::new();
        for edge in &edges {
            if edge.kind != codestory_core::EdgeKind::MEMBER {
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
                    codestory_core::EdgeKind::INHERITANCE | codestory_core::EdgeKind::OVERRIDE
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
        canonical_layout: Some(canonical_layout),
    })
}

pub(super) fn graph_trail(
    controller: &AppController,
    req: TrailConfigDto,
) -> Result<GraphResponse, ApiError> {
    let root_id = req.root_id.to_core()?;
    let graph_flags = app_graph_flags();
    let target_id = match req.target_id {
        Some(id) => Some(id.to_core()?),
        None => None,
    };

    let config = codestory_core::TrailConfig {
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

    let storage = controller.open_storage()?;
    let result = storage
        .get_trail(&config)
        .map_err(|e| ApiError::internal(format!("Failed to compute trail: {e}")))?;

    let codestory_core::TrailResult {
        nodes,
        edges,
        depth_map,
        truncated,
    } = result;

    let node_kind_by_id: HashMap<codestory_core::NodeId, codestory_core::NodeKind> =
        nodes.iter().map(|node| (node.id, node.kind)).collect();
    let mut visible_member_counts: HashMap<codestory_core::NodeId, u32> = HashMap::new();
    for edge in &edges {
        if edge.kind != codestory_core::EdgeKind::MEMBER {
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

    Ok(GraphResponse {
        center_id,
        nodes: node_dtos,
        edges: edge_dtos,
        truncated,
        canonical_layout: Some(canonical_layout),
    })
}
