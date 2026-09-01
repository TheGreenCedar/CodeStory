//! Repository-derived evidence planning (Stage B).
//!
//! After runtime retrieves seed citations and a bounded typed relationship
//! graph, this module selects material nodes/edges from repository structure
//! alone. It never invents domain stage taxonomies from prompt vocabulary.

use crate::packet_required_probes::packet_prompt_explicit_source_path_queries;
use crate::text::exact_symbol_query_terms;
use codestory_contracts::api::{
    AgentCitationDto, EdgeId, EdgeKind, GraphEdgeDto, NodeId, PacketTaskClassDto,
};
use std::collections::{BTreeMap, BTreeSet, HashSet, VecDeque};

/// Frozen planner constants after visible metamorphic suite green.
/// Do not mutate without a new Phase-4 suite run and holdout invalidation.
pub const DEFAULT_REPOSITORY_EVIDENCE_LIMITS: RepositoryEvidenceLimits = RepositoryEvidenceLimits {
    max_seed_nodes: 12,
    max_candidate_nodes: 256,
    max_candidate_edges: 512,
    max_depth: 4,
    max_relation_paths: 32,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RepositoryEvidenceLimits {
    pub max_seed_nodes: usize,
    pub max_candidate_nodes: usize,
    pub max_candidate_edges: usize,
    pub max_depth: usize,
    pub max_relation_paths: usize,
}

impl Default for RepositoryEvidenceLimits {
    fn default() -> Self {
        DEFAULT_REPOSITORY_EVIDENCE_LIMITS
    }
}

#[derive(Debug, Clone)]
pub struct RepositoryEvidenceInput<'a> {
    pub question: &'a str,
    pub task_class: PacketTaskClassDto,
    pub seeds: &'a [AgentCitationDto],
    pub relations: &'a [GraphEdgeDto],
}

