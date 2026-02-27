use super::*;

pub(super) fn get_trail(storage: &Storage, config: &TrailConfig) -> Result<TrailResult, StorageError> {
    match config.mode {
        TrailMode::ToTargetSymbol => get_trail_to_target(storage, config),
        _ => get_trail_bfs(storage, config),
    }
}

pub(super) fn get_trail_bfs(
    storage: &Storage,
    config: &TrailConfig,
) -> Result<TrailResult, StorageError> {
    let mut result = TrailResult::default();
    let mut visited: HashSet<NodeId> = HashSet::new();
    let mut queue: VecDeque<(NodeId, u32)> = VecDeque::new();
    let max_edges = config.max_nodes.saturating_mul(3).max(128);
    let max_depth = if config.depth == 0 { u32::MAX } else { config.depth };

    let direction = match config.mode {
        TrailMode::AllReferenced => TrailDirection::Outgoing,
        TrailMode::AllReferencing => TrailDirection::Incoming,
        _ => config.direction,
    };

    queue.push_back((config.root_id, 0));
    visited.insert(config.root_id);
    result.depth_map.insert(config.root_id, 0);

    while let Some((current_id, depth)) = queue.pop_front() {
        if result.nodes.len() >= config.max_nodes {
            result.truncated = true;
            break;
        }

        if let Some(node) = storage.get_node(current_id)? {
            result.nodes.push(node);
        }

        if depth < max_depth {
            let edges = get_edges_for_node(
                storage,
                current_id,
                &direction,
                &config.edge_filter,
                config.caller_scope,
                config.show_utility_calls,
            )?;

            for edge in edges {
                if result.edges.len() >= max_edges {
                    result.truncated = true;
                    break;
                }
                result.edges.push(edge.clone());
                let Some(neighbor_id) = super::neighbor_for_direction(current_id, direction, &edge) else {
                    continue;
                };

                if !visited.contains(&neighbor_id) {
                    visited.insert(neighbor_id);
                    result.depth_map.insert(neighbor_id, depth + 1);
                    queue.push_back((neighbor_id, depth + 1));
                }
            }

            if result.truncated {
                break;
            }
        }
    }

    super::apply_trail_node_filter(&mut result, config);
    Ok(result)
}

