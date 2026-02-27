use codestory_api::{
    CanonicalEdgeDto, CanonicalEdgeFamily, CanonicalLayoutDto, CanonicalMemberDto,
    CanonicalMemberVisibility, CanonicalNodeDto, CanonicalNodeStyle, CanonicalRouteKind, EdgeKind,
    GraphEdgeDto, GraphNodeDto, MemberAccess, NodeId, NodeKind,
};
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

const CARD_WIDTH_MIN: f32 = 228.0;
const CARD_WIDTH_MAX: f32 = 432.0;
const CARD_CHROME_WIDTH: f32 = 112.0;
const CARD_HEIGHT_MIN: f32 = 110.0;
const CARD_HEIGHT_MAX: f32 = 560.0;
const PILL_WIDTH_MIN: f32 = 96.0;
const PILL_WIDTH_MAX: f32 = 560.0;
const PILL_CHROME_WIDTH: f32 = 72.0;
const PILL_HEIGHT: f32 = 34.0;
const APPROX_CHAR_WIDTH: f32 = 7.25;
const SCHEMA_VERSION: u32 = 1;
const MAX_MERGED_SYMBOL_IDS: usize = 6;

#[derive(Debug, Clone)]
struct NodeLike {
    id: NodeId,
    label: String,
    kind: NodeKind,
    depth: u32,
    badge_visible_members: Option<u32>,
    badge_total_members: Option<u32>,
    member_access: Option<MemberAccess>,
}

impl From<&GraphNodeDto> for NodeLike {
    fn from(value: &GraphNodeDto) -> Self {
        Self {
            id: value.id.clone(),
            label: value.label.clone(),
            kind: value.kind,
            depth: value.depth,
            badge_visible_members: value.badge_visible_members,
            badge_total_members: value.badge_total_members,
            member_access: value.member_access,
        }
    }
}

#[derive(Debug, Clone)]
struct FoldedEdge {
    id: String,
    source_edge_ids: Vec<String>,
    source: NodeId,
    target: NodeId,
    kind: EdgeKind,
    certainty: Option<String>,
    multiplicity: u32,
    source_handle: String,
    target_handle: String,
    family: CanonicalEdgeFamily,
}

#[derive(Debug, Clone)]
struct MemberExtraction {
    member_host_by_id: HashMap<NodeId, NodeId>,
    members_by_host: HashMap<NodeId, Vec<CanonicalMemberDto>>,
    synthetic_hosts: Vec<NodeLike>,
}

#[derive(Debug)]
struct FoldResult {
    folded_edges: Vec<FoldedEdge>,
    canonical_node_by_id: HashMap<NodeId, NodeId>,
    duplicate_count_by_canonical: HashMap<NodeId, u32>,
    merged_ids_by_canonical: HashMap<NodeId, Vec<NodeId>>,
}

