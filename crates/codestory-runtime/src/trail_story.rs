use codestory_contracts::api::{
    EdgeKind, GraphEdgeDto, GraphNodeDto, GraphResponse, NodeDetailsDto, NodeId, NodeKind,
    TrailCallerScope, TrailConfigDto, TrailDirection, TrailMode, TrailStoryDto, TrailStoryStepDto,
};
use std::collections::{HashMap, HashSet};
use std::path::Path;

const TRAIL_STORY_CORE_FLOW_LIMIT: usize = 16;
const TRAIL_STORY_PREVIEW_LIMIT: usize = 5;

pub(crate) fn build_trail_story(
    project_root: Option<&Path>,
    focus: &NodeDetailsDto,
    trail: &GraphResponse,
    req: &TrailConfigDto,
) -> TrailStoryDto {
    let nodes_by_id = trail
        .nodes
        .iter()
        .map(|node| (node.id.clone(), node))
        .collect::<HashMap<_, _>>();
    let mut incoming_counts = HashMap::<_, u32>::new();
    for edge in &trail.edges {
        *incoming_counts.entry(edge.target.clone()).or_default() += 1;
    }

    let test_nodes = trail
        .nodes
        .iter()
        .filter(|node| is_test_like_story_node(node))
        .collect::<Vec<_>>();

    let mut entry_points = Vec::new();
    entry_points.push(format!("focus: {}", story_focus_ref(project_root, focus)));
    for node in trail
        .nodes
        .iter()
        .filter(|node| incoming_counts.get(&node.id).copied().unwrap_or_default() == 0)
        .filter(|node| node.id != focus.id)
        .take(TRAIL_STORY_PREVIEW_LIMIT)
    {
        entry_points.push(format!("entry: {}", story_node_ref(project_root, node)));
    }
    if entry_points.len() == 1 && trail.edges.is_empty() {
        entry_points.push("no graph entry edges were returned for this focus".to_string());
    }

    let grouped = grouped_story_steps(project_root, trail, req, &nodes_by_id);
    let core_flow = grouped
        .runtime_flow
        .iter()
        .chain(grouped.data_flow.iter())
        .chain(grouped.type_structure.iter())
        .take(TRAIL_STORY_CORE_FLOW_LIMIT)
        .cloned()
        .collect::<Vec<_>>();
    let side_effects = side_effects_for_story(project_root, trail, &nodes_by_id);
    let uncertainty = uncertainty_for_story(project_root, trail, &nodes_by_id, req);
    let test_scope = test_scope_for_story(project_root, req, &test_nodes);
    let limits = limits_for_story(trail, req, &nodes_by_id);
    let structural_only = trail_is_structural_only(trail, &nodes_by_id);
    let summary = format!(
        "Story trail around `{}` found {} nodes and {} edges; mode={} direction={} tests={} utility_calls={} truncated={} structural_only={}.",
        focus.display_name,
        trail.nodes.len(),
        trail.edges.len(),
        story_trail_mode(req.mode),
        story_trail_direction(req.direction),
        if req.caller_scope == TrailCallerScope::IncludeTestsAndBenches {
            "included"
        } else {
            "excluded"
        },
        if req.show_utility_calls {
            "included"
        } else {
            "hidden"
        },
        trail.truncated,
        structural_only
    );

    TrailStoryDto {
        summary,
        entry_points,
        core_flow,
        runtime_flow: grouped.runtime_flow,
        data_flow: grouped.data_flow,
        type_structure: grouped.type_structure,
        utility_calls: grouped.utility_calls,
        side_effects,
        uncertainty,
        test_scope,
        limits,
    }
}

#[derive(Default)]
struct GroupedStorySteps {
    runtime_flow: Vec<TrailStoryStepDto>,
    data_flow: Vec<TrailStoryStepDto>,
    type_structure: Vec<TrailStoryStepDto>,
    utility_calls: Vec<TrailStoryStepDto>,
}