pub(super) fn get_trail_to_target(
    storage: &Storage,
    config: &TrailConfig,
) -> Result<TrailResult, StorageError> {
    let target_id = config.target_id.ok_or_else(|| {
        StorageError::Other("TrailMode::ToTargetSymbol requires TrailConfig.target_id".to_string())
    })?;

    let max_depth = if config.depth == 0 { u32::MAX } else { config.depth };
    let bfs_cap = config
        .max_nodes
        .saturating_mul(4)
        .max(config.max_nodes)
        .min(100_000);

    let (dist_from_root, truncated_from_root) = bfs_distances(
        storage,
        config.root_id,
        TrailDirection::Outgoing,
        &config.edge_filter,
        config.caller_scope,
        config.show_utility_calls,
        max_depth,
        bfs_cap,
    )?;
    let (dist_to_target, truncated_to_target) = bfs_distances(
        storage,
        target_id,
        TrailDirection::Incoming,
        &config.edge_filter,
        config.caller_scope,
        config.show_utility_calls,
        max_depth,
        bfs_cap,
    )?;

    if !dist_from_root.contains_key(&target_id) {
        let mut result = TrailResult::default();
        if let Some(node) = storage.get_node(config.root_id)? {
            result.nodes.push(node);
            result.depth_map.insert(config.root_id, 0);
        }
        if target_id != config.root_id
            && let Some(node) = storage.get_node(target_id)?
        {
            result.nodes.push(node);
        }
        result.truncated = truncated_from_root || truncated_to_target;
        super::apply_trail_node_filter(&mut result, config);
        return Ok(result);
    }

    let mut included: HashSet<NodeId> = HashSet::new();
    for (id, d_root) in &dist_from_root {
        if let Some(d_to) = dist_to_target.get(id)
            && (max_depth == u32::MAX || (*d_root as u64 + *d_to as u64) <= max_depth as u64)
        {
            included.insert(*id);
        }
    }
    included.insert(config.root_id);
    included.insert(target_id);

    let mut path_nodes: Vec<NodeId> = vec![config.root_id];
    let mut current = config.root_id;
    while current != target_id {
        let Some(&d_cur) = dist_from_root.get(&current) else {
            break;
        };
        let edges = get_edges_for_node(
            storage,
            current,
            &TrailDirection::Outgoing,
            &config.edge_filter,
            config.caller_scope,
            config.show_utility_calls,
        )?;
        let mut best_next: Option<NodeId> = None;
        let mut best_key: Option<(u32, i64)> = None;
        for edge in edges {
            let (src, dst) = edge.effective_endpoints();
            if src != current {
                continue;
            }
            let next = dst;
            let Some(&d_next) = dist_from_root.get(&next) else {
                continue;
            };
            if d_next != d_cur.saturating_add(1) {
                continue;
            }
            let Some(&d_to) = dist_to_target.get(&next) else {
                continue;
            };
            if max_depth != u32::MAX {
                let path_len = d_cur as u64 + 1 + d_to as u64;
                if path_len > max_depth as u64 {
                    continue;
                }
            }
            if !included.contains(&next) {
                continue;
            }
            let key = (d_to, next.0);
            if best_key.is_none_or(|k| key < k) {
                best_key = Some(key);
                best_next = Some(next);
            }
        }

        let Some(next) = best_next else {
            break;
        };
        path_nodes.push(next);
        current = next;
    }

    let mut selected: Vec<NodeId> = Vec::new();
    let mut selected_set: HashSet<NodeId> = HashSet::new();
    for id in &path_nodes {
        push_unique(&mut selected, &mut selected_set, *id);
    }
    push_unique(&mut selected, &mut selected_set, target_id);

    let mut other: Vec<NodeId> = included.iter().copied().collect();
    other.sort_by(|a, b| {
        let da = dist_from_root.get(a).copied().unwrap_or(u32::MAX);
        let db = dist_from_root.get(b).copied().unwrap_or(u32::MAX);
        let ta = dist_to_target.get(a).copied().unwrap_or(u32::MAX);
        let tb = dist_to_target.get(b).copied().unwrap_or(u32::MAX);
        (da.saturating_add(ta), da, a.0).cmp(&(db.saturating_add(tb), db, b.0))
    });
    for id in other {
        if selected.len() >= config.max_nodes {
            break;
        }
        push_unique(&mut selected, &mut selected_set, id);
    }

    let truncated = truncated_from_root
        || truncated_to_target
        || included.len() > config.max_nodes
        || selected.len() < included.len();
    let mut result = TrailResult {
        truncated,
        ..TrailResult::default()
    };

    selected.sort_by(|a, b| {
        let da = dist_from_root.get(a).copied().unwrap_or(u32::MAX);
        let db = dist_from_root.get(b).copied().unwrap_or(u32::MAX);
        (da, a.0).cmp(&(db, b.0))
    });
    for id in &selected {
        if let Some(node) = storage.get_node(*id)? {
            result.nodes.push(node);
        }
        let depth = dist_from_root.get(id).copied().unwrap_or(0);
        result.depth_map.insert(*id, depth);
    }

    let selected_set: HashSet<NodeId> = selected.iter().copied().collect();
    let mut edge_ids: HashSet<codestory_core::EdgeId> = HashSet::new();
    for id in &selected {
        let Some(&d_root) = dist_from_root.get(id) else {
            continue;
        };
        let edges = get_edges_for_node(
            storage,
            *id,
            &TrailDirection::Outgoing,
            &config.edge_filter,
            config.caller_scope,
            config.show_utility_calls,
        )?;
        for edge in edges {
            let (src, dst) = edge.effective_endpoints();
            if src != *id || !selected_set.contains(&dst) {
                continue;
            }
            let Some(&d_to) = dist_to_target.get(&dst) else {
                continue;
            };
            if max_depth != u32::MAX {
                let len = d_root as u64 + 1 + d_to as u64;
                if len > max_depth as u64 {
                    continue;
                }
            }
            if edge_ids.insert(edge.id) {
                result.edges.push(edge);
            }
        }
    }
    result.edges.sort_by_key(|e| e.id.0);

    super::apply_trail_node_filter(&mut result, config);
    Ok(result)
}