pub fn build_canonical_layout(
    center_id: &NodeId,
    nodes: &[GraphNodeDto],
    edges: &[GraphEdgeDto],
) -> CanonicalLayoutDto {
    let base_nodes = nodes.iter().map(NodeLike::from).collect::<Vec<_>>();
    let MemberExtraction {
        member_host_by_id,
        members_by_host,
        synthetic_hosts,
    } = extract_members(&base_nodes, edges);

    let mut all_nodes = base_nodes;
    all_nodes.extend(synthetic_hosts);

    let node_by_id = all_nodes
        .iter()
        .map(|node| (node.id.clone(), node))
        .collect::<HashMap<_, _>>();
    let label_by_node = all_nodes
        .iter()
        .map(|node| (node.id.clone(), node.label.clone()))
        .collect::<HashMap<_, _>>();

    let center_host_node_id = member_host_by_id
        .get(center_id)
        .cloned()
        .unwrap_or_else(|| center_id.clone());
    let signed_depth_by_node =
        compute_signed_depth_by_node(&all_nodes, edges, &center_host_node_id);

    let FoldResult {
        folded_edges,
        canonical_node_by_id,
        duplicate_count_by_canonical,
        merged_ids_by_canonical,
    } = fold_edges(
        &all_nodes,
        edges,
        &center_host_node_id,
        &member_host_by_id,
        &signed_depth_by_node,
    );

    let mut members_by_canonical: HashMap<NodeId, Vec<CanonicalMemberDto>> = HashMap::new();
    for (node_id, canonical_id) in &canonical_node_by_id {
        let Some(members) = members_by_host.get(node_id) else {
            continue;
        };
        if members.is_empty() {
            continue;
        }

        let merged = members_by_canonical
            .entry(canonical_id.clone())
            .or_default();
        let mut seen_ids = merged
            .iter()
            .map(|member| member.id.clone())
            .collect::<HashSet<_>>();
        for member in members {
            if seen_ids.insert(member.id.clone()) {
                merged.push(member.clone());
            }
        }
    }

    let mut canonical_node_ids = canonical_node_by_id
        .values()
        .cloned()
        .collect::<HashSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    canonical_node_ids.sort_by(|left, right| left.0.cmp(&right.0));

    let center_node_id = canonical_node_by_id
        .get(&center_host_node_id)
        .cloned()
        .unwrap_or(center_host_node_id);

    let mut depth_by_canonical = HashMap::new();
    for canonical_id in &canonical_node_ids {
        let merged_ids = merged_ids_by_canonical
            .get(canonical_id)
            .cloned()
            .unwrap_or_else(|| vec![canonical_id.clone()]);
        let depths = merged_ids
            .iter()
            .map(|id| signed_depth_by_node.get(id).copied().unwrap_or(0))
            .collect::<Vec<_>>();
        let depth = if depths.is_empty() {
            0
        } else {
            let sum = depths.iter().map(|value| *value as f64).sum::<f64>();
            js_round(sum / depths.len() as f64)
        };
        depth_by_canonical.insert(canonical_id.clone(), depth);
    }

    canonical_node_ids.sort_by(|left, right| {
        let depth_diff = depth_by_canonical.get(left).copied().unwrap_or(0)
            - depth_by_canonical.get(right).copied().unwrap_or(0);
        if depth_diff != 0 {
            return depth_diff.cmp(&0);
        }
        let left_label = label_by_node
            .get(left)
            .map(String::as_str)
            .unwrap_or(&left.0);
        let right_label = label_by_node
            .get(right)
            .map(String::as_str)
            .unwrap_or(&right.0);
        let label_cmp = left_label.cmp(right_label);
        if label_cmp != Ordering::Equal {
            return label_cmp;
        }
        left.0.cmp(&right.0)
    });

    let mut row_by_depth = HashMap::<i32, u32>::new();
    let mut canonical_nodes = Vec::with_capacity(canonical_node_ids.len());
    for node_id in &canonical_node_ids {
        let Some(node) = node_by_id.get(node_id) else {
            continue;
        };

        let mut members = members_by_canonical
            .get(node_id)
            .cloned()
            .unwrap_or_default();
        members.sort_by(|left, right| {
            let label_cmp = left.label.cmp(&right.label);
            if label_cmp != Ordering::Equal {
                return label_cmp;
            }
            left.id.0.cmp(&right.id.0)
        });

        let depth = depth_by_canonical.get(node_id).copied().unwrap_or(0);
        let row = row_by_depth.get(&depth).copied().unwrap_or(0);
        row_by_depth.insert(depth, row.saturating_add(1));
        let node_style = if is_card_node_kind(node.kind) {
            CanonicalNodeStyle::Card
        } else {
            CanonicalNodeStyle::Pill
        };
        let merged_symbol_ids = merged_ids_by_canonical
            .get(node_id)
            .cloned()
            .unwrap_or_else(|| vec![node_id.clone()])
            .into_iter()
            .take(MAX_MERGED_SYMBOL_IDS)
            .collect::<Vec<_>>();
        let width = estimated_node_width(node.kind, &node.label, &members);
        let height = estimated_node_height(node.kind, &members);

        canonical_nodes.push(CanonicalNodeDto {
            id: node_id.clone(),
            kind: node.kind,
            label: node.label.clone(),
            center: *node_id == center_node_id,
            node_style,
            is_non_indexed: matches!(node.kind, NodeKind::UNKNOWN | NodeKind::BUILTIN_TYPE),
            duplicate_count: duplicate_count_by_canonical
                .get(node_id)
                .copied()
                .unwrap_or(1),
            merged_symbol_ids,
            member_count: node
                .badge_visible_members
                .unwrap_or_else(|| members.len().min(u32::MAX as usize) as u32),
            badge_visible_members: node.badge_visible_members,
            badge_total_members: node.badge_total_members,
            members,
            x_rank: depth,
            y_rank: row,
            width,
            height,
            is_virtual_bundle: false,
        });
    }

    let canonical_edges = folded_edges
        .into_iter()
        .map(|edge| CanonicalEdgeDto {
            id: edge.id,
            source_edge_ids: edge
                .source_edge_ids
                .into_iter()
                .map(codestory_api::EdgeId)
                .collect(),
            source: edge.source,
            target: edge.target,
            source_handle: edge.source_handle,
            target_handle: edge.target_handle,
            kind: edge.kind,
            certainty: edge.certainty,
            multiplicity: edge.multiplicity,
            family: edge.family,
            route_kind: if edge.family == CanonicalEdgeFamily::Hierarchy {
                CanonicalRouteKind::Hierarchy
            } else {
                CanonicalRouteKind::Direct
            },
        })
        .collect::<Vec<_>>();

    CanonicalLayoutDto {
        schema_version: SCHEMA_VERSION,
        center_node_id,
        nodes: canonical_nodes,
        edges: canonical_edges,
    }
}
fn extract_members(nodes: &[NodeLike], edges: &[GraphEdgeDto]) -> MemberExtraction {
    let node_by_id = nodes
        .iter()
        .map(|node| (node.id.clone(), node))
        .collect::<HashMap<_, _>>();
    let mut member_host_by_id = HashMap::<NodeId, NodeId>::new();
    let mut members_by_host = HashMap::<NodeId, Vec<CanonicalMemberDto>>::new();
    let mut synthetic_hosts_by_id = HashMap::<NodeId, NodeLike>::new();

    for edge in edges {
        if edge.kind != EdgeKind::MEMBER {
            continue;
        }

        let Some(source_node) = node_by_id.get(&edge.source) else {
            continue;
        };
        let Some(target_node) = node_by_id.get(&edge.target) else {
            continue;
        };

        let source_is_structural = is_structural_kind(source_node.kind);
        let target_is_structural = is_structural_kind(target_node.kind);

        let (member_id, host_id) = if source_is_structural && !target_is_structural {
            (Some(target_node.id.clone()), Some(source_node.id.clone()))
        } else if !source_is_structural && target_is_structural {
            (Some(source_node.id.clone()), Some(target_node.id.clone()))
        } else {
            (None, None)
        };

        let (Some(member_id), Some(host_id)) = (member_id, host_id) else {
            continue;
        };

        member_host_by_id.insert(member_id.clone(), host_id.clone());
        let host_members = members_by_host.entry(host_id).or_default();
        if host_members.iter().any(|member| member.id == member_id) {
            continue;
        }

        let member_node = node_by_id.get(&member_id).copied();
        let member_label = member_node
            .map(|node| node.label.clone())
            .unwrap_or_else(|| member_id.0.clone());
        let member_kind = member_node
            .map(|node| node.kind)
            .unwrap_or(NodeKind::UNKNOWN);
        host_members.push(CanonicalMemberDto {
            id: member_id,
            label: member_label.clone(),
            kind: member_kind,
            visibility: infer_member_visibility(
                member_kind,
                &member_label,
                member_node.and_then(|node| node.member_access),
            ),
        });
    }

    let mut host_ids_by_label = HashMap::<String, NodeId>::new();
    for node in nodes {
        if is_structural_kind(node.kind) {
            host_ids_by_label.insert(node.label.clone(), node.id.clone());
        }
    }

    for node in nodes {
        if is_structural_kind(node.kind) || member_host_by_id.contains_key(&node.id) {
            continue;
        }
        let Some(separator_idx) = node.label.find("::") else {
            continue;
        };
        if separator_idx == 0 {
            continue;
        }

        let host_label = node.label[..separator_idx].to_string();
        let host_id = if let Some(existing) = host_ids_by_label.get(&host_label) {
            existing.clone()
        } else {
            let id = NodeId(synthetic_host_id(&host_label));
            host_ids_by_label.insert(host_label.clone(), id.clone());
            synthetic_hosts_by_id
                .entry(id.clone())
                .or_insert_with(|| NodeLike {
                    id: id.clone(),
                    label: host_label.clone(),
                    kind: NodeKind::CLASS,
                    depth: std::cmp::max(1, node.depth.saturating_sub(1)),
                    badge_visible_members: None,
                    badge_total_members: None,
                    member_access: None,
                });
            id
        };

        member_host_by_id.insert(node.id.clone(), host_id.clone());
        let host_members = members_by_host.entry(host_id).or_default();
        if host_members.iter().any(|member| member.id == node.id) {
            continue;
        }
        host_members.push(CanonicalMemberDto {
            id: node.id.clone(),
            label: node.label.clone(),
            kind: node.kind,
            visibility: infer_member_visibility(node.kind, &node.label, node.member_access),
        });
    }

    MemberExtraction {
        member_host_by_id,
        members_by_host,
        synthetic_hosts: synthetic_hosts_by_id.into_values().collect(),
    }
}