fn grouped_story_steps(
    project_root: Option<&Path>,
    trail: &GraphResponse,
    req: &TrailConfigDto,
    nodes_by_id: &HashMap<NodeId, &GraphNodeDto>,
) -> GroupedStorySteps {
    let mut grouped = GroupedStorySteps::default();
    let mut seen = HashSet::new();
    for edge in &trail.edges {
        let target = nodes_by_id.get(&edge.target).copied();
        let step = story_step(project_root, edge, nodes_by_id);
        let key = format!(
            "{}\u{1f}{}\u{1f}{}\u{1f}{}",
            step.source, step.relation, step.target, step.certainty
        );
        if !seen.insert(key) {
            continue;
        }
        if edge_is_utility_call(edge, target) {
            if req.show_utility_calls {
                grouped.utility_calls.push(step);
            }
            continue;
        }
        match story_edge_group(edge.kind) {
            StoryEdgeGroup::Runtime => grouped.runtime_flow.push(step),
            StoryEdgeGroup::Data => grouped.data_flow.push(step),
            StoryEdgeGroup::TypeStructure => grouped.type_structure.push(step),
        }
    }
    grouped.runtime_flow.truncate(TRAIL_STORY_CORE_FLOW_LIMIT);
    grouped.data_flow.truncate(TRAIL_STORY_CORE_FLOW_LIMIT);
    grouped.type_structure.truncate(TRAIL_STORY_CORE_FLOW_LIMIT);
    grouped.utility_calls.truncate(TRAIL_STORY_PREVIEW_LIMIT);
    grouped
}

enum StoryEdgeGroup {
    Runtime,
    Data,
    TypeStructure,
}

fn story_edge_group(kind: EdgeKind) -> StoryEdgeGroup {
    match kind {
        EdgeKind::CALL | EdgeKind::MACRO_USAGE => StoryEdgeGroup::Runtime,
        EdgeKind::USAGE | EdgeKind::INCLUDE | EdgeKind::IMPORT | EdgeKind::ANNOTATION_USAGE => {
            StoryEdgeGroup::Data
        }
        EdgeKind::TYPE_USAGE
        | EdgeKind::MEMBER
        | EdgeKind::INHERITANCE
        | EdgeKind::OVERRIDE
        | EdgeKind::TYPE_ARGUMENT
        | EdgeKind::TEMPLATE_SPECIALIZATION
        | EdgeKind::UNKNOWN => StoryEdgeGroup::TypeStructure,
    }
}

fn edge_is_utility_call(edge: &GraphEdgeDto, target: Option<&GraphNodeDto>) -> bool {
    if edge.kind != EdgeKind::CALL {
        return false;
    }
    let Some(target) = target else {
        return false;
    };
    let normalized_label = target.label.to_ascii_lowercase();
    if normalized_label
        .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))
        .any(|segment| {
            matches!(
                segment,
                "map_err" | "unwrap_or" | "unwrap_or_else" | "to_string" | "as_ref" | "as_mut"
            )
        })
    {
        return true;
    }
    let tokens = story_identifier_tokens(&target.label);
    tokens.iter().any(|token| {
        matches!(
            token.as_str(),
            "ok" | "err"
                | "some"
                | "none"
                | "clone"
                | "cloned"
                | "copy"
                | "copied"
                | "from"
                | "into"
                | "map"
                | "map_err"
                | "unwrap"
                | "unwrap_or"
                | "unwrap_or_else"
                | "to_string"
                | "as_ref"
                | "as_mut"
                | "lock"
                | "borrow"
                | "default"
        )
    })
}

fn story_step(
    project_root: Option<&Path>,
    edge: &GraphEdgeDto,
    nodes_by_id: &HashMap<NodeId, &GraphNodeDto>,
) -> TrailStoryStepDto {
    let source = nodes_by_id
        .get(&edge.source)
        .map(|node| story_node_ref(project_root, node))
        .unwrap_or_else(|| edge.source.0.clone());
    let target = nodes_by_id
        .get(&edge.target)
        .map(|node| story_node_ref(project_root, node))
        .unwrap_or_else(|| edge.target.0.clone());
    let relation = story_relation(edge.kind).to_string();
    let certainty = story_certainty(edge);
    let confidence = edge
        .confidence
        .map(|value| format!(" confidence={value:.2}"))
        .unwrap_or_default();
    let candidates = if edge.candidate_targets.is_empty() {
        String::new()
    } else {
        format!(" candidate_targets={}", edge.candidate_targets.len())
    };
    let callsite = edge
        .callsite_identity
        .as_deref()
        .map(|value| format!(" callsite={value}"))
        .unwrap_or_default();
    let note = format!(
        "{} {} edge{}{}{}",
        certainty,
        format!("{:?}", edge.kind).to_lowercase(),
        confidence,
        candidates,
        callsite
    );

    TrailStoryStepDto {
        edge_id: edge.id.0.clone(),
        source,
        relation,
        target,
        certainty,
        note,
    }
}

