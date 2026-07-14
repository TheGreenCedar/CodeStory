use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;

use codestory_contracts::api::{
    AffectedAnalysisDto, AffectedAnalysisRequest, AffectedChangeKindDto, AffectedChangeRecordDto,
    ApiError, EdgeKind, GraphNodeDto, LayoutDirection, NodeId, NodeOccurrencesRequest,
    SourceOccurrenceDto, SymbolContextDto, TrailCallerScope, TrailConfigDto, TrailContextDto,
    TrailDirection, TrailMode,
};
use serde::Serialize;

use crate::{AmbiguousTarget, AppController, ResolvedTarget, TargetResolution, TargetSelection};

const IMPACTED_SYMBOLS_CAP: u32 = 200;
const IMPACTED_ROUTES_CAP: u32 = 100;
const TRANSITIVE_CALLERS_CAP: usize = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SymbolWorkflowMode {
    Impact,
    TestMap,
}

impl SymbolWorkflowMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Impact => "impact",
            Self::TestMap => "test_map",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Impact => "Symbol Impact",
            Self::TestMap => "Symbol Test Map",
        }
    }
}

#[derive(Debug, Clone)]
pub struct SymbolWorkflowRequest {
    pub mode: SymbolWorkflowMode,
    pub target: TargetSelection,
    pub file_filter: Option<String>,
    pub depth: u32,
    pub max_nodes: u32,
    pub include_tests: bool,
}

#[derive(Debug, Clone)]
pub enum SymbolWorkflowOutcome {
    Complete(Box<SymbolWorkflowResponse>),
    Ambiguous(AmbiguousTarget),
    Rejected(String),
}