pub(super) fn bfs_distances(
    storage: &Storage,
    start: NodeId,
    direction: TrailDirection,
    edge_filter: &[EdgeKind],
    caller_scope: TrailCallerScope,
    show_utility_calls: bool,
    max_depth: u32,
    max_nodes: usize,
) -> Result<(HashMap<NodeId, u32>, bool), StorageError> {
    let mut dist: HashMap<NodeId, u32> = HashMap::new();
    let mut queue: VecDeque<(NodeId, u32)> = VecDeque::new();
    let mut truncated = false;

    dist.insert(start, 0);
    queue.push_back((start, 0));

    while let Some((current_id, depth)) = queue.pop_front() {
        if dist.len() >= max_nodes {
            truncated = true;
            break;
        }
        if depth >= max_depth {
            continue;
        }

        let edges = get_edges_for_node(
            storage,
            current_id,
            &direction,
            edge_filter,
            caller_scope,
            show_utility_calls,
        )?;
        for edge in edges {
            let Some(neighbor_id) = super::neighbor_for_direction(current_id, direction, &edge) else {
                continue;
            };
            if let std::collections::hash_map::Entry::Vacant(entry) = dist.entry(neighbor_id) {
                let next_depth = depth.saturating_add(1);
                entry.insert(next_depth);
                queue.push_back((neighbor_id, next_depth));
            }
        }
    }

    Ok((dist, truncated))
}

pub(super) fn get_edges_for_node(
    storage: &Storage,
    node_id: NodeId,
    direction: &TrailDirection,
    edge_filter: &[EdgeKind],
    caller_scope: TrailCallerScope,
    show_utility_calls: bool,
) -> Result<Vec<Edge>, StorageError> {
    let where_clause = match direction {
        TrailDirection::Outgoing => "e.source_node_id = ?1 OR e.resolved_source_node_id = ?1",
        TrailDirection::Incoming => "e.target_node_id = ?1 OR e.resolved_target_node_id = ?1",
        TrailDirection::Both => {
            "e.source_node_id = ?1 OR e.target_node_id = ?1 OR e.resolved_source_node_id = ?1 OR e.resolved_target_node_id = ?1"
        }
    };
    let query = format!("{} WHERE {where_clause} ORDER BY e.id", super::EDGE_SELECT_BASE);

    let mut stmt = storage.conn.prepare(&query)?;
    let mut edges = Vec::new();
    let mut rows = stmt.query(params![node_id.0])?;

    while let Some(row) = rows.next()? {
        let mut edge = Storage::edge_from_row(row)?;
        let target_symbol: String = row.get(12)?;
        let caller_file_path: Option<String> = row.get(13)?;

        if edge.kind == EdgeKind::CALL
            && edge.resolved_target.is_some()
            && super::should_ignore_call_resolution(&target_symbol, edge.certainty, edge.confidence)
        {
            edge.resolved_target = None;
            edge.confidence = None;
            edge.certainty = None;
        }

        if edge.kind == EdgeKind::CALL
            && !show_utility_calls
            && super::is_common_unqualified_call_name(&target_symbol)
        {
            continue;
        }

        if !super::is_caller_scope_allowed(caller_scope, caller_file_path.as_deref()) {
            continue;
        }

        if !edge_filter.is_empty() && !edge_filter.contains(&edge.kind) {
            continue;
        }

        let (eff_source, eff_target) = edge.effective_endpoints();
        let matches_node = match direction {
            TrailDirection::Outgoing => eff_source == node_id,
            TrailDirection::Incoming => eff_target == node_id,
            TrailDirection::Both => eff_source == node_id || eff_target == node_id,
        };
        if matches_node {
            edges.push(edge);
        }
    }
    Ok(edges)
}

pub(super) fn get_edges_for_node_id(storage: &Storage, node_id: NodeId) -> Result<Vec<Edge>, StorageError> {
    get_edges_for_node(
        storage,
        node_id,
        &TrailDirection::Both,
        &[],
        TrailCallerScope::IncludeTestsAndBenches,
        true,
    )
}

fn push_unique(selected: &mut Vec<NodeId>, selected_set: &mut HashSet<NodeId>, id: NodeId) {
    if selected_set.insert(id) {
        selected.push(id);
    }
}