fn story_node_ref(project_root: Option<&Path>, node: &GraphNodeDto) -> String {
    let path = node
        .file_path
        .as_deref()
        .map(|value| format!(" `{}`", story_path(project_root, value)))
        .unwrap_or_else(|| " [no source path]".to_string());
    format!("{} [{}]{}", node.label, story_node_kind(node.kind), path)
}

fn story_focus_ref(project_root: Option<&Path>, node: &NodeDetailsDto) -> String {
    let path = node
        .file_path
        .as_deref()
        .map(|value| format!(" `{}`", story_path(project_root, value)))
        .unwrap_or_else(|| " [no source path]".to_string());
    format!(
        "{} [{}]{}",
        node.display_name,
        story_node_kind(node.kind),
        path
    )
}

fn story_path(project_root: Option<&Path>, value: &str) -> String {
    let path = Path::new(value);
    project_root
        .and_then(|root| path.strip_prefix(root).ok())
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|| value.replace('\\', "/"))
}

fn story_node_kind(kind: NodeKind) -> &'static str {
    match kind {
        NodeKind::MODULE => "module",
        NodeKind::NAMESPACE => "namespace",
        NodeKind::PACKAGE => "package",
        NodeKind::FILE => "file",
        NodeKind::STRUCT => "struct",
        NodeKind::CLASS => "class",
        NodeKind::INTERFACE => "interface",
        NodeKind::ANNOTATION => "annotation",
        NodeKind::UNION => "union",
        NodeKind::ENUM => "enum",
        NodeKind::TYPEDEF => "typedef",
        NodeKind::TYPE_PARAMETER => "type_parameter",
        NodeKind::BUILTIN_TYPE => "builtin_type",
        NodeKind::FUNCTION => "function",
        NodeKind::METHOD => "method",
        NodeKind::MACRO => "macro",
        NodeKind::GLOBAL_VARIABLE => "global_variable",
        NodeKind::FIELD => "field",
        NodeKind::VARIABLE => "variable",
        NodeKind::CONSTANT => "constant",
        NodeKind::ENUM_CONSTANT => "enum_constant",
        NodeKind::UNKNOWN => "unknown",
    }
}

fn story_relation(kind: EdgeKind) -> &'static str {
    match kind {
        EdgeKind::CALL => "calls",
        EdgeKind::USAGE => "uses",
        EdgeKind::TYPE_USAGE => "uses type",
        EdgeKind::MEMBER => "contains",
        EdgeKind::INHERITANCE => "inherits from",
        EdgeKind::OVERRIDE => "overrides",
        EdgeKind::TYPE_ARGUMENT => "passes type argument to",
        EdgeKind::TEMPLATE_SPECIALIZATION => "specializes",
        EdgeKind::INCLUDE => "includes",
        EdgeKind::IMPORT => "imports",
        EdgeKind::MACRO_USAGE => "uses macro",
        EdgeKind::ANNOTATION_USAGE => "uses annotation",
        EdgeKind::UNKNOWN => "relates to",
    }
}

fn story_certainty(edge: &GraphEdgeDto) -> String {
    edge.certainty
        .as_deref()
        .map(str::to_ascii_lowercase)
        .unwrap_or_else(|| "missing certainty metadata".to_string())
}

fn is_uncertain_story_certainty(certainty: &str) -> bool {
    matches!(
        certainty,
        "probable" | "uncertain" | "speculative" | "missing certainty metadata"
    )
}