fn compute_signed_depth_by_node(
    nodes: &[NodeLike],
    edges: &[GraphEdgeDto],
    center_host_node_id: &NodeId,
) -> HashMap<NodeId, i32> {
    let mut direction_bias_by_node = HashMap::<NodeId, i32>::new();
    for edge in edges {
        if edge.kind == EdgeKind::MEMBER {
            continue;
        }
        if edge.source == *center_host_node_id && edge.target != *center_host_node_id {
            *direction_bias_by_node
                .entry(edge.target.clone())
                .or_insert(0) += 1;
        }
        if edge.target == *center_host_node_id && edge.source != *center_host_node_id {
            *direction_bias_by_node
                .entry(edge.source.clone())
                .or_insert(0) -= 1;
        }
    }

    let mut signed_depth_by_node = HashMap::new();
    for node in nodes {
        if node.id == *center_host_node_id {
            signed_depth_by_node.insert(node.id.clone(), 0);
            continue;
        }
        let base_depth = std::cmp::max(1, node.depth as i32);
        let bias = direction_bias_by_node.get(&node.id).copied().unwrap_or(0);
        signed_depth_by_node.insert(
            node.id.clone(),
            if bias < 0 { -base_depth } else { base_depth },
        );
    }
    signed_depth_by_node
}