#[derive(Debug, Clone, Serialize)]
pub struct SymbolWorkflowResolution {
    #[serde(flatten)]
    pub target: ResolvedTarget,
    #[serde(skip)]
    pub occurrences: HashMap<NodeId, Vec<SourceOccurrenceDto>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SymbolWorkflowNode {
    pub node_id: NodeId,
    pub display_name: String,
    pub kind: String,
    pub file_path: Option<String>,
    pub depth: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct SymbolWorkflowRoute {
    pub display_name: String,
    pub method: String,
    pub path: String,
    pub file_path: Option<String>,
    pub line: Option<u32>,
    pub confidence: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SymbolWorkflowTest {
    pub path: String,
    pub reason: String,
    pub confidence: String,
    pub graph_depth: u32,
    pub impacted_symbol_count: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct SymbolWorkflowCaps {
    pub caller_depth: u32,
    pub caller_max_nodes: u32,
    pub affected_depth: u32,
    pub impacted_symbols_cap: u32,
    pub impacted_routes_cap: u32,
    pub affected_seed: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SymbolWorkflowResponse {
    pub workflow: &'static str,
    pub project_root: String,
    pub resolution: SymbolWorkflowResolution,
    pub symbol: SymbolContextDto,
    pub direct_callers: Vec<SymbolWorkflowNode>,
    pub transitive_callers: Vec<SymbolWorkflowNode>,
    pub impacted_files: Vec<String>,
    pub impacted_routes: Vec<SymbolWorkflowRoute>,
    pub likely_tests: Vec<SymbolWorkflowTest>,
    pub caps: SymbolWorkflowCaps,
    pub unknowns: Vec<String>,
    pub next_commands: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub affected: Option<AffectedAnalysisDto>,
    pub trail: TrailContextDto,
}

impl AppController {
    pub fn symbol_workflow(
        &self,
        request: SymbolWorkflowRequest,
    ) -> Result<SymbolWorkflowOutcome, ApiError> {
        let project_root = self.require_project_root()?;
        let target = match self.resolve_target(request.target, request.file_filter.as_deref())? {
            TargetResolution::Resolved(target) => *target,
            TargetResolution::Ambiguous(ambiguous) => {
                return Ok(SymbolWorkflowOutcome::Ambiguous(ambiguous));
            }
            TargetResolution::Rejected(message) => {
                return Ok(SymbolWorkflowOutcome::Rejected(message));
            }
        };

        let depth = request.depth.clamp(1, 8);
        let max_nodes = request.max_nodes.clamp(1, 200);
        let include_tests = request.include_tests || request.mode == SymbolWorkflowMode::TestMap;
        let symbol = self.symbol_context(target.selected.node_id.clone())?;
        let trail = self.trail_context(TrailConfigDto {
            root_id: target.selected.node_id.clone(),
            mode: TrailMode::AllReferencing,
            target_id: None,
            depth,
            direction: TrailDirection::Incoming,
            caller_scope: if include_tests {
                TrailCallerScope::IncludeTestsAndBenches
            } else {
                TrailCallerScope::ProductionOnly
            },
            edge_filter: Vec::new(),
            show_utility_calls: false,
            hide_speculative: true,
            story: false,
            node_filter: Vec::new(),
            max_nodes,
            layout_direction: LayoutDirection::Horizontal,
        })?;

        let affected_seed = target
            .selected
            .file_path
            .clone()
            .or_else(|| symbol.node.file_path.clone())
            .map(|path| workflow_seed_path(&project_root, &path));
        let affected = affected_seed
            .as_ref()
            .map(|path| {
                self.affected_analysis(AffectedAnalysisRequest {
                    changed_paths: vec![path.clone()],
                    change_records: vec![AffectedChangeRecordDto {
                        path: path.clone(),
                        kind: AffectedChangeKindDto::Unknown,
                        status: "symbol_file".to_string(),
                        previous_path: None,
                    }],
                    depth: Some(depth),
                    filter: None,
                })
            })
            .transpose()?;
        let (direct_callers, transitive_callers) = workflow_callers(&trail);
        let likely_tests = workflow_tests(affected.as_ref());
        let unknowns = workflow_unknowns(
            affected.as_ref(),
            &trail,
            &direct_callers,
            &transitive_callers,
            &likely_tests,
            affected_seed.as_deref(),
            max_nodes,
        );
        let next_commands = workflow_next_commands(
            &project_root,
            &target.selected.node_id,
            affected_seed.as_deref(),
            depth,
            max_nodes,
            request.mode,
            include_tests,
        );
        let occurrences = workflow_occurrences(self, &target);

        Ok(SymbolWorkflowOutcome::Complete(Box::new(
            SymbolWorkflowResponse {
                workflow: request.mode.label(),
                project_root: project_root.to_string_lossy().to_string(),
                resolution: SymbolWorkflowResolution {
                    target,
                    occurrences,
                },
                symbol,
                direct_callers,
                transitive_callers,
                impacted_files: workflow_impacted_files(affected.as_ref()),
                impacted_routes: workflow_routes(affected.as_ref()),
                likely_tests,
                caps: SymbolWorkflowCaps {
                    caller_depth: depth,
                    caller_max_nodes: max_nodes,
                    affected_depth: depth,
                    impacted_symbols_cap: IMPACTED_SYMBOLS_CAP,
                    impacted_routes_cap: IMPACTED_ROUTES_CAP,
                    affected_seed: affected_seed.unwrap_or_else(|| {
                        "none: selected symbol has no indexed file path".to_string()
                    }),
                },
                unknowns,
                next_commands,
                affected,
                trail,
            },
        )))
    }
}

fn workflow_occurrences(
    controller: &AppController,
    target: &ResolvedTarget,
) -> HashMap<NodeId, Vec<SourceOccurrenceDto>> {
    let mut seen = HashSet::new();
    let mut by_node = HashMap::new();
    for hit in std::iter::once(&target.selected).chain(target.alternatives.iter()) {
        if hit.is_text_match() || !hit.resolvable || !seen.insert(hit.node_id.clone()) {
            continue;
        }
        if let Ok(occurrences) = controller.node_occurrences(NodeOccurrencesRequest {
            id: hit.node_id.clone(),
        }) {
            by_node.insert(hit.node_id.clone(), occurrences);
        }
    }
    by_node
}

fn workflow_callers(trail: &TrailContextDto) -> (Vec<SymbolWorkflowNode>, Vec<SymbolWorkflowNode>) {
    let nodes = trail
        .trail
        .nodes
        .iter()
        .map(|node| (node.id.clone(), node))
        .collect::<HashMap<_, _>>();
    let mut incoming = HashMap::<NodeId, Vec<NodeId>>::new();
    for edge in &trail.trail.edges {
        if edge.kind == EdgeKind::CALL && edge.source != edge.target {
            incoming
                .entry(edge.target.clone())
                .or_default()
                .push(edge.source.clone());
        }
    }
    let mut direct_order = incoming.get(&trail.focus.id).cloned().unwrap_or_default();
    let mut seen_direct = HashSet::new();
    direct_order.retain(|id| seen_direct.insert(id.clone()));
    let direct_ids = direct_order.iter().cloned().collect::<HashSet<_>>();
    let mut direct = direct_order
        .iter()
        .filter_map(|id| nodes.get(id).map(|node| workflow_node(node)))
        .collect::<Vec<_>>();
    direct.sort_by(|left, right| {
        left.file_path
            .cmp(&right.file_path)
            .then(left.display_name.cmp(&right.display_name))
    });

    let mut call_ancestors = HashSet::new();
    let mut stack = vec![trail.focus.id.clone()];
    while let Some(target) = stack.pop() {
        for source in incoming.get(&target).into_iter().flatten() {
            if call_ancestors.insert(source.clone()) {
                stack.push(source.clone());
            }
        }
    }
    let mut transitive = trail
        .trail
        .nodes
        .iter()
        .filter(|node| {
            node.id != trail.focus.id
                && node.depth > 1
                && !direct_ids.contains(&node.id)
                && call_ancestors.contains(&node.id)
        })
        .map(workflow_node)
        .collect::<Vec<_>>();
    transitive.sort_by(|left, right| {
        left.depth
            .cmp(&right.depth)
            .then(left.file_path.cmp(&right.file_path))
            .then(left.display_name.cmp(&right.display_name))
    });
    transitive.truncate(TRANSITIVE_CALLERS_CAP);
    (direct, transitive)
}

fn workflow_node(node: &GraphNodeDto) -> SymbolWorkflowNode {
    SymbolWorkflowNode {
        node_id: node.id.clone(),
        display_name: node
            .qualified_name
            .clone()
            .unwrap_or_else(|| node.label.clone()),
        kind: format!("{:?}", node.kind).to_ascii_lowercase(),
        file_path: node.file_path.clone(),
        depth: node.depth,
    }
}

fn workflow_seed_path(project_root: &Path, path: &str) -> String {
    let clean_path = path
        .strip_prefix(r"\\?\")
        .unwrap_or(path)
        .replace('\\', "/");
    codestory_workspace::workspace_relative_path(project_root, Path::new(&clean_path))
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
        .unwrap_or(clean_path)
}

fn workflow_impacted_files(affected: Option<&AffectedAnalysisDto>) -> Vec<String> {
    let mut files = BTreeSet::new();
    if let Some(affected) = affected {
        files.extend(affected.matched_files.iter().map(|file| file.path.clone()));
        files.extend(
            affected
                .impacted_symbols
                .iter()
                .filter_map(|symbol| symbol.file_path.clone()),
        );
        files.extend(
            affected
                .impacted_routes
                .iter()
                .filter_map(|route| route.file_path.clone()),
        );
        files.extend(
            affected
                .impacted_routes
                .iter()
                .filter_map(|route| route.route.source_file.clone()),
        );
        files.extend(affected.impacted_tests.iter().map(|test| test.path.clone()));
    }
    files.into_iter().collect()
}

fn workflow_routes(affected: Option<&AffectedAnalysisDto>) -> Vec<SymbolWorkflowRoute> {
    affected
        .into_iter()
        .flat_map(|affected| affected.impacted_routes.iter())
        .map(|route| SymbolWorkflowRoute {
            display_name: route.display_name.clone(),
            method: route.route.method.clone(),
            path: route.route.path.clone(),
            file_path: route
                .file_path
                .clone()
                .or_else(|| route.route.source_file.clone()),
            line: route.line.or(route.route.line),
            confidence: route.confidence.clone(),
            reason: route.reason.clone(),
        })
        .collect()
}

fn workflow_tests(affected: Option<&AffectedAnalysisDto>) -> Vec<SymbolWorkflowTest> {
    affected
        .into_iter()
        .flat_map(|affected| affected.impacted_tests.iter())
        .map(|test| SymbolWorkflowTest {
            path: test.path.clone(),
            reason: test.reason.clone(),
            confidence: test.confidence.clone(),
            graph_depth: test.graph_depth,
            impacted_symbol_count: test.impacted_symbol_count,
        })
        .collect()
}

fn workflow_unknowns(
    affected: Option<&AffectedAnalysisDto>,
    trail: &TrailContextDto,
    direct_callers: &[SymbolWorkflowNode],
    transitive_callers: &[SymbolWorkflowNode],
    likely_tests: &[SymbolWorkflowTest],
    affected_seed: Option<&str>,
    max_nodes: u32,
) -> Vec<String> {
    let mut unknowns = vec![
        "affected files/routes/tests are seeded from the selected symbol's file, not a symbol-level change slice"
            .to_string(),
    ];
    if affected_seed.is_none() {
        unknowns.push(
            "selected symbol has no indexed file path; affected analysis was skipped".to_string(),
        );
    }
    if direct_callers.is_empty() {
        unknowns.push("no direct callers found in the incoming trail".to_string());
    }
    if transitive_callers.is_empty() {
        unknowns.push("no transitive callers found inside the caller depth cap".to_string());
    }
    if likely_tests.is_empty() {
        unknowns.push("no test-like file reached by the affected graph walk".to_string());
    }
    if trail.trail.truncated {
        unknowns.push(format!(
            "caller trail truncated at max_nodes={max_nodes}; rerun with a narrower symbol or higher cap"
        ));
    }
    if let Some(affected) = affected {
        unknowns.extend(affected.blind_spots.iter().cloned());
    }
    unknowns.sort();
    unknowns.dedup();
    unknowns
}

fn workflow_next_commands(
    project_root: &Path,
    node_id: &NodeId,
    affected_seed: Option<&str>,
    depth: u32,
    max_nodes: u32,
    mode: SymbolWorkflowMode,
    include_tests: bool,
) -> Vec<String> {
    let project = quote_command_path(project_root);
    let id = quote_command_value(&node_id.0);
    let caller_scope_flag = if include_tests {
        " --include-tests"
    } else {
        ""
    };
    let mut commands = vec![
        format!("codestory-cli symbol --project {project} --id {id}"),
        format!(
            "codestory-cli callers --project {project} --id {id} --depth {depth} --max-nodes {max_nodes}{caller_scope_flag}"
        ),
    ];
    if let Some(path) = affected_seed {
        commands.push(format!(
            "codestory-cli affected --project {project} {} --depth {depth}",
            quote_command_value(path)
        ));
    }
    let paired = match mode {
        SymbolWorkflowMode::Impact => "test-map",
        SymbolWorkflowMode::TestMap => "impact",
    };
    commands.push(format!(
        "codestory-cli {paired} --project {project} --id {id} --depth {depth} --max-nodes {max_nodes}{caller_scope_flag}"
    ));
    commands
}

fn quote_command_path(path: &Path) -> String {
    quote_command_argument_value(&clean_path_string(&path.to_string_lossy()))
}

fn quote_command_value(value: &str) -> String {
    shell_single_quoted_value(value)
}

fn quote_command_argument_value(value: &str) -> String {
    if value.chars().any(|ch| matches!(ch, '$' | '`' | '\'' | '"')) {
        quote_command_value(value)
    } else {
        format!("\"{}\"", value.replace('"', "\\\""))
    }
}

#[cfg(windows)]
fn shell_single_quoted_value(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

#[cfg(not(windows))]
fn shell_single_quoted_value(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn clean_path_string(path: &str) -> String {
    let mut normalized = path.replace('\\', "/");
    if let Some(stripped) = normalized.strip_prefix("//?/UNC/") {
        normalized = format!("//{stripped}");
    } else if normalized.starts_with("//?/") {
        normalized = normalized[4..].to_string();
    }
    normalized
}

#[cfg(test)]
mod tests {
    use super::*;
    use codestory_contracts::api::{EdgeId, GraphEdgeDto, GraphResponse, NodeDetailsDto, NodeKind};

    fn node(id: &str, label: &str, depth: u32) -> GraphNodeDto {
        GraphNodeDto {
            id: NodeId(id.to_string()),
            label: label.to_string(),
            kind: NodeKind::FUNCTION,
            depth,
            label_policy: None,
            badge_visible_members: None,
            badge_total_members: None,
            merged_symbol_examples: Vec::new(),
            file_path: Some(format!("src/{label}.rs")),
            qualified_name: None,
            member_access: None,
        }
    }

    #[test]
    fn callers_follow_only_call_paths_in_one_traversal() {
        let trail = TrailContextDto {
            focus: NodeDetailsDto {
                id: NodeId("focus".to_string()),
                kind: NodeKind::FUNCTION,
                display_name: "Focus".to_string(),
                serialized_name: "Focus".to_string(),
                qualified_name: None,
                canonical_id: None,
                file_path: Some("src/Focus.rs".to_string()),
                start_line: Some(1),
                start_col: Some(0),
                end_line: Some(1),
                end_col: Some(5),
                member_access: None,
                route_endpoint: None,
            },
            trail: GraphResponse {
                center_id: NodeId("focus".to_string()),
                nodes: vec![
                    node("focus", "Focus", 0),
                    node("direct-b", "Direct", 1),
                    node("direct-a", "Direct", 1),
                    node("transitive-b", "Transitive", 2),
                    node("transitive-a", "Transitive", 2),
                    node("reference", "Reference", 2),
                ],
                edges: vec![
                    GraphEdgeDto {
                        id: EdgeId("direct-b-focus".to_string()),
                        source: NodeId("direct-b".to_string()),
                        target: NodeId("focus".to_string()),
                        kind: EdgeKind::CALL,
                        confidence: None,
                        certainty: None,
                        callsite_identity: None,
                        candidate_targets: Vec::new(),
                    },
                    GraphEdgeDto {
                        id: EdgeId("direct-a-focus".to_string()),
                        source: NodeId("direct-a".to_string()),
                        target: NodeId("focus".to_string()),
                        kind: EdgeKind::CALL,
                        confidence: None,
                        certainty: None,
                        callsite_identity: None,
                        candidate_targets: Vec::new(),
                    },
                    GraphEdgeDto {
                        id: EdgeId("transitive-b-direct".to_string()),
                        source: NodeId("transitive-b".to_string()),
                        target: NodeId("direct-b".to_string()),
                        kind: EdgeKind::CALL,
                        confidence: None,
                        certainty: None,
                        callsite_identity: None,
                        candidate_targets: Vec::new(),
                    },
                    GraphEdgeDto {
                        id: EdgeId("transitive-a-direct".to_string()),
                        source: NodeId("transitive-a".to_string()),
                        target: NodeId("direct-a".to_string()),
                        kind: EdgeKind::CALL,
                        confidence: None,
                        certainty: None,
                        callsite_identity: None,
                        candidate_targets: Vec::new(),
                    },
                    GraphEdgeDto {
                        id: EdgeId("reference-direct".to_string()),
                        source: NodeId("reference".to_string()),
                        target: NodeId("direct-b".to_string()),
                        kind: EdgeKind::USAGE,
                        confidence: None,
                        certainty: None,
                        callsite_identity: None,
                        candidate_targets: Vec::new(),
                    },
                ],
                truncated: false,
                omitted_edge_count: 0,
                canonical_layout: None,
            },
            story: None,
        };

        let (direct, transitive) = workflow_callers(&trail);

        assert_eq!(
            direct
                .iter()
                .map(|node| node.node_id.0.as_str())
                .collect::<Vec<_>>(),
            vec!["direct-b", "direct-a"]
        );
        assert_eq!(
            transitive
                .iter()
                .map(|node| node.node_id.0.as_str())
                .collect::<Vec<_>>(),
            vec!["transitive-b", "transitive-a"]
        );
    }

    #[test]
    fn test_map_next_action_keeps_test_scope() {
        let commands = workflow_next_commands(
            Path::new("C:/repo"),
            &NodeId("focus".to_string()),
            None,
            3,
            20,
            SymbolWorkflowMode::Impact,
            true,
        );
        assert!(commands.iter().all(|command| {
            !command.contains(" callers ") && !command.contains(" test-map ")
                || command.contains("--include-tests")
        }));
    }
}