fn side_effects_for_story(
    project_root: Option<&Path>,
    trail: &GraphResponse,
    nodes_by_id: &HashMap<NodeId, &GraphNodeDto>,
) -> Vec<String> {
    let mut side_effects = Vec::new();
    let mut seen = HashSet::new();
    for edge in &trail.edges {
        let target = nodes_by_id.get(&edge.target).copied();
        if !edge_suggests_side_effect(edge, target) {
            continue;
        }
        let step = story_step(project_root, edge, nodes_by_id);
        let key = format!(
            "{}\u{1f}{}\u{1f}{}",
            step.source, step.relation, step.target
        );
        if !seen.insert(key) {
            continue;
        }
        side_effects.push(format!(
            "possible side-effect candidate [{}] {} {} {} (certainty={})",
            step.edge_id, step.source, step.relation, step.target, step.certainty
        ));
    }
    if side_effects.is_empty() {
        side_effects.push(
            "none detected from conservative edge-kind and target-name heuristics; inspect snippets for runtime effects"
                .to_string(),
        );
    }
    side_effects
}

fn edge_suggests_side_effect(edge: &GraphEdgeDto, target: Option<&GraphNodeDto>) -> bool {
    if edge.kind != EdgeKind::CALL {
        return false;
    }
    let Some(target) = target else {
        return false;
    };
    let tokens = story_identifier_tokens(&target.label);
    tokens.iter().any(|token| {
        matches!(
            token.as_str(),
            "write"
                | "save"
                | "persist"
                | "update"
                | "insert"
                | "delete"
                | "remove"
                | "emit"
                | "send"
                | "flush"
                | "commit"
                | "publish"
        )
    })
}