/// A repository-grounded objective. Identifiers refer only to graph entities.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryEvidenceObjective {
    pub kind: RepositoryEvidenceObjectiveKind,
    pub node_ids: Vec<NodeId>,
    pub edge_ids: Vec<EdgeId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepositoryEvidenceObjectiveKind {
    /// Explicit prompt anchor resolved to a repository node.
    ResolvedAnchor,
    /// Shortest retained relationship path connecting distinct anchors.
    RelationPath,
    /// Implementation / membership relationship behind a selected anchor.
    ImplementationRelation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryEvidenceGap {
    pub kind: RepositoryEvidenceGapKind,
    pub detail: String,
    pub node_ids: Vec<NodeId>,
    pub edge_ids: Vec<EdgeId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepositoryEvidenceGapKind {
    /// No seed citations resolved from the prompt.
    UnresolvedAnchors,
    /// Seeds exist but no typed relationship supports the requested path.
    MissingRelation,
    /// Search truncated by planner limits; continuation may name remainder.
    TruncatedSearch,
    /// Ambiguous or incomplete graph; do not assert absence.
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RepositoryEvidencePlan {
    pub material_node_ids: Vec<NodeId>,
    pub material_edge_ids: Vec<EdgeId>,
    pub objectives: Vec<RepositoryEvidenceObjective>,
    pub uncovered: Vec<RepositoryEvidenceGap>,
}

pub fn build_repository_evidence_plan(
    input: RepositoryEvidenceInput<'_>,
    limits: RepositoryEvidenceLimits,
) -> RepositoryEvidencePlan {
    let mut plan = RepositoryEvidencePlan::default();

    let seed_nodes =
        resolve_prompt_anchor_nodes(input.question, input.seeds, limits.max_seed_nodes);
    if seed_nodes.is_empty() {
        plan.uncovered.push(RepositoryEvidenceGap {
            kind: RepositoryEvidenceGapKind::UnresolvedAnchors,
            detail: "no repository seeds resolved from the prompt".into(),
            node_ids: Vec::new(),
            edge_ids: Vec::new(),
        });
        return plan;
    }

    for node_id in &seed_nodes {
        plan.objectives.push(RepositoryEvidenceObjective {
            kind: RepositoryEvidenceObjectiveKind::ResolvedAnchor,
            node_ids: vec![node_id.clone()],
            edge_ids: Vec::new(),
        });
        push_unique_node(&mut plan.material_node_ids, node_id.clone());
    }

    let preferred = preferred_edge_kinds(input.task_class);
    let adjacency = build_adjacency(input.relations, &preferred, limits.max_candidate_edges);

    let mut path_count = 0usize;
    let mut truncated = adjacency.truncated;
    for (seed_index, start) in seed_nodes.iter().enumerate() {
        for end in seed_nodes.iter().skip(seed_index + 1) {
            if path_count >= limits.max_relation_paths {
                truncated = true;
                break;
            }
            match shortest_path(
                start,
                end,
                &adjacency.edges,
                limits.max_depth,
                limits.max_candidate_nodes,
            ) {
                PathSearch::Found(path) => {
                    path_count += 1;
                    for node in &path.nodes {
                        push_unique_node(&mut plan.material_node_ids, node.clone());
                    }
                    for edge in &path.edges {
                        push_unique_edge(&mut plan.material_edge_ids, edge.clone());
                    }
                    plan.objectives.push(RepositoryEvidenceObjective {
                        kind: RepositoryEvidenceObjectiveKind::RelationPath,
                        node_ids: path.nodes,
                        edge_ids: path.edges,
                    });
                }
                PathSearch::Exhausted if adjacency.truncated => {
                    // The graph itself was cut, so an exhausted search over it
                    // still proves nothing about the repository.
                    plan.uncovered.push(RepositoryEvidenceGap {
                        kind: RepositoryEvidenceGapKind::TruncatedSearch,
                        detail: "relationship graph truncated before the anchors were connected"
                            .into(),
                        node_ids: vec![start.clone(), end.clone()],
                        edge_ids: Vec::new(),
                    });
                }
                PathSearch::Exhausted => {
                    // Missing path between two seeds is unknown, not absence.
                    plan.uncovered.push(RepositoryEvidenceGap {
                        kind: RepositoryEvidenceGapKind::MissingRelation,
                        detail: "no retained typed path between resolved anchors".into(),
                        node_ids: vec![start.clone(), end.clone()],
                        edge_ids: Vec::new(),
                    });
                }
                PathSearch::Truncated => {
                    truncated = true;
                    plan.uncovered.push(RepositoryEvidenceGap {
                        kind: RepositoryEvidenceGapKind::TruncatedSearch,
                        detail: "relationship search hit the depth or node budget".into(),
                        node_ids: vec![start.clone(), end.clone()],
                        edge_ids: Vec::new(),
                    });
                }
            }
        }
        if truncated && path_count >= limits.max_relation_paths {
            break;
        }
    }

    // Implementation relations incident to seeds (membership / override / etc.).
    for edge in input
        .relations
        .iter()
        .filter(|edge| is_implementation_kind(edge.kind, input.task_class))
        .take(limits.max_candidate_edges)
    {
        let touches_seed = seed_nodes
            .iter()
            .any(|n| n == &edge.source || n == &edge.target);
        if !touches_seed {
            continue;
        }
        push_unique_node(&mut plan.material_node_ids, edge.source.clone());
        push_unique_node(&mut plan.material_node_ids, edge.target.clone());
        push_unique_edge(&mut plan.material_edge_ids, edge.id.clone());
        plan.objectives.push(RepositoryEvidenceObjective {
            kind: RepositoryEvidenceObjectiveKind::ImplementationRelation,
            node_ids: vec![edge.source.clone(), edge.target.clone()],
            edge_ids: vec![edge.id.clone()],
        });
        if plan.objectives.len() > limits.max_relation_paths.saturating_mul(2) {
            truncated = true;
            break;
        }
    }

    if seed_nodes.len() >= 2
        && plan
            .objectives
            .iter()
            .all(|o| o.kind == RepositoryEvidenceObjectiveKind::ResolvedAnchor)
    {
        plan.uncovered.push(RepositoryEvidenceGap {
            kind: RepositoryEvidenceGapKind::Unknown,
            detail: "anchors resolved but no repository relationship selected".into(),
            node_ids: seed_nodes.clone(),
            edge_ids: Vec::new(),
        });
    }

    if truncated {
        plan.uncovered.push(RepositoryEvidenceGap {
            kind: RepositoryEvidenceGapKind::TruncatedSearch,
            detail: "repository evidence search hit planner limits".into(),
            node_ids: Vec::new(),
            edge_ids: Vec::new(),
        });
    }

    // Domain vocabulary in the question never creates objectives by itself.
    // Objectives exist only from seeds/relations above.
    let _ = input.task_class;
    plan
}

/// Anchors are the seeds whose path, display name, or node identity the prompt
/// actually names, ordered by where the prompt names them.
///
/// A prompt that names no repository identity resolves no anchors. Retrieval
/// order is not evidence that the caller asked about a symbol, so falling back
/// to it would let a broad prompt protect whatever happened to rank first and
/// demand a continuation for it.
///
/// Prompt order, not seed order, also fixes the direction of every pair the
/// caller asked about: "trace A calling B" searches A to B whatever order
/// retrieval returned them in.
fn resolve_prompt_anchor_nodes(
    question: &str,
    seeds: &[AgentCitationDto],
    max_seed_nodes: usize,
) -> Vec<NodeId> {
    let identities = prompt_path_and_symbol_identities(question);
    let lowered_question = question.to_lowercase();
    let mut anchors: Vec<(usize, usize, NodeId)> = Vec::new();
    let mut seen = BTreeSet::new();
    for (seed_index, seed) in seeds.iter().enumerate() {
        let Some(offset) =
            seed_prompt_mention_offset(seed, question, &lowered_question, &identities)
        else {
            continue;
        };
        if seen.insert(seed.node_id.clone()) {
            anchors.push((offset, seed_index, seed.node_id.clone()));
        }
    }
    anchors.sort_by(|left, right| left.0.cmp(&right.0).then(left.1.cmp(&right.1)));
    anchors.truncate(max_seed_nodes);
    anchors.into_iter().map(|(_, _, node)| node).collect()
}

/// Where the prompt first names this seed, or `None` when it never does.
fn seed_prompt_mention_offset(
    seed: &AgentCitationDto,
    question: &str,
    lowered_question: &str,
    identities: &[String],
) -> Option<usize> {
    let mut offset: Option<usize> = None;
    let mut record = |candidate: Option<usize>| {
        if let Some(candidate) = candidate {
            offset = Some(offset.map_or(candidate, |best: usize| best.min(candidate)));
        }
    };

    if !seed.display_name.is_empty() {
        record(question.find(&seed.display_name));
    }
    if let Some(path) = seed.file_path.as_deref().filter(|path| !path.is_empty()) {
        record(question.find(path));
    }
    for identity in identities {
        if !seed_matches_prompt_identity(seed, std::slice::from_ref(identity)) {
            continue;
        }
        // An identity the prompt produced is normally locatable in it. When
        // casing or trailing punctuation hides it, the seed still resolves; it
        // just carries no position of its own.
        record(Some(
            lowered_question
                .find(&identity.to_lowercase())
                .unwrap_or(question.len()),
        ));
    }
    offset
}

fn prompt_path_and_symbol_identities(question: &str) -> Vec<String> {
    let mut identities = packet_prompt_explicit_source_path_queries(question);
    let mut seen: HashSet<String> = identities.iter().cloned().collect();
    for term in exact_symbol_query_terms(question) {
        if seen.insert(term.clone()) {
            identities.push(term);
        }
    }
    identities
}

fn seed_matches_prompt_identity(seed: &AgentCitationDto, identities: &[String]) -> bool {
    identities.iter().any(|identity| {
        let normalized = identity.trim_end_matches(['?', '!', '.', ',', ';']);
        seed_path_matches_identity(seed.file_path.as_deref(), identity)
            || (!normalized.is_empty()
                && normalized != identity
                && seed_path_matches_identity(seed.file_path.as_deref(), normalized))
            || seed_symbol_matches_identity(&seed.display_name, identity)
            || (!normalized.is_empty()
                && normalized != identity
                && seed_symbol_matches_identity(&seed.display_name, normalized))
            || seed.node_id.0 == *identity
            || (!normalized.is_empty() && seed.node_id.0 == normalized)
    })
}

fn seed_path_matches_identity(file_path: Option<&str>, identity: &str) -> bool {
    let Some(path) = file_path.map(str::trim).filter(|path| !path.is_empty()) else {
        return false;
    };
    let identity = identity.trim();
    if identity.is_empty() {
        return false;
    }
    path == identity
        || path.ends_with(identity)
        || identity.ends_with(path)
        || path_file_name(path) == Some(identity)
        || path_file_name(identity) == Some(path)
}

fn path_file_name(path: &str) -> Option<&str> {
    path.rsplit(['/', '\\'])
        .next()
        .filter(|name| !name.is_empty() && *name != path)
}

fn seed_symbol_matches_identity(display_name: &str, identity: &str) -> bool {
    let display = display_name.trim();
    let identity = identity.trim();
    if display.is_empty() || identity.is_empty() {
        return false;
    }
    if display == identity {
        return true;
    }
    let display_segments = identity_segments(display);
    let identity_segments = identity_segments(identity);
    if identity_segments.is_empty() || display_segments.len() < identity_segments.len() {
        return false;
    }
    let suffix_start = display_segments.len() - identity_segments.len();
    display_segments[suffix_start..] == identity_segments[..]
        || display_segments[..identity_segments.len()] == identity_segments[..]
}

fn identity_segments(value: &str) -> Vec<&str> {
    value
        .split([':', '.', '#', '/', '\\'])
        .map(str::trim)
        .map(|segment| segment.strip_suffix("()").unwrap_or(segment))
        .filter(|segment| !segment.is_empty())
        .collect()
}

fn preferred_edge_kinds(task_class: PacketTaskClassDto) -> HashSet<EdgeKind> {
    let kinds: &[EdgeKind] = match task_class {
        PacketTaskClassDto::ArchitectureExplanation => &[
            EdgeKind::CALL,
            EdgeKind::MEMBER,
            EdgeKind::INHERITANCE,
            EdgeKind::OVERRIDE,
            EdgeKind::IMPORT,
            EdgeKind::INCLUDE,
        ],
        PacketTaskClassDto::BugLocalization => &[
            EdgeKind::CALL,
            EdgeKind::USAGE,
            EdgeKind::TYPE_USAGE,
            EdgeKind::MEMBER,
            EdgeKind::OVERRIDE,
        ],
        PacketTaskClassDto::ChangeImpact => &[
            EdgeKind::CALL,
            EdgeKind::USAGE,
            EdgeKind::TYPE_USAGE,
            EdgeKind::IMPORT,
            EdgeKind::INCLUDE,
        ],
        PacketTaskClassDto::RouteTracing => &[EdgeKind::CALL],
        PacketTaskClassDto::SymbolOwnership => {
            &[EdgeKind::MEMBER, EdgeKind::OVERRIDE, EdgeKind::INHERITANCE]
        }
        PacketTaskClassDto::DataFlow => &[
            EdgeKind::CALL,
            EdgeKind::USAGE,
            EdgeKind::TYPE_USAGE,
            EdgeKind::MEMBER,
        ],
        PacketTaskClassDto::EditPlanning => &[
            EdgeKind::CALL,
            EdgeKind::USAGE,
            EdgeKind::TYPE_USAGE,
            EdgeKind::MEMBER,
            EdgeKind::OVERRIDE,
            EdgeKind::INHERITANCE,
            EdgeKind::IMPORT,
            EdgeKind::INCLUDE,
        ],
    };
    kinds.iter().copied().collect()
}

/// Membership and inheritance relations are symmetric facts about a seed: which
/// type owns it, what it overrides. They hold whichever way the incident edge is
/// stored, so an undirected incidence pass may select them.
///
/// `CALL` is not such a fact. It is exactly the directional claim the path
/// search adjudicates, so selecting it by incidence would protect the reverse
/// edge that the directed search just rejected.
fn is_implementation_kind(kind: EdgeKind, task_class: PacketTaskClassDto) -> bool {
    preferred_edge_kinds(task_class).contains(&kind)
        && matches!(
            kind,
            EdgeKind::MEMBER | EdgeKind::OVERRIDE | EdgeKind::INHERITANCE
        )
}

#[derive(Debug, Clone)]
struct AdjEdge {
    to: NodeId,
    edge_id: EdgeId,
}

/// Directed kinds keep source→target only. Reverse insertion would let a stored
/// `B→A` CALL satisfy an `A→B` route search. No current `EdgeKind` is undirected.
fn edge_kind_is_undirected(_kind: EdgeKind) -> bool {
    false
}

struct Adjacency {
    edges: BTreeMap<NodeId, Vec<AdjEdge>>,
    /// The edge budget dropped preferred edges, so any failed search over this
    /// graph is truncated rather than exhaustive.
    truncated: bool,
}

/// Spend the edge budget on edges this task class can actually traverse. Taking
/// the first `max_edges` relations regardless of kind let unusable edges consume
/// the budget and made a reachable path look absent.
fn build_adjacency(
    relations: &[GraphEdgeDto],
    preferred: &HashSet<EdgeKind>,
    max_edges: usize,
) -> Adjacency {
    let mut adj: BTreeMap<NodeId, Vec<AdjEdge>> = BTreeMap::new();
    let mut admitted = 0usize;
    let mut truncated = false;
    for edge in relations
        .iter()
        .filter(|edge| preferred.contains(&edge.kind))
    {
        if admitted >= max_edges {
            truncated = true;
            break;
        }
        admitted += 1;
        adj.entry(edge.source.clone()).or_default().push(AdjEdge {
            to: edge.target.clone(),
            edge_id: edge.id.clone(),
        });
        if edge_kind_is_undirected(edge.kind) {
            adj.entry(edge.target.clone()).or_default().push(AdjEdge {
                to: edge.source.clone(),
                edge_id: edge.id.clone(),
            });
        }
    }
    Adjacency {
        edges: adj,
        truncated,
    }
}

#[derive(Debug, Clone)]
struct PathResult {
    nodes: Vec<NodeId>,
    edges: Vec<EdgeId>,
}

/// Why a directed search ended. A search stopped by a planner limit has not
/// looked everywhere, so it cannot report that no path exists.
enum PathSearch {
    Found(PathResult),
    /// The whole reachable subgraph was searched and holds no path.
    Exhausted,
    /// A depth or node budget cut the search short.
    Truncated,
}

fn shortest_path(
    start: &NodeId,
    end: &NodeId,
    adjacency: &BTreeMap<NodeId, Vec<AdjEdge>>,
    max_depth: usize,
    max_nodes: usize,
) -> PathSearch {
    if start == end {
        return PathSearch::Found(PathResult {
            nodes: vec![start.clone()],
            edges: Vec::new(),
        });
    }
    let mut queue = VecDeque::new();
    let mut visited = BTreeSet::new();
    // pred: node -> (previous node, edge used)
    let mut pred: BTreeMap<NodeId, (NodeId, EdgeId)> = BTreeMap::new();
    let mut truncated = false;
    queue.push_back((start.clone(), 0usize));
    visited.insert(start.clone());
    while let Some((node, depth)) = queue.pop_front() {
        if depth >= max_depth {
            // Frontier nodes still had unexplored edges when the depth budget
            // ran out, so the search is incomplete rather than exhaustive.
            if adjacency.get(&node).is_some_and(|edges| !edges.is_empty()) {
                truncated = true;
            }
            continue;
        }
        for edge in adjacency.get(&node).into_iter().flatten() {
            if !visited.insert(edge.to.clone()) {
                continue;
            }
            pred.insert(edge.to.clone(), (node.clone(), edge.edge_id.clone()));
            if &edge.to == end {
                return PathSearch::Found(reconstruct_path(start, end, &pred));
            }
            if visited.len() >= max_nodes {
                return PathSearch::Truncated;
            }
            queue.push_back((edge.to.clone(), depth + 1));
        }
    }
    if truncated {
        PathSearch::Truncated
    } else {
        PathSearch::Exhausted
    }
}

fn reconstruct_path(
    start: &NodeId,
    end: &NodeId,
    pred: &BTreeMap<NodeId, (NodeId, EdgeId)>,
) -> PathResult {
    let mut nodes = vec![end.clone()];
    let mut edges = Vec::new();
    let mut current = end.clone();
    while &current != start {
        let (prev, edge_id) = pred.get(&current).expect("path predecessor");
        edges.push(edge_id.clone());
        nodes.push(prev.clone());
        current = prev.clone();
    }
    nodes.reverse();
    edges.reverse();
    PathResult { nodes, edges }
}

fn push_unique_node(nodes: &mut Vec<NodeId>, node: NodeId) {
    if !nodes.iter().any(|n| n == &node) {
        nodes.push(node);
    }
}

fn push_unique_edge(edges: &mut Vec<EdgeId>, edge: EdgeId) {
    if !edges.iter().any(|e| e == &edge) {
        edges.push(edge);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codestory_contracts::api::{NodeKind, SearchHitOrigin};

    fn citation(id: &str, name: &str) -> AgentCitationDto {
        AgentCitationDto {
            node_id: NodeId(id.into()),
            display_name: name.into(),
            kind: NodeKind::FUNCTION,
            file_path: Some(format!("src/{name}.rs")),
            line: Some(1),
            score: 1.0,
            origin: SearchHitOrigin::IndexedSymbol,
            target: None,
            resolvable: true,
            subgraph_id: None,
            evidence_edge_ids: Vec::new(),
            retrieval_score_breakdown: None,
            evidence_tier: None,
            evidence_producer: None,
            resolution_status: None,
            loss_reason: None,
            coverage_role: None,
            eligible_for_sufficiency: None,
            source_excerpt: None,
        }
    }

    fn call_edge(id: &str, source: &str, target: &str) -> GraphEdgeDto {
        GraphEdgeDto {
            id: EdgeId(id.into()),
            source: NodeId(source.into()),
            target: NodeId(target.into()),
            kind: EdgeKind::CALL,
            confidence: Some(1.0),
            certainty: Some("certain".into()),
            callsite_identity: None,
            candidate_targets: Vec::new(),
        }
    }

    #[test]
    fn empty_graph_yields_unresolved_or_unknown_gaps() {
        let plan = build_repository_evidence_plan(
            RepositoryEvidenceInput {
                question: "Explain the client cache mapper animation flow",
                task_class: PacketTaskClassDto::ArchitectureExplanation,
                seeds: &[],
                relations: &[],
            },
            RepositoryEvidenceLimits::default(),
        );
        assert!(plan.objectives.is_empty());
        assert!(plan.material_node_ids.is_empty());
        assert!(
            plan.uncovered
                .iter()
                .any(|g| g.kind == RepositoryEvidenceGapKind::UnresolvedAnchors)
        );
    }

    #[test]
    fn two_seeds_with_call_edge_select_material_path() {
        let seeds = [citation("n1", "foo"), citation("n2", "bar")];
        let relations = [call_edge("e1", "n1", "n2")];
        let plan = build_repository_evidence_plan(
            RepositoryEvidenceInput {
                question: "Trace foo calling bar",
                task_class: PacketTaskClassDto::RouteTracing,
                seeds: &seeds,
                relations: &relations,
            },
            RepositoryEvidenceLimits::default(),
        );
        assert!(
            plan.objectives
                .iter()
                .any(|o| o.kind == RepositoryEvidenceObjectiveKind::RelationPath)
        );
        assert!(plan.material_edge_ids.iter().any(|e| e.0 == "e1"));
        assert!(plan.material_node_ids.iter().any(|n| n.0 == "n1"));
        assert!(plan.material_node_ids.iter().any(|n| n.0 == "n2"));
        assert!(
            !plan
                .objectives
                .iter()
                .any(|o| format!("{o:?}").contains("client_transport"))
        );
    }

    #[test]
    fn broad_prompt_naming_no_identity_materializes_nothing() {
        // The prompt names no repository identity, so the top retrieval hits are
        // leads, not anchors. Materializing them would protect arbitrary results
        // and demand a continuation to close a gap the caller never opened.
        let seeds = [citation("n1", "Client"), citation("n2", "Cache")];
        let plan = build_repository_evidence_plan(
            RepositoryEvidenceInput {
                question: "Explain how the client cache formatter mapper request animation works",
                task_class: PacketTaskClassDto::ArchitectureExplanation,
                seeds: &seeds,
                relations: &[],
            },
            RepositoryEvidenceLimits::default(),
        );
        assert!(plan.objectives.is_empty(), "{:?}", plan.objectives);
        assert!(plan.material_node_ids.is_empty());
        assert!(plan.material_edge_ids.is_empty());
        assert_eq!(
            plan.uncovered
                .iter()
                .map(|gap| gap.kind)
                .collect::<Vec<_>>(),
            vec![RepositoryEvidenceGapKind::UnresolvedAnchors],
        );
    }

    #[test]
    fn broad_prompt_does_not_protect_the_first_retrieval_hit() {
        // Same prompt, but retrieval also returned a relationship between the
        // top hits. Neither the hits nor the edge become material.
        let seeds = [citation("n1", "Client"), citation("n2", "Cache")];
        let relations = [call_edge("e1", "n1", "n2")];
        let plan = build_repository_evidence_plan(
            RepositoryEvidenceInput {
                question: "Explain how requests are handled end to end",
                task_class: PacketTaskClassDto::ArchitectureExplanation,
                seeds: &seeds,
                relations: &relations,
            },
            RepositoryEvidenceLimits::default(),
        );
        assert!(plan.material_node_ids.is_empty());
        assert!(plan.material_edge_ids.is_empty());
    }

    #[test]
    fn named_anchors_without_edges_report_a_missing_relation() {
        let seeds = [citation("n1", "Client"), citation("n2", "Cache")];
        let plan = build_repository_evidence_plan(
            RepositoryEvidenceInput {
                question: "Explain how Client uses Cache",
                task_class: PacketTaskClassDto::ArchitectureExplanation,
                seeds: &seeds,
                relations: &[],
            },
            RepositoryEvidenceLimits::default(),
        );
        assert!(
            plan.objectives
                .iter()
                .all(|o| o.kind == RepositoryEvidenceObjectiveKind::ResolvedAnchor)
        );
        assert!(plan.material_edge_ids.is_empty());
        assert!(plan.uncovered.iter().any(|g| matches!(
            g.kind,
            RepositoryEvidenceGapKind::MissingRelation | RepositoryEvidenceGapKind::Unknown
        )));
    }

    #[test]
    fn pair_direction_follows_prompt_order_not_retrieval_order() {
        // Retrieval returned Beta first; the prompt asks about Alpha calling
        // Beta, and the stored edge runs Alpha to Beta. Searching in retrieval
        // order would look for Beta to Alpha and report a missing relation.
        let seeds = [citation("b", "Beta::finish"), citation("a", "Alpha::run")];
        let relations = [call_edge("forward", "a", "b")];
        let plan = build_repository_evidence_plan(
            RepositoryEvidenceInput {
                question: "Trace Alpha::run calling Beta::finish",
                task_class: PacketTaskClassDto::RouteTracing,
                seeds: &seeds,
                relations: &relations,
            },
            RepositoryEvidenceLimits::default(),
        );
        let anchors: Vec<_> = plan
            .objectives
            .iter()
            .filter(|o| o.kind == RepositoryEvidenceObjectiveKind::ResolvedAnchor)
            .flat_map(|o| o.node_ids.iter().map(|n| n.0.as_str()))
            .collect();
        assert_eq!(anchors, vec!["a", "b"]);
        assert!(plan.material_edge_ids.iter().any(|e| e.0 == "forward"));
    }

    #[test]
    fn reversed_prompt_order_searches_the_other_direction() {
        // Mirror of the previous case: the same seeds and the same stored edge,
        // but the prompt asks the reverse question, which the graph cannot show.
        let seeds = [citation("b", "Beta::finish"), citation("a", "Alpha::run")];
        let relations = [call_edge("forward", "a", "b")];
        let plan = build_repository_evidence_plan(
            RepositoryEvidenceInput {
                question: "Trace Beta::finish calling Alpha::run",
                task_class: PacketTaskClassDto::RouteTracing,
                seeds: &seeds,
                relations: &relations,
            },
            RepositoryEvidenceLimits::default(),
        );
        let anchors: Vec<_> = plan
            .objectives
            .iter()
            .filter(|o| o.kind == RepositoryEvidenceObjectiveKind::ResolvedAnchor)
            .flat_map(|o| o.node_ids.iter().map(|n| n.0.as_str()))
            .collect();
        assert_eq!(anchors, vec!["b", "a"]);
        assert!(!plan.material_edge_ids.iter().any(|e| e.0 == "forward"));
    }

    #[test]
    fn prompt_identity_selects_anchors_not_retrieval_order() {
        let seeds = [
            citation("noise_a", "NoiseA::run"),
            citation("noise_b", "NoiseB::run"),
            citation("a", "Alpha::run"),
            citation("b", "Beta::finish"),
            citation("noise_c", "NoiseC::run"),
        ];
        let relations = [call_edge("e1", "a", "b")];
        let plan = build_repository_evidence_plan(
            RepositoryEvidenceInput {
                question: "Trace Alpha::run calling Beta::finish",
                task_class: PacketTaskClassDto::RouteTracing,
                seeds: &seeds,
                relations: &relations,
            },
            RepositoryEvidenceLimits::default(),
        );
        let anchors: Vec<_> = plan
            .objectives
            .iter()
            .filter(|o| o.kind == RepositoryEvidenceObjectiveKind::ResolvedAnchor)
            .flat_map(|o| o.node_ids.iter().map(|n| n.0.as_str()))
            .collect();
        assert_eq!(anchors, vec!["a", "b"]);
        assert!(
            !plan
                .material_node_ids
                .iter()
                .any(|n| n.0.starts_with("noise_"))
        );
        assert!(plan.material_edge_ids.iter().any(|e| e.0 == "e1"));
    }

    #[test]
    fn prompt_path_identity_selects_matching_seed_anchors() {
        let seeds = [
            citation("noise", "Other"),
            citation("left", "Alpha"),
            citation("right", "Beta"),
        ];
        // Override paths to match the prompt.
        let mut seeds = seeds;
        seeds[0].file_path = Some("src/noise.rs".into());
        seeds[1].file_path = Some("src/alpha.rs".into());
        seeds[2].file_path = Some("src/beta.rs".into());
        let plan = build_repository_evidence_plan(
            RepositoryEvidenceInput {
                question: "Inspect src/alpha.rs and src/beta.rs relationship",
                task_class: PacketTaskClassDto::ArchitectureExplanation,
                seeds: &seeds,
                relations: &[],
            },
            RepositoryEvidenceLimits::default(),
        );
        let anchors: Vec<_> = plan
            .objectives
            .iter()
            .filter(|o| o.kind == RepositoryEvidenceObjectiveKind::ResolvedAnchor)
            .flat_map(|o| o.node_ids.iter().map(|n| n.0.as_str()))
            .collect();
        assert_eq!(anchors, vec!["left", "right"]);
        assert!(!plan.material_node_ids.iter().any(|n| n.0 == "noise"));
    }

    #[test]
    fn directed_call_does_not_satisfy_reverse_route() {
        let seeds = [citation("a", "Alpha::run"), citation("b", "Beta::finish")];
        // Stored edge is B→A only; searching A→B must not invent a reverse path.
        let relations = [call_edge("reverse_only", "b", "a")];
        let plan = build_repository_evidence_plan(
            RepositoryEvidenceInput {
                question: "Trace Alpha::run calling Beta::finish",
                task_class: PacketTaskClassDto::RouteTracing,
                seeds: &seeds,
                relations: &relations,
            },
            RepositoryEvidenceLimits::default(),
        );
        assert!(
            !plan
                .objectives
                .iter()
                .any(|o| o.kind == RepositoryEvidenceObjectiveKind::RelationPath)
        );
        assert!(!plan.objectives.iter().any(|o| {
            o.kind == RepositoryEvidenceObjectiveKind::RelationPath
                && o.edge_ids.iter().any(|e| e.0 == "reverse_only")
        }));
        // The incidence pass must not readmit the edge the directed search
        // rejected: a rejected route cannot come back as material evidence.
        assert!(
            !plan.material_edge_ids.iter().any(|e| e.0 == "reverse_only"),
            "{:?}",
            plan.material_edge_ids
        );
        assert!(
            !plan
                .objectives
                .iter()
                .any(|o| o.edge_ids.iter().any(|e| e.0 == "reverse_only")),
            "{:?}",
            plan.objectives
        );
        assert!(plan.uncovered.iter().any(|g| {
            g.kind == RepositoryEvidenceGapKind::MissingRelation
                && g.node_ids.iter().any(|n| n.0 == "a")
                && g.node_ids.iter().any(|n| n.0 == "b")
        }));
    }

    #[test]
    fn depth_exhaustion_reports_truncation_not_a_missing_relation() {
        // A chain longer than max_depth. The path exists; the search cannot see
        // it, and must not claim the repository lacks one.
        let seeds = [citation("a", "Alpha::run"), citation("z", "Zeta::finish")];
        let relations = [
            call_edge("e1", "a", "m1"),
            call_edge("e2", "m1", "m2"),
            call_edge("e3", "m2", "m3"),
            call_edge("e4", "m3", "m4"),
            call_edge("e5", "m4", "z"),
        ];
        let limits = RepositoryEvidenceLimits {
            max_depth: 2,
            ..RepositoryEvidenceLimits::default()
        };
        let plan = build_repository_evidence_plan(
            RepositoryEvidenceInput {
                question: "Trace Alpha::run calling Zeta::finish",
                task_class: PacketTaskClassDto::RouteTracing,
                seeds: &seeds,
                relations: &relations,
            },
            limits,
        );
        assert!(
            plan.uncovered
                .iter()
                .any(|g| g.kind == RepositoryEvidenceGapKind::TruncatedSearch),
            "{:?}",
            plan.uncovered
        );
        assert!(
            !plan
                .uncovered
                .iter()
                .any(|g| g.kind == RepositoryEvidenceGapKind::MissingRelation),
            "{:?}",
            plan.uncovered
        );
    }

    #[test]
    fn node_budget_exhaustion_reports_truncation() {
        let seeds = [citation("a", "Alpha::run"), citation("z", "Zeta::finish")];
        let mut relations = vec![call_edge("e1", "a", "m1"), call_edge("e2", "m1", "z")];
        for index in 0..8 {
            relations.push(call_edge(
                &format!("fan{index}"),
                "a",
                &format!("fanout{index}"),
            ));
        }
        let limits = RepositoryEvidenceLimits {
            max_candidate_nodes: 3,
            ..RepositoryEvidenceLimits::default()
        };
        let plan = build_repository_evidence_plan(
            RepositoryEvidenceInput {
                question: "Trace Alpha::run calling Zeta::finish",
                task_class: PacketTaskClassDto::RouteTracing,
                seeds: &seeds,
                relations: &relations,
            },
            limits,
        );
        assert!(
            plan.uncovered
                .iter()
                .any(|g| g.kind == RepositoryEvidenceGapKind::TruncatedSearch),
            "{:?}",
            plan.uncovered
        );
        assert!(
            !plan
                .uncovered
                .iter()
                .any(|g| g.kind == RepositoryEvidenceGapKind::MissingRelation),
            "{:?}",
            plan.uncovered
        );
    }

    #[test]
    fn unusable_edges_do_not_spend_the_traversal_budget() {
        // RouteTracing only traverses CALL. The IMPORT edges arrive first, and
        // taking the first max_candidate_edges relations would spend the whole
        // budget on edges this task class can never follow.
        let seeds = [citation("a", "Alpha::run"), citation("b", "Beta::finish")];
        let mut relations = Vec::new();
        for index in 0..8 {
            relations.push(GraphEdgeDto {
                kind: EdgeKind::IMPORT,
                ..call_edge(&format!("import{index}"), "a", &format!("module{index}"))
            });
        }
        relations.push(call_edge("forward", "a", "b"));
        let limits = RepositoryEvidenceLimits {
            max_candidate_edges: 4,
            ..RepositoryEvidenceLimits::default()
        };
        let plan = build_repository_evidence_plan(
            RepositoryEvidenceInput {
                question: "Trace Alpha::run calling Beta::finish",
                task_class: PacketTaskClassDto::RouteTracing,
                seeds: &seeds,
                relations: &relations,
            },
            limits,
        );
        assert!(
            plan.material_edge_ids.iter().any(|e| e.0 == "forward"),
            "{:?}",
            plan.material_edge_ids
        );
    }

    #[test]
    fn a_truncated_graph_never_reports_a_missing_relation() {
        let seeds = [citation("a", "Alpha::run"), citation("b", "Beta::finish")];
        let mut relations = Vec::new();
        for index in 0..8 {
            relations.push(call_edge(
                &format!("noise{index}"),
                &format!("other{index}"),
                &format!("target{index}"),
            ));
        }
        let limits = RepositoryEvidenceLimits {
            max_candidate_edges: 4,
            ..RepositoryEvidenceLimits::default()
        };
        let plan = build_repository_evidence_plan(
            RepositoryEvidenceInput {
                question: "Trace Alpha::run calling Beta::finish",
                task_class: PacketTaskClassDto::RouteTracing,
                seeds: &seeds,
                relations: &relations,
            },
            limits,
        );
        assert!(
            !plan
                .uncovered
                .iter()
                .any(|g| g.kind == RepositoryEvidenceGapKind::MissingRelation),
            "{:?}",
            plan.uncovered
        );
        assert!(
            plan.uncovered
                .iter()
                .any(|g| g.kind == RepositoryEvidenceGapKind::TruncatedSearch),
            "{:?}",
            plan.uncovered
        );
    }

    #[test]
    fn a_complete_graph_still_reports_a_genuine_missing_relation() {
        // Nothing was cut, so an exhausted search is real evidence of absence
        // within the retained graph and must stay MissingRelation.
        let seeds = [citation("a", "Alpha::run"), citation("b", "Beta::finish")];
        let relations = [call_edge("elsewhere", "other", "target")];
        let plan = build_repository_evidence_plan(
            RepositoryEvidenceInput {
                question: "Trace Alpha::run calling Beta::finish",
                task_class: PacketTaskClassDto::RouteTracing,
                seeds: &seeds,
                relations: &relations,
            },
            RepositoryEvidenceLimits::default(),
        );
        assert!(
            plan.uncovered
                .iter()
                .any(|g| g.kind == RepositoryEvidenceGapKind::MissingRelation),
            "{:?}",
            plan.uncovered
        );
        assert!(
            !plan
                .uncovered
                .iter()
                .any(|g| g.kind == RepositoryEvidenceGapKind::TruncatedSearch),
            "{:?}",
            plan.uncovered
        );
    }

    #[test]
    fn directed_call_forward_route_still_selects_path() {
        let seeds = [citation("a", "Alpha::run"), citation("b", "Beta::finish")];
        let relations = [call_edge("forward", "a", "b")];
        let plan = build_repository_evidence_plan(
            RepositoryEvidenceInput {
                question: "Trace Alpha::run calling Beta::finish",
                task_class: PacketTaskClassDto::RouteTracing,
                seeds: &seeds,
                relations: &relations,
            },
            RepositoryEvidenceLimits::default(),
        );
        assert!(
            plan.objectives
                .iter()
                .any(|o| o.kind == RepositoryEvidenceObjectiveKind::RelationPath)
        );
        assert!(plan.material_edge_ids.iter().any(|e| e.0 == "forward"));
    }
}