fn fold_edges(
    nodes: &[NodeLike],
    edges: &[GraphEdgeDto],
    center_host_node_id: &NodeId,
    member_host_by_id: &HashMap<NodeId, NodeId>,
    signed_depth_by_node: &HashMap<NodeId, i32>,
) -> FoldResult {
    let mut canonical_node_by_id = HashMap::<NodeId, NodeId>::new();
    let mut canonical_node_by_key = HashMap::<String, NodeId>::new();
    let mut duplicate_count_by_canonical = HashMap::<NodeId, u32>::new();
    let mut merged_ids_by_canonical = HashMap::<NodeId, Vec<NodeId>>::new();

    for node in nodes {
        if member_host_by_id.contains_key(&node.id) {
            continue;
        }
        let depth = signed_depth_by_node
            .get(&node.id)
            .copied()
            .unwrap_or_else(|| std::cmp::max(1, node.depth as i32));
        let is_center = &node.id == center_host_node_id;
        let key = dedupe_key_for_node(node.kind, &node.label, depth, is_center);
        let canonical_id = if let Some(ref key_value) = key {
            canonical_node_by_key
                .get(key_value)
                .cloned()
                .unwrap_or_else(|| node.id.clone())
        } else {
            node.id.clone()
        };
        if let Some(key_value) = key
            && !canonical_node_by_key.contains_key(&key_value)
        {
            canonical_node_by_key.insert(key_value, canonical_id.clone());
        }
        canonical_node_by_id.insert(node.id.clone(), canonical_id.clone());
        *duplicate_count_by_canonical
            .entry(canonical_id.clone())
            .or_insert(0) += 1;
        merged_ids_by_canonical
            .entry(canonical_id)
            .or_default()
            .push(node.id.clone());
    }

    let mut folded = HashMap::<String, FoldedEdge>::new();
    for edge in edges {
        if edge.kind == EdgeKind::MEMBER {
            continue;
        }

        let family = edge_family_for_kind(edge.kind);
        let source_host = member_host_by_id.get(&edge.source).cloned();
        let target_host = member_host_by_id.get(&edge.target).cloned();
        let source_node_id = source_host.clone().unwrap_or_else(|| edge.source.clone());
        let target_node_id = target_host.clone().unwrap_or_else(|| edge.target.clone());
        let source = canonical_node_by_id
            .get(&source_node_id)
            .cloned()
            .unwrap_or(source_node_id);
        let target = canonical_node_by_id
            .get(&target_node_id)
            .cloned()
            .unwrap_or(target_node_id);
        if source == target {
            continue;
        }

        let source_handle = if source_host.is_some() {
            format!("source-member-{}", edge.source.0)
        } else if family == CanonicalEdgeFamily::Hierarchy {
            "source-node-top".to_string()
        } else {
            "source-node".to_string()
        };
        let target_handle = if target_host.is_some() {
            format!("target-member-{}", edge.target.0)
        } else if family == CanonicalEdgeFamily::Hierarchy {
            "target-node-bottom".to_string()
        } else {
            "target-node".to_string()
        };

        let key = format!(
            "{}:{}:{}:{}:{}",
            edge_kind_name(edge.kind),
            source.0,
            source_handle,
            target.0,
            target_handle
        );
        if let Some(existing) = folded.get_mut(&key) {
            existing.multiplicity = existing.multiplicity.saturating_add(1);
            existing.source_edge_ids.push(edge.id.0.clone());
            existing.certainty =
                merge_certainty(existing.certainty.as_deref(), edge.certainty.as_deref());
            continue;
        }

        folded.insert(
            key.clone(),
            FoldedEdge {
                id: key,
                source_edge_ids: vec![edge.id.0.clone()],
                source,
                target,
                kind: edge.kind,
                certainty: edge.certainty.clone(),
                multiplicity: 1,
                source_handle,
                target_handle,
                family,
            },
        );
    }

    let mut folded_edges = folded.into_values().collect::<Vec<_>>();
    folded_edges.sort_by(|left, right| left.id.cmp(&right.id));

    FoldResult {
        folded_edges,
        canonical_node_by_id,
        duplicate_count_by_canonical,
        merged_ids_by_canonical,
    }
}
fn infer_member_visibility(
    kind: NodeKind,
    label: &str,
    explicit_access: Option<MemberAccess>,
) -> CanonicalMemberVisibility {
    if let Some(access) = explicit_access {
        return match access {
            MemberAccess::Public => CanonicalMemberVisibility::Public,
            MemberAccess::Protected => CanonicalMemberVisibility::Protected,
            MemberAccess::Private => CanonicalMemberVisibility::Private,
            MemberAccess::Default => CanonicalMemberVisibility::Default,
        };
    }
    if is_private_member_kind(kind) {
        return CanonicalMemberVisibility::Private;
    }
    if is_public_member_kind(kind) {
        return CanonicalMemberVisibility::Public;
    }
    if label.starts_with('_')
        || label.ends_with('_')
        || label
            .strip_prefix("m_")
            .and_then(|tail| tail.chars().next())
            .is_some_and(|c| c.is_ascii_alphanumeric())
    {
        return CanonicalMemberVisibility::Private;
    }
    CanonicalMemberVisibility::Public
}