fn story_identifier_tokens(value: &str) -> Vec<String> {
    let mut normalized = String::new();
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            if ch.is_ascii_uppercase() {
                normalized.push(' ');
            }
            normalized.push(ch.to_ascii_lowercase());
        } else {
            normalized.push(' ');
        }
    }
    normalized
        .split_whitespace()
        .filter(|token| !token.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn uncertainty_for_story(
    project_root: Option<&Path>,
    trail: &GraphResponse,
    nodes_by_id: &HashMap<NodeId, &GraphNodeDto>,
    req: &TrailConfigDto,
) -> Vec<String> {
    let mut uncertainty = Vec::new();
    if req.hide_speculative {
        uncertainty.push(
            "hide_speculative was applied before story rendering; uncertain/speculative edges may have been removed"
                .to_string(),
        );
    }
    if !req.edge_filter.is_empty() {
        uncertainty.push("edge filters were applied before rendering".to_string());
    }
    if let Some(focus) = nodes_by_id.get(&req.root_id)
        && story_node_is_callable(focus)
        && !trail_has_incoming_call_edge(trail, &req.root_id)
    {
        uncertainty.push(
            "focus has no visible incoming call edges in the rendered trail; treat runtime participation as unproven unless a framework/runtime entry path is source-verified"
                .to_string(),
        );
    }
    for edge in &trail.edges {
        let certainty = story_certainty(edge);
        if !is_uncertain_story_certainty(&certainty) {
            continue;
        }
        let step = story_step(project_root, edge, nodes_by_id);
        uncertainty.push(format!(
            "[{}] {} {} {} is {}. {}",
            step.edge_id, step.source, step.relation, step.target, step.certainty, step.note
        ));
    }
    if trail.edges.is_empty() {
        uncertainty.push("no rendered trail edges to evaluate for certainty".to_string());
    } else if uncertainty.is_empty() {
        uncertainty.push("all rendered trail edges are explicitly marked certain".to_string());
    }
    uncertainty
}

fn story_node_is_callable(node: &GraphNodeDto) -> bool {
    matches!(
        node.kind,
        NodeKind::FUNCTION | NodeKind::METHOD | NodeKind::MACRO
    )
}

fn trail_has_incoming_call_edge(trail: &GraphResponse, node_id: &NodeId) -> bool {
    trail
        .edges
        .iter()
        .any(|edge| edge.kind == EdgeKind::CALL && &edge.target == node_id)
}

fn test_scope_for_story(
    project_root: Option<&Path>,
    req: &TrailConfigDto,
    test_nodes: &[&GraphNodeDto],
) -> Vec<String> {
    let mut scope = Vec::new();
    if req.caller_scope == TrailCallerScope::IncludeTestsAndBenches {
        scope.push("tests and benches included by request caller scope".to_string());
    } else {
        scope.push(
            "tests and benches excluded by production-only caller scope; request IncludeTestsAndBenches to include them"
                .to_string(),
        );
    }
    if test_nodes.is_empty() {
        scope.push("no test-like nodes are present in the rendered trail".to_string());
    } else {
        scope.push(format!(
            "{} test-like node(s) present: {}",
            test_nodes.len(),
            test_nodes
                .iter()
                .take(TRAIL_STORY_PREVIEW_LIMIT)
                .map(|node| story_node_ref(project_root, node))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    scope.push(if req.show_utility_calls {
        "utility/helper calls included by request".to_string()
    } else {
        "utility/helper calls hidden by default; enable show_utility_calls to include them"
            .to_string()
    });
    scope
}

fn limits_for_story(
    trail: &GraphResponse,
    req: &TrailConfigDto,
    nodes_by_id: &HashMap<NodeId, &GraphNodeDto>,
) -> Vec<String> {
    let mut limits = Vec::new();
    if trail.edges.len() > TRAIL_STORY_CORE_FLOW_LIMIT {
        limits.push(format!(
            "core_flow shows first {} of {} rendered edges",
            TRAIL_STORY_CORE_FLOW_LIMIT,
            trail.edges.len()
        ));
    }
    if trail.truncated {
        limits.push(format!(
            "trail was truncated at max_nodes={} with omitted_edge_count={}",
            req.max_nodes, trail.omitted_edge_count
        ));
    } else {
        limits.push(format!(
            "trail not truncated; max_nodes={} omitted_edge_count={}",
            req.max_nodes, trail.omitted_edge_count
        ));
    }
    if trail.edges.is_empty() {
        limits
            .push("no edges were returned, so core flow is limited to the focus node".to_string());
    }
    if trail_is_structural_only(trail, nodes_by_id) {
        limits.push(
            "structural-only trail: inspect child methods/occurrences and rerun snippet with --function-body on a concrete function or method anchor"
                .to_string(),
        );
    }
    if trail.nodes.iter().any(|node| node.file_path.is_none()) {
        limits.push(
            "one or more trail nodes have no source path; treat them as navigation anchors until a path-backed occurrence or snippet is opened"
                .to_string(),
        );
    }
    limits
}

fn trail_is_structural_only(
    trail: &GraphResponse,
    nodes_by_id: &HashMap<NodeId, &GraphNodeDto>,
) -> bool {
    if trail.edges.is_empty() {
        return trail
            .nodes
            .iter()
            .any(|node| is_structural_story_kind(node.kind));
    }
    trail.edges.iter().all(|edge| {
        matches!(
            edge.kind,
            EdgeKind::MEMBER
                | EdgeKind::INHERITANCE
                | EdgeKind::TYPE_USAGE
                | EdgeKind::OVERRIDE
                | EdgeKind::TYPE_ARGUMENT
                | EdgeKind::TEMPLATE_SPECIALIZATION
        ) || nodes_by_id
            .get(&edge.source)
            .is_some_and(|node| is_structural_story_kind(node.kind))
            && nodes_by_id
                .get(&edge.target)
                .is_some_and(|node| is_structural_story_kind(node.kind))
    })
}

fn is_structural_story_kind(kind: NodeKind) -> bool {
    matches!(
        kind,
        NodeKind::MODULE
            | NodeKind::NAMESPACE
            | NodeKind::PACKAGE
            | NodeKind::FILE
            | NodeKind::STRUCT
            | NodeKind::CLASS
            | NodeKind::INTERFACE
            | NodeKind::ENUM
            | NodeKind::UNION
            | NodeKind::TYPEDEF
    )
}

fn is_test_like_story_node(node: &GraphNodeDto) -> bool {
    let text = format!(
        "{} {}",
        node.label.to_ascii_lowercase(),
        node.file_path
            .as_deref()
            .unwrap_or_default()
            .replace('\\', "/")
            .to_ascii_lowercase()
    );
    text.contains("/test")
        || text.contains("tests/")
        || text.contains("_test")
        || text.contains("test_")
        || text.contains("/benches/")
        || text.contains("bench_")
}

fn story_trail_mode(mode: TrailMode) -> &'static str {
    match mode {
        TrailMode::Neighborhood => "neighborhood",
        TrailMode::AllReferenced => "referenced",
        TrailMode::AllReferencing => "referencing",
        TrailMode::ToTargetSymbol => "to_target_symbol",
    }
}

fn story_trail_direction(direction: TrailDirection) -> &'static str {
    match direction {
        TrailDirection::Incoming => "incoming",
        TrailDirection::Outgoing => "outgoing",
        TrailDirection::Both => "both",
    }
}

#[cfg(test)]
mod trail_story_tests {
    use super::*;
    use codestory_contracts::api::{EdgeId, LayoutDirection};

    fn node(id: &str, label: &str, file_path: &str) -> GraphNodeDto {
        node_of_kind(id, label, file_path, NodeKind::FUNCTION)
    }

    fn node_of_kind(id: &str, label: &str, file_path: &str, kind: NodeKind) -> GraphNodeDto {
        GraphNodeDto {
            id: NodeId(id.to_string()),
            label: label.to_string(),
            kind,
            depth: 0,
            label_policy: None,
            badge_visible_members: None,
            badge_total_members: None,
            merged_symbol_examples: Vec::new(),
            file_path: (!file_path.is_empty()).then(|| file_path.to_string()),
            qualified_name: None,
            member_access: None,
        }
    }

    fn edge(id: usize, source: &str, target: &str, certainty: Option<&str>) -> GraphEdgeDto {
        GraphEdgeDto {
            id: EdgeId(format!("edge-{id}")),
            source: NodeId(source.to_string()),
            target: NodeId(target.to_string()),
            kind: EdgeKind::CALL,
            confidence: Some(0.99),
            certainty: certainty.map(ToOwned::to_owned),
            callsite_identity: None,
            candidate_targets: Vec::new(),
        }
    }

    fn edge_of_kind(
        id: usize,
        source: &str,
        target: &str,
        kind: EdgeKind,
        certainty: Option<&str>,
    ) -> GraphEdgeDto {
        GraphEdgeDto {
            id: EdgeId(format!("edge-{id}")),
            source: NodeId(source.to_string()),
            target: NodeId(target.to_string()),
            kind,
            confidence: Some(0.99),
            certainty: certainty.map(ToOwned::to_owned),
            callsite_identity: None,
            candidate_targets: Vec::new(),
        }
    }

    fn request(story: bool) -> TrailConfigDto {
        TrailConfigDto {
            root_id: NodeId("focus".to_string()),
            mode: TrailMode::Neighborhood,
            target_id: None,
            depth: 2,
            direction: TrailDirection::Both,
            caller_scope: TrailCallerScope::ProductionOnly,
            edge_filter: Vec::new(),
            show_utility_calls: false,
            hide_speculative: false,
            story,
            node_filter: Vec::new(),
            max_nodes: 24,
            layout_direction: LayoutDirection::Horizontal,
        }
    }

    fn focus_details() -> NodeDetailsDto {
        NodeDetailsDto {
            id: NodeId("focus".to_string()),
            kind: NodeKind::FUNCTION,
            display_name: "handle_request".to_string(),
            serialized_name: "handle_request".to_string(),
            qualified_name: None,
            canonical_id: None,
            file_path: Some("C:/repo/src/request.rs".to_string()),
            start_line: None,
            start_col: None,
            end_line: None,
            end_col: None,
            evidence_tier: None,
            evidence_producer: None,
            resolution_status: None,
            member_access: None,
            route_endpoint: None,
        }
    }

    #[test]
    fn trail_story_preserves_missing_certainty_and_reports_story_truncation() {
        let focus = focus_details();
        let mut nodes = vec![node("focus", "handle_request", "C:/repo/src/request.rs")];
        let mut edges = Vec::new();
        for index in 0..18 {
            let target = format!("target-{index}");
            nodes.push(node(
                &target,
                &format!("target_{index}"),
                "C:/repo/src/flow.rs",
            ));
            edges.push(edge(index, "focus", &target, None));
        }
        let trail = GraphResponse {
            center_id: NodeId("focus".to_string()),
            nodes,
            edges,
            truncated: false,
            omitted_edge_count: 0,
            canonical_layout: None,
        };

        let story = build_trail_story(None, &focus, &trail, &request(true));

        assert_eq!(story.core_flow.len(), TRAIL_STORY_CORE_FLOW_LIMIT);
        assert!(
            story
                .uncertainty
                .iter()
                .any(|item| item.contains("missing certainty metadata")),
            "missing certainty should remain textual uncertainty: {story:#?}"
        );
        assert!(
            story
                .limits
                .iter()
                .any(|item| item.contains("core_flow shows first 16 of 18 rendered edges")),
            "story-level truncation should be disclosed: {story:#?}"
        );
    }

    #[test]
    fn trail_story_reports_certainty_spectrum_textually() {
        let focus = focus_details();
        let trail = GraphResponse {
            center_id: NodeId("focus".to_string()),
            nodes: vec![
                node("focus", "handle_request", "C:/repo/src/request.rs"),
                node("certain", "validate_request", "C:/repo/src/request.rs"),
                node("probable", "load_profile", "C:/repo/src/profile.rs"),
                node(
                    "speculative",
                    "dynamic_plugin_hook",
                    "C:/repo/src/plugin.rs",
                ),
                node("missing", "legacy_dispatch", "C:/repo/src/legacy.rs"),
            ],
            edges: vec![
                edge(1, "focus", "certain", Some("certain")),
                edge(2, "focus", "probable", Some("probable")),
                edge(3, "focus", "speculative", Some("speculative")),
                edge(4, "focus", "missing", None),
            ],
            truncated: false,
            omitted_edge_count: 0,
            canonical_layout: None,
        };

        let story = build_trail_story(None, &focus, &trail, &request(true));
        let core_certainties = story
            .core_flow
            .iter()
            .map(|step| step.certainty.as_str())
            .collect::<Vec<_>>();

        assert!(
            core_certainties.contains(&"certain")
                && core_certainties.contains(&"probable")
                && core_certainties.contains(&"speculative")
                && core_certainties.contains(&"missing certainty metadata"),
            "core flow should keep every certainty label textual: {story:#?}"
        );
        assert!(
            story
                .uncertainty
                .iter()
                .any(|item| item.contains("speculative"))
                && story
                    .uncertainty
                    .iter()
                    .any(|item| item.contains("missing certainty metadata")),
            "uncertainty section should call out speculative and missing certainty: {story:#?}"
        );
    }

    #[test]
    fn trail_story_empty_edges_do_not_claim_certainty() {
        let focus = focus_details();
        let trail = GraphResponse {
            center_id: NodeId("focus".to_string()),
            nodes: vec![node("focus", "handle_request", "C:/repo/src/request.rs")],
            edges: Vec::new(),
            truncated: false,
            omitted_edge_count: 0,
            canonical_layout: None,
        };

        let story = build_trail_story(None, &focus, &trail, &request(true));

        assert!(
            story
                .uncertainty
                .iter()
                .any(|item| item.contains("no rendered trail edges to evaluate")),
            "empty story should not claim all edges are certain: {story:#?}"
        );
        assert!(
            story
                .uncertainty
                .iter()
                .any(|item| item.contains("no visible incoming call edges")),
            "callable focus with no visible callers should be labeled: {story:#?}"
        );
    }

    #[test]
    fn trail_story_flags_structural_only_and_missing_paths() {
        let focus = NodeDetailsDto {
            id: NodeId("focus".to_string()),
            kind: NodeKind::CLASS,
            display_name: "SourceGroupCxxCdb".to_string(),
            serialized_name: "SourceGroupCxxCdb".to_string(),
            qualified_name: None,
            canonical_id: None,
            file_path: None,
            start_line: None,
            start_col: None,
            end_line: None,
            end_col: None,
            evidence_tier: None,
            evidence_producer: None,
            resolution_status: None,
            member_access: None,
            route_endpoint: None,
        };
        let trail = GraphResponse {
            center_id: NodeId("focus".to_string()),
            nodes: vec![
                node_of_kind("focus", "SourceGroupCxxCdb", "", NodeKind::CLASS),
                node_of_kind("member", "getIndexerCommands", "", NodeKind::METHOD),
            ],
            edges: vec![edge_of_kind(
                1,
                "focus",
                "member",
                EdgeKind::MEMBER,
                Some("certain"),
            )],
            truncated: false,
            omitted_edge_count: 0,
            canonical_layout: None,
        };

        let story = build_trail_story(None, &focus, &trail, &request(true));

        assert!(
            story.summary.contains("structural_only=true"),
            "summary should expose structural-only state: {story:#?}"
        );
        assert!(
            story
                .limits
                .iter()
                .any(|item| item.contains("structural-only trail")
                    && item.contains("--function-body")),
            "limits should point to method/body snippets: {story:#?}"
        );
        assert!(
            story
                .entry_points
                .iter()
                .any(|item| item.contains("[no source path]")),
            "missing source paths should be visible: {story:#?}"
        );
    }

    #[test]
    fn trail_story_side_effects_are_conservative_candidates() {
        let focus = NodeDetailsDto {
            id: NodeId("focus".to_string()),
            kind: NodeKind::FUNCTION,
            display_name: "handle_request".to_string(),
            serialized_name: "handle_request".to_string(),
            qualified_name: None,
            canonical_id: None,
            file_path: None,
            start_line: None,
            start_col: None,
            end_line: None,
            end_col: None,
            evidence_tier: None,
            evidence_producer: None,
            resolution_status: None,
            member_access: None,
            route_endpoint: None,
        };
        let trail = GraphResponse {
            center_id: NodeId("focus".to_string()),
            nodes: vec![
                node("focus", "handle_request", "C:/repo/src/request.rs"),
                node("write", "write_audit_log", "C:/repo/src/audit.rs"),
                node("catalog", "catalog_entries", "C:/repo/src/catalog.rs"),
            ],
            edges: vec![
                edge(1, "focus", "write", Some("certain")),
                edge(2, "focus", "catalog", Some("certain")),
            ],
            truncated: false,
            omitted_edge_count: 0,
            canonical_layout: None,
        };

        let story = build_trail_story(None, &focus, &trail, &request(true));

        assert!(
            story
                .side_effects
                .iter()
                .any(|item| item.contains("possible side-effect candidate")
                    && item.contains("write_audit_log")),
            "write target should be flagged as a candidate: {story:#?}"
        );
        assert!(
            story
                .side_effects
                .iter()
                .all(|item| !item.contains("catalog_entries")),
            "catalog substring should not be treated as a side effect: {story:#?}"
        );
    }

    #[test]
    fn trail_story_utility_calls_are_suppressed_from_machine_output_by_default() {
        let focus = focus_details();
        let trail = GraphResponse {
            center_id: NodeId("focus".to_string()),
            nodes: vec![
                node("focus", "handle_request", "C:/repo/src/request.rs"),
                node("utility", "to_string", "C:/repo/src/request.rs"),
                node("business", "load_profile", "C:/repo/src/profile.rs"),
            ],
            edges: vec![
                edge(1, "focus", "utility", Some("certain")),
                edge(2, "focus", "business", Some("certain")),
            ],
            truncated: false,
            omitted_edge_count: 0,
            canonical_layout: None,
        };

        let hidden = build_trail_story(None, &focus, &trail, &request(true));
        assert!(
            hidden.utility_calls.is_empty(),
            "default JSON story should not leak hidden utility calls: {hidden:#?}"
        );
        assert!(
            hidden
                .runtime_flow
                .iter()
                .all(|step| !step.target.contains("to_string")),
            "default runtime flow should suppress utility calls: {hidden:#?}"
        );

        let mut visible_req = request(true);
        visible_req.show_utility_calls = true;
        let visible = build_trail_story(None, &focus, &trail, &visible_req);
        assert!(
            visible
                .utility_calls
                .iter()
                .any(|step| step.target.contains("to_string")),
            "explicit utility output should retain utility calls: {visible:#?}"
        );
        assert!(
            visible
                .runtime_flow
                .iter()
                .all(|step| !step.target.contains("to_string")),
            "utility calls should stay in the utility group, not duplicate runtime flow: {visible:#?}"
        );
    }
}