fn certainty_rank(certainty: Option<&str>) -> i32 {
    match certainty.map(|value| value.to_ascii_lowercase()) {
        Some(value) if value == "uncertain" => 2,
        Some(value) if value == "probable" => 1,
        _ => 0,
    }
}

fn merge_certainty(existing: Option<&str>, next: Option<&str>) -> Option<String> {
    if certainty_rank(next) > certainty_rank(existing) {
        next.map(ToOwned::to_owned)
    } else {
        existing.map(ToOwned::to_owned)
    }
}

fn dedupe_key_for_node(kind: NodeKind, label: &str, depth: i32, is_center: bool) -> Option<String> {
    if is_center {
        return None;
    }
    if is_card_node_kind(kind) {
        return Some(format!(
            "{}:{}",
            node_kind_name(kind),
            label.to_ascii_lowercase()
        ));
    }
    Some(format!(
        "{}:{}:{}",
        node_kind_name(kind),
        label.to_ascii_lowercase(),
        depth
    ))
}

fn synthetic_host_id(host_label: &str) -> String {
    let mut slug = String::new();
    let mut last_dash = false;
    for ch in host_label.trim().chars() {
        let lowered = ch.to_ascii_lowercase();
        if lowered.is_ascii_alphanumeric() {
            slug.push(lowered);
            last_dash = false;
            continue;
        }
        if !slug.is_empty() && !last_dash {
            slug.push('-');
            last_dash = true;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    if slug.is_empty() {
        slug.push_str("anonymous");
    }
    format!("__synthetic_host__{slug}")
}

fn js_round(value: f64) -> i32 {
    (value + 0.5).floor() as i32
}

fn estimated_node_width(kind: NodeKind, label: &str, members: &[CanonicalMemberDto]) -> f32 {
    if is_card_node_kind(kind) {
        let member_longest = members
            .iter()
            .map(|member| member.label.chars().count())
            .max()
            .unwrap_or(0);
        let longest_label = label.chars().count().max(member_longest);
        return clamp(
            CARD_CHROME_WIDTH + text_width(longest_label),
            CARD_WIDTH_MIN,
            CARD_WIDTH_MAX,
        );
    }
    clamp(
        PILL_CHROME_WIDTH + text_width(label.chars().count()),
        PILL_WIDTH_MIN,
        PILL_WIDTH_MAX,
    )
}

fn estimated_node_height(kind: NodeKind, members: &[CanonicalMemberDto]) -> f32 {
    if !is_card_node_kind(kind) {
        return PILL_HEIGHT;
    }
    let public_count = members
        .iter()
        .filter(|member| member.visibility == CanonicalMemberVisibility::Public)
        .count();
    let protected_count = members
        .iter()
        .filter(|member| member.visibility == CanonicalMemberVisibility::Protected)
        .count();
    let private_count = members
        .iter()
        .filter(|member| member.visibility == CanonicalMemberVisibility::Private)
        .count();
    let default_count = members
        .iter()
        .filter(|member| member.visibility == CanonicalMemberVisibility::Default)
        .count();
    let section_count = [public_count, protected_count, private_count, default_count]
        .into_iter()
        .filter(|count| *count > 0)
        .count();
    let effective_sections = if section_count == 0 { 1 } else { section_count };
    clamp(
        74.0 + effective_sections as f32 * 28.0 + std::cmp::max(1, members.len()) as f32 * 21.0,
        CARD_HEIGHT_MIN,
        CARD_HEIGHT_MAX,
    )
}

fn text_width(chars: usize) -> f32 {
    chars as f32 * APPROX_CHAR_WIDTH
}

fn clamp(value: f32, min: f32, max: f32) -> f32 {
    value.max(min).min(max)
}

fn edge_family_for_kind(kind: EdgeKind) -> CanonicalEdgeFamily {
    match kind {
        EdgeKind::INHERITANCE
        | EdgeKind::OVERRIDE
        | EdgeKind::TYPE_ARGUMENT
        | EdgeKind::TEMPLATE_SPECIALIZATION => CanonicalEdgeFamily::Hierarchy,
        _ => CanonicalEdgeFamily::Flow,
    }
}

fn is_structural_kind(kind: NodeKind) -> bool {
    matches!(
        kind,
        NodeKind::CLASS
            | NodeKind::STRUCT
            | NodeKind::INTERFACE
            | NodeKind::UNION
            | NodeKind::ENUM
            | NodeKind::NAMESPACE
            | NodeKind::MODULE
            | NodeKind::PACKAGE
    )
}

fn is_card_node_kind(kind: NodeKind) -> bool {
    is_structural_kind(kind) || kind == NodeKind::FILE
}

fn is_private_member_kind(kind: NodeKind) -> bool {
    matches!(
        kind,
        NodeKind::FIELD
            | NodeKind::VARIABLE
            | NodeKind::GLOBAL_VARIABLE
            | NodeKind::CONSTANT
            | NodeKind::ENUM_CONSTANT
    )
}

fn is_public_member_kind(kind: NodeKind) -> bool {
    matches!(
        kind,
        NodeKind::FUNCTION | NodeKind::METHOD | NodeKind::MACRO
    )
}

fn edge_kind_name(kind: EdgeKind) -> &'static str {
    match kind {
        EdgeKind::MEMBER => "MEMBER",
        EdgeKind::TYPE_USAGE => "TYPE_USAGE",
        EdgeKind::USAGE => "USAGE",
        EdgeKind::CALL => "CALL",
        EdgeKind::INHERITANCE => "INHERITANCE",
        EdgeKind::OVERRIDE => "OVERRIDE",
        EdgeKind::TYPE_ARGUMENT => "TYPE_ARGUMENT",
        EdgeKind::TEMPLATE_SPECIALIZATION => "TEMPLATE_SPECIALIZATION",
        EdgeKind::INCLUDE => "INCLUDE",
        EdgeKind::IMPORT => "IMPORT",
        EdgeKind::MACRO_USAGE => "MACRO_USAGE",
        EdgeKind::ANNOTATION_USAGE => "ANNOTATION_USAGE",
        EdgeKind::UNKNOWN => "UNKNOWN",
    }
}

fn node_kind_name(kind: NodeKind) -> &'static str {
    match kind {
        NodeKind::MODULE => "MODULE",
        NodeKind::NAMESPACE => "NAMESPACE",
        NodeKind::PACKAGE => "PACKAGE",
        NodeKind::FILE => "FILE",
        NodeKind::STRUCT => "STRUCT",
        NodeKind::CLASS => "CLASS",
        NodeKind::INTERFACE => "INTERFACE",
        NodeKind::ANNOTATION => "ANNOTATION",
        NodeKind::UNION => "UNION",
        NodeKind::ENUM => "ENUM",
        NodeKind::TYPEDEF => "TYPEDEF",
        NodeKind::TYPE_PARAMETER => "TYPE_PARAMETER",
        NodeKind::BUILTIN_TYPE => "BUILTIN_TYPE",
        NodeKind::FUNCTION => "FUNCTION",
        NodeKind::METHOD => "METHOD",
        NodeKind::MACRO => "MACRO",
        NodeKind::GLOBAL_VARIABLE => "GLOBAL_VARIABLE",
        NodeKind::FIELD => "FIELD",
        NodeKind::VARIABLE => "VARIABLE",
        NodeKind::CONSTANT => "CONSTANT",
        NodeKind::ENUM_CONSTANT => "ENUM_CONSTANT",
        NodeKind::UNKNOWN => "UNKNOWN",
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str, label: &str, kind: NodeKind, depth: u32) -> GraphNodeDto {
        GraphNodeDto {
            id: NodeId(id.to_string()),
            label: label.to_string(),
            kind,
            depth,
            label_policy: None,
            badge_visible_members: None,
            badge_total_members: None,
            merged_symbol_examples: Vec::new(),
            file_path: None,
            qualified_name: None,
            member_access: None,
        }
    }

    fn edge(id: &str, source: &str, target: &str, kind: EdgeKind) -> GraphEdgeDto {
        GraphEdgeDto {
            id: codestory_api::EdgeId(id.to_string()),
            source: NodeId(source.to_string()),
            target: NodeId(target.to_string()),
            kind,
            confidence: None,
            certainty: None,
            callsite_identity: None,
            candidate_targets: Vec::new(),
        }
    }

    #[test]
    fn center_member_promotes_host_and_uses_member_handles() {
        let nodes = vec![
            node("workspace", "WorkspaceIndexer", NodeKind::CLASS, 0),
            node("run", "WorkspaceIndexer::run", NodeKind::METHOD, 0),
            node("merge", "Storage::merge", NodeKind::METHOD, 1),
        ];
        let edges = vec![
            edge("member-1", "workspace", "run", EdgeKind::MEMBER),
            edge("call-1", "run", "merge", EdgeKind::CALL),
        ];

        let layout = build_canonical_layout(&NodeId("run".to_string()), &nodes, &edges);

        assert_eq!(layout.center_node_id.0, "workspace");
        assert!(
            layout.nodes.iter().any(|node| {
                node.id.0 == "workspace"
                    && node.center
                    && node.members.iter().any(|member| member.id.0 == "run")
            }),
            "expected center host node to include the focused member"
        );
        assert!(
            layout.edges.iter().any(|edge| {
                edge.kind == EdgeKind::CALL
                    && edge.source_handle == "source-member-run"
                    && edge.target_handle == "target-member-merge"
            }),
            "expected folded edge handles to reference member endpoints"
        );
    }

    #[test]
    fn detached_qualified_members_create_synthetic_host() {
        let nodes = vec![
            node("run", "TicTacToe::run", NodeKind::FUNCTION, 0),
            node("field_is_draw", "Field::is_draw", NodeKind::FUNCTION, 1),
            node("field_make_move", "Field::make_move", NodeKind::FUNCTION, 1),
        ];
        let edges = vec![
            edge("call-1", "run", "field_is_draw", EdgeKind::CALL),
            edge("call-2", "run", "field_make_move", EdgeKind::CALL),
        ];

        let layout = build_canonical_layout(&NodeId("run".to_string()), &nodes, &edges);
        let host = layout.nodes.iter().find(|node| node.label == "Field");

        assert!(
            host.is_some(),
            "expected synthetic host node for detached members"
        );
        let host = host.expect("synthetic host node");
        assert_eq!(host.kind, NodeKind::CLASS);
        assert!(
            host.members
                .iter()
                .any(|member| member.id.0 == "field_is_draw")
        );
        assert!(
            host.members
                .iter()
                .any(|member| member.id.0 == "field_make_move")
        );
    }

    #[test]
    fn folds_parallel_edges_and_preserves_source_edge_ids() {
        let nodes = vec![
            node("runner", "Runner::run", NodeKind::METHOD, 0),
            node("worker", "Worker::execute", NodeKind::METHOD, 1),
        ];
        let mut first = edge("call-1", "runner", "worker", EdgeKind::CALL);
        first.certainty = Some("probable".to_string());
        let mut second = edge("call-2", "runner", "worker", EdgeKind::CALL);
        second.certainty = Some("uncertain".to_string());
        let edges = vec![first, second];

        let layout = build_canonical_layout(&NodeId("runner".to_string()), &nodes, &edges);
        let call_edges = layout
            .edges
            .iter()
            .filter(|edge| edge.kind == EdgeKind::CALL)
            .collect::<Vec<_>>();

        assert_eq!(call_edges.len(), 1);
        let folded = call_edges[0];
        assert_eq!(folded.multiplicity, 2);
        assert_eq!(
            folded
                .source_edge_ids
                .iter()
                .map(|id| id.0.clone())
                .collect::<Vec<_>>(),
            vec!["call-1".to_string(), "call-2".to_string()]
        );
        assert_eq!(folded.certainty.as_deref(), Some("uncertain"));
    }

    #[test]
    fn canonical_ordering_is_stable() {
        let nodes = vec![
            node("host", "Service", NodeKind::CLASS, 0),
            node("run", "Service::run", NodeKind::METHOD, 0),
            node("helper", "Helper::assist", NodeKind::METHOD, 1),
            node("worker", "Worker::execute", NodeKind::METHOD, 1),
        ];
        let edges = vec![
            edge("member-1", "host", "run", EdgeKind::MEMBER),
            edge("call-1", "run", "helper", EdgeKind::CALL),
            edge("call-2", "run", "worker", EdgeKind::CALL),
        ];

        let first = build_canonical_layout(&NodeId("run".to_string()), &nodes, &edges);
        let second = build_canonical_layout(&NodeId("run".to_string()), &nodes, &edges);

        let first_node_ids = first
            .nodes
            .iter()
            .map(|node| node.id.0.clone())
            .collect::<Vec<_>>();
        let second_node_ids = second
            .nodes
            .iter()
            .map(|node| node.id.0.clone())
            .collect::<Vec<_>>();
        let first_edge_ids = first
            .edges
            .iter()
            .map(|edge| edge.id.clone())
            .collect::<Vec<_>>();
        let second_edge_ids = second
            .edges
            .iter()
            .map(|edge| edge.id.clone())
            .collect::<Vec<_>>();

        assert_eq!(first_node_ids, second_node_ids);
        assert_eq!(first_edge_ids, second_edge_ids);
    }
}
