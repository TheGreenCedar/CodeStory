use crate::candidate::{
    CandidateHit, CandidateLane, CandidateLaneEvidence, CandidateSource, RankFeatures,
    is_phantom_sidecar_hit,
};
use crate::query_features::{QueryFeatures, QueryShape};
use codestory_contracts::graph::NodeKind;
use codestory_store::FileRole;
use std::path::Path;

#[derive(Debug, Clone, Copy)]
struct RrfLaneWeights {
    lexical: f32,
    semantic: f32,
    graph: f32,
}

const RRF_K: f32 = 20.0;
pub const RANKING_POLICY_VERSION: &str = "weighted_rrf_v2";
const FUSED_RRF_WEIGHT: f32 = 0.70;
const GRAPH_SUPPORT_WEIGHT: f32 = 0.15;
const TEXT_QUALITY_WEIGHT: f32 = 0.05;
const REQUESTED_ROLE_WEIGHT: f32 = 0.05;
const QUERY_OVERLAP_WEIGHT: f32 = 0.05;

pub fn rank_candidates(
    features: &QueryFeatures,
    mut candidates: Vec<CandidateHit>,
) -> Vec<CandidateHit> {
    let query_tokens = tokenize(&features.raw_query);
    candidates.retain(|candidate| !is_phantom_sidecar_hit(candidate));
    retain_primary_candidates_for_query(features, &mut candidates);
    for candidate in &mut candidates {
        candidate.ensure_source_lane();
    }
    assign_missing_lane_ranks(&mut candidates, CandidateLane::Lexical);
    assign_missing_lane_ranks(&mut candidates, CandidateLane::Semantic);
    assign_missing_lane_ranks(&mut candidates, CandidateLane::Graph);
    let pinned_exact_node = unique_exact_definition(features, &candidates);

    let lane_weights = rrf_weights_for_shape(features.shape);
    for candidate in &mut candidates {
        let rank_features = build_rank_features(candidate, features, &query_tokens);
        let rrf = weighted_rrf(candidate, lane_weights);
        candidate.score = FUSED_RRF_WEIGHT * rrf
            + GRAPH_SUPPORT_WEIGHT * rank_features.scip_distance.clamp(0.0, 1.0)
            + TEXT_QUALITY_WEIGHT * rank_features.text_quality
            + REQUESTED_ROLE_WEIGHT * rank_features.requested_role_agreement
            + QUERY_OVERLAP_WEIGHT * rank_features.token_overlap;
        candidate.rank_features = Some(rank_features);
    }

    candidates.sort_by(|left, right| {
        let left_pinned = pinned_exact_node
            .as_deref()
            .is_some_and(|node_id| left.node_id.as_deref() == Some(node_id));
        let right_pinned = pinned_exact_node
            .as_deref()
            .is_some_and(|node_id| right.node_id.as_deref() == Some(node_id));
        right_pinned
            .cmp(&left_pinned)
            .then_with(|| {
                right
                    .score
                    .partial_cmp(&left.score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| file_role_sort_rank(left).cmp(&file_role_sort_rank(right)))
            .then_with(|| source_sort_rank(left).cmp(&source_sort_rank(right)))
            .then_with(|| left.file_path.cmp(&right.file_path))
            .then_with(|| left.symbol_name.cmp(&right.symbol_name))
            .then_with(|| left.start_line.cmp(&right.start_line))
            .then_with(|| left.node_id.cmp(&right.node_id))
    });
    candidates
}

pub(crate) fn retain_primary_candidates_for_query(
    features: &QueryFeatures,
    candidates: &mut Vec<CandidateHit>,
) {
    let query_tokens = tokenize(&features.raw_query);
    for candidate in candidates.iter_mut() {
        candidate.file_role = Some(effective_file_role(candidate));
    }
    if candidates
        .iter()
        .all(|candidate| candidate.file_role.is_some_and(non_primary_search_role))
    {
        return;
    }
    candidates.retain(|candidate| {
        let role = candidate.file_role.unwrap_or(FileRole::Source);
        !non_primary_search_role(role)
            || query_requests_file_role(&query_tokens, role)
            || candidate.provenance.iter().any(|label| label == "exact")
    });
}

fn unique_exact_definition(
    _features: &QueryFeatures,
    candidates: &[CandidateHit],
) -> Option<String> {
    let mut node_ids = candidates
        .iter()
        .filter(|candidate| candidate.provenance.iter().any(|label| label == "exact"))
        .filter_map(|candidate| candidate.node_id.clone())
        .collect::<Vec<_>>();
    node_ids.sort();
    node_ids.dedup();
    (node_ids.len() == 1).then(|| node_ids.remove(0))
}

fn rrf_weights_for_shape(shape: QueryShape) -> RrfLaneWeights {
    match shape {
        QueryShape::SymbolLike => RrfLaneWeights {
            lexical: 1.0,
            semantic: 0.5,
            graph: 1.25,
        },
        QueryShape::PathLike => RrfLaneWeights {
            lexical: 1.25,
            semantic: 0.0,
            graph: 0.75,
        },
        QueryShape::NaturalLanguage => RrfLaneWeights {
            lexical: 1.0,
            semantic: 1.0,
            graph: 0.75,
        },
        QueryShape::Mixed => RrfLaneWeights {
            lexical: 1.0,
            semantic: 0.85,
            graph: 1.0,
        },
    }
}

fn assign_missing_lane_ranks(candidates: &mut [CandidateHit], lane: CandidateLane) {
    let mut indices = candidates
        .iter()
        .enumerate()
        .filter_map(|(index, candidate)| {
            lane_evidence(candidate, lane)
                .is_some_and(|evidence| evidence.rank == 0)
                .then_some(index)
        })
        .collect::<Vec<_>>();
    let mut next_rank = candidates
        .iter()
        .filter_map(|candidate| lane_evidence(candidate, lane))
        .map(|evidence| evidence.rank)
        .max()
        .unwrap_or(0)
        .saturating_add(1);
    indices.sort_by(|left, right| {
        let left_candidate = &candidates[*left];
        let right_candidate = &candidates[*right];
        lane_evidence(right_candidate, lane)
            .expect("ranked lane evidence")
            .raw_score
            .partial_cmp(
                &lane_evidence(left_candidate, lane)
                    .expect("ranked lane evidence")
                    .raw_score,
            )
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left_candidate.file_path.cmp(&right_candidate.file_path))
            .then_with(|| left_candidate.symbol_name.cmp(&right_candidate.symbol_name))
            .then_with(|| left_candidate.start_line.cmp(&right_candidate.start_line))
    });
    for index in indices {
        lane_evidence_mut(&mut candidates[index], lane).rank = next_rank;
        next_rank = next_rank.saturating_add(1);
    }
}

fn lane_evidence(candidate: &CandidateHit, lane: CandidateLane) -> Option<&CandidateLaneEvidence> {
    match lane {
        CandidateLane::Lexical => candidate.lane_scores.lexical.as_ref(),
        CandidateLane::Semantic => candidate.lane_scores.semantic.as_ref(),
        CandidateLane::Graph => candidate.lane_scores.graph.as_ref(),
    }
}

fn lane_evidence_mut(
    candidate: &mut CandidateHit,
    lane: CandidateLane,
) -> &mut CandidateLaneEvidence {
    match lane {
        CandidateLane::Lexical => candidate.lane_scores.lexical.as_mut(),
        CandidateLane::Semantic => candidate.lane_scores.semantic.as_mut(),
        CandidateLane::Graph => candidate.lane_scores.graph.as_mut(),
    }
    .expect("candidate selected from this lane")
}

fn weighted_rrf(candidate: &CandidateHit, weights: RrfLaneWeights) -> f32 {
    let graph = candidate_has_typed_graph_support(candidate)
        .then_some(candidate.lane_scores.graph.as_ref())
        .flatten();
    let weighted = [
        (candidate.lane_scores.lexical.as_ref(), weights.lexical),
        (candidate.lane_scores.semantic.as_ref(), weights.semantic),
        (graph, weights.graph),
    ]
    .into_iter()
    .filter_map(|(evidence, weight)| {
        if weight <= 0.0 {
            return None;
        }
        let evidence = evidence?;
        Some(weight / (RRF_K + evidence.rank.max(1) as f32))
    })
    .sum::<f32>();
    let maximum = (weights.lexical + weights.semantic + weights.graph) / (RRF_K + 1.0);
    if maximum <= 0.0 {
        0.0
    } else {
        (weighted / maximum).clamp(0.0, 1.0)
    }
}

fn build_rank_features(
    candidate: &CandidateHit,
    query: &QueryFeatures,
    query_tokens: &[String],
) -> RankFeatures {
    let path_lower = candidate.file_path.to_ascii_lowercase();
    let symbol_lower = candidate
        .symbol_name
        .as_deref()
        .unwrap_or("")
        .to_ascii_lowercase();
    let qualified_lower = candidate
        .qualified_name
        .as_deref()
        .unwrap_or("")
        .to_ascii_lowercase();

    let lexical = candidate
        .lane_scores
        .lexical
        .as_ref()
        .map(|evidence| evidence.raw_score)
        .unwrap_or(0.0);
    let semantic = candidate
        .lane_scores
        .semantic
        .as_ref()
        .map(|evidence| evidence.raw_score)
        .unwrap_or(0.0);
    let scip_distance = if candidate_has_typed_graph_support(candidate) {
        candidate
            .lane_scores
            .graph
            .as_ref()
            .map(|evidence| evidence.raw_score)
            .unwrap_or(0.0)
    } else {
        0.0
    };

    let file_role_prior = file_role_prior(effective_file_role(candidate));
    let definition_quality = definition_quality(candidate);
    let token_overlap = token_overlap_score(
        query_tokens,
        &path_lower,
        &format!("{symbol_lower} {qualified_lower}"),
    );
    let text_quality = (0.75 * text_file_quality(effective_file_role(candidate), query_tokens)
        + 0.25 * definition_quality)
        .clamp(0.0, 1.0);
    let requested_role_agreement = requested_role_agreement(
        &query.intent.structural_kinds,
        &path_lower,
        &symbol_lower,
        candidate.structural_kind,
    );

    RankFeatures {
        ranking_policy: RANKING_POLICY_VERSION.into(),
        lexical,
        semantic,
        scip_distance,
        file_role_prior,
        definition_quality,
        token_overlap,
        text_quality,
        requested_role_agreement,
    }
}

fn candidate_has_typed_graph_support(candidate: &CandidateHit) -> bool {
    candidate.graph_evidence.is_some()
        || candidate.scip_hop_distance.is_some()
        || candidate
            .provenance
            .iter()
            .any(|label| matches!(label.as_str(), "graph_neighbor" | "scip_graph_projection"))
}

fn requested_role_agreement(
    structural_kinds: &[String],
    path_lower: &str,
    symbol_lower: &str,
    structural_kind: Option<NodeKind>,
) -> f32 {
    let requested = structural_kinds.iter().collect::<Vec<_>>();
    if requested.is_empty() {
        return 0.0;
    }
    let structural_label = structural_kind.map(structural_kind_label);
    let matched = requested
        .iter()
        .filter(|term| {
            path_lower.contains(term.as_str())
                || symbol_lower.contains(term.as_str())
                || structural_label.is_some_and(|label| label == term.as_str())
        })
        .count();
    matched as f32 / requested.len() as f32
}

/// Definition quality claims a definition, so a bare display name cannot earn
/// it: only a candidate resolved to an indexed node or anchored to a line has
/// a definition site to report.
fn definition_quality(candidate: &CandidateHit) -> f32 {
    let named = candidate
        .symbol_name
        .as_deref()
        .is_some_and(|symbol_name| !symbol_name.trim().is_empty());
    let anchored = candidate.node_id.is_some() && candidate.start_line.is_some();
    if !named || !anchored {
        return 0.0;
    }
    match candidate.structural_kind {
        Some(
            NodeKind::FUNCTION
            | NodeKind::METHOD
            | NodeKind::STRUCT
            | NodeKind::CLASS
            | NodeKind::INTERFACE
            | NodeKind::ENUM
            | NodeKind::UNION
            | NodeKind::TYPEDEF
            | NodeKind::MODULE
            | NodeKind::NAMESPACE
            | NodeKind::PACKAGE,
        ) => 1.0,
        Some(
            NodeKind::ANNOTATION
            | NodeKind::GLOBAL_VARIABLE
            | NodeKind::FIELD
            | NodeKind::VARIABLE
            | NodeKind::CONSTANT
            | NodeKind::ENUM_CONSTANT,
        ) => 0.65,
        Some(NodeKind::MACRO) => 0.25,
        Some(NodeKind::FILE | NodeKind::UNKNOWN | NodeKind::BUILTIN_TYPE) | None => 0.0,
        Some(NodeKind::TYPE_PARAMETER) => 0.45,
    }
}

fn structural_kind_label(kind: NodeKind) -> &'static str {
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

fn file_role_prior(file_role: FileRole) -> f32 {
    match file_role {
        FileRole::Entrypoint => 0.95,
        FileRole::Source => 0.72,
        FileRole::Test => 0.35,
        FileRole::Docs => 0.30,
        FileRole::Benchmark => 0.28,
        FileRole::Generated => 0.22,
        FileRole::Vendor => 0.18,
    }
}

fn text_file_quality(file_role: FileRole, query_tokens: &[String]) -> f32 {
    let asks_for = |labels: &[&str]| {
        query_tokens
            .iter()
            .any(|token| labels.contains(&token.as_str()))
    };
    match file_role {
        FileRole::Entrypoint => 1.0,
        FileRole::Source => 0.80,
        FileRole::Test
            if asks_for(&[
                "test", "tests", "testing", "spec", "specs", "fixture", "fixtures",
            ]) =>
        {
            1.0
        }
        FileRole::Docs if asks_for(&["doc", "docs", "documentation", "readme"]) => 1.0,
        FileRole::Benchmark if asks_for(&["bench", "benchmark", "benchmarks", "performance"]) => {
            1.0
        }
        FileRole::Generated if asks_for(&["generated", "codegen"]) => 1.0,
        FileRole::Vendor
            if asks_for(&["vendor", "dependency", "dependencies", "third", "party"]) =>
        {
            1.0
        }
        FileRole::Docs => 0.35,
        FileRole::Benchmark => 0.20,
        FileRole::Test | FileRole::Generated | FileRole::Vendor => 0.0,
    }
}

fn non_primary_search_role(role: FileRole) -> bool {
    matches!(
        role,
        FileRole::Test | FileRole::Benchmark | FileRole::Generated | FileRole::Vendor
    )
}

fn query_requests_file_role(query_tokens: &[String], role: FileRole) -> bool {
    let asks_for = |labels: &[&str]| {
        query_tokens
            .iter()
            .any(|token| labels.contains(&token.as_str()))
    };
    match role {
        FileRole::Test => asks_for(&["test", "tests", "testing", "spec", "specs", "fixture"]),
        FileRole::Benchmark => asks_for(&["bench", "benchmark", "benchmarks", "performance"]),
        FileRole::Generated => asks_for(&["generated", "codegen"]),
        FileRole::Vendor => asks_for(&["vendor", "dependency", "dependencies", "third", "party"]),
        FileRole::Entrypoint | FileRole::Source | FileRole::Docs => true,
    }
}

fn effective_file_role(candidate: &CandidateHit) -> FileRole {
    if candidate.qualified_name.as_deref().is_some_and(|name| {
        name.starts_with("tests::")
            || name.contains("::tests::")
            || name.starts_with("test::")
            || name.contains("::test::")
    }) {
        return FileRole::Test;
    }
    let path = Path::new(&candidate.file_path);
    if path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "md" | "mdx" | "rst"
            )
        })
    {
        FileRole::Docs
    } else {
        candidate
            .file_role
            .unwrap_or_else(|| FileRole::classify_path(path))
    }
}

fn file_role_sort_rank(candidate: &CandidateHit) -> u8 {
    match effective_file_role(candidate) {
        FileRole::Entrypoint => 0,
        FileRole::Source => 1,
        FileRole::Test => 2,
        FileRole::Docs => 3,
        FileRole::Benchmark => 4,
        FileRole::Generated => 5,
        FileRole::Vendor => 6,
    }
}

fn source_sort_rank(candidate: &CandidateHit) -> u8 {
    match candidate.source {
        CandidateSource::Lexical => 0,
        CandidateSource::Scip => 1,
        CandidateSource::Semantic => 2,
        CandidateSource::Legacy => 3,
    }
}

fn token_overlap_score(query_tokens: &[String], path_lower: &str, symbol_lower: &str) -> f32 {
    if query_tokens.is_empty() {
        return 0.0;
    }
    let mut hits = 0usize;
    for token in query_tokens {
        if token.len() < 2 {
            continue;
        }
        if path_lower.contains(token) || symbol_lower.contains(token) {
            hits += 1;
        }
    }
    hits as f32 / query_tokens.len() as f32
}

fn tokenize(query: &str) -> Vec<String> {
    query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|token| token.len() >= 2)
        .map(|token| token.to_ascii_lowercase())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::candidate::CandidateSource;
    use crate::query_features::classify_query;
    use codestory_store::FileRole;

    #[test]
    fn ranker_prefers_higher_lexical_for_path_query() {
        let features = classify_query("src/lib.rs");
        let candidates = vec![
            CandidateHit::lexical_stub("src/lib.rs", 0.9),
            CandidateHit::lexical_stub("docs/readme.md", 0.2),
        ];
        let ranked = rank_candidates(&features, candidates);
        assert_eq!(ranked[0].file_path, "src/lib.rs");
        assert!(ranked[0].score > ranked[1].score);
    }

    #[test]
    fn ranker_does_not_use_repo_name_features() {
        let features = classify_query("handler");
        let mut hit = CandidateHit::lexical_stub("src/handler.rs", 0.8);
        hit.source = CandidateSource::Lexical;
        hit.file_role = Some(FileRole::Source);
        let ranked = rank_candidates(&features, vec![hit]);
        let rf = ranked[0].rank_features.as_ref().expect("features");
        assert!(rf.file_role_prior > 0.0);
    }

    #[test]
    fn ranker_exports_zero_graph_feature_without_graph_provenance() {
        let features = classify_query("explain service startup");
        let lexical = CandidateHit::lexical_stub("src/service.rs", 0.8);
        let dense = CandidateHit::with_source(
            "src/search.rs",
            Some("SearchService".into()),
            0.9,
            CandidateSource::Semantic,
        );

        let ranked = rank_candidates(&features, vec![lexical, dense]);

        for candidate in ranked {
            assert_eq!(
                candidate
                    .rank_features
                    .as_ref()
                    .expect("rank features")
                    .scip_distance,
                0.0,
                "{} should not export graph evidence",
                candidate.file_path
            );
        }
    }

    #[test]
    fn ranker_exports_only_dense_feature_for_pure_dense_candidate() {
        let features = classify_query("explain search service");
        let mut dense = CandidateHit::with_source(
            "src/search.rs",
            Some("SearchService".into()),
            0.9,
            CandidateSource::Semantic,
        );
        dense.provenance = vec!["dense_anchor".into()];

        let ranked = rank_candidates(&features, vec![dense]);
        let rank_features = ranked[0].rank_features.as_ref().expect("rank features");

        assert_eq!(rank_features.lexical, 0.0);
        assert!(rank_features.semantic > 0.0);
        assert_eq!(rank_features.scip_distance, 0.0);
    }

    #[test]
    fn ranker_prefers_entrypoint_role_over_test_role() {
        let features = classify_query("main startup entrypoint");
        let mut test_hit = CandidateHit::lexical_stub("src/main_test.rs", 0.94);
        test_hit.file_role = Some(FileRole::Test);
        let mut entry_hit = CandidateHit::lexical_stub("src/main.rs", 0.72);
        entry_hit.file_role = Some(FileRole::Entrypoint);
        let ranked = rank_candidates(&features, vec![test_hit, entry_hit]);
        assert_eq!(
            ranked.first().map(|hit| hit.file_path.as_str()),
            Some("src/main.rs")
        );
    }

    #[test]
    fn ranker_does_not_apply_a_hidden_structural_cap() {
        let features = classify_query("layout styles for dashboard");
        let mut structural = CandidateHit::lexical_stub("src/ui/layout.css", 0.9);
        let mut graph = CandidateHit::lexical_stub("src/app/dashboard.rs", 0.55);
        structural.source = CandidateSource::Lexical;
        graph.source = CandidateSource::Lexical;
        let ranked = rank_candidates(&features, vec![structural, graph]);
        assert_eq!(ranked[0].file_path, "src/ui/layout.css");
    }

    #[test]
    fn ranker_boosts_structural_on_strong_token_overlap() {
        let features = classify_query("primary button layout css class");
        let mut structural = CandidateHit::lexical_stub("src/ui/primary.css", 0.7);
        structural.symbol_name = Some("primary".to_string());
        // A lexical symbol document always resolves to an indexed node, which is
        // what earns the candidate its definition-quality feature.
        structural.node_id = Some("41".to_string());
        structural.start_line = Some(12);
        structural.source = CandidateSource::Lexical;
        let mut graph = CandidateHit::lexical_stub("src/ui/components.rs", 0.72);
        graph.source = CandidateSource::Lexical;
        let ranked = rank_candidates(&features, vec![structural, graph]);
        assert_eq!(ranked[0].file_path, "src/ui/primary.css");
    }

    #[test]
    fn ranker_combines_lane_rank_with_query_overlap_for_structural_files() {
        let features = classify_query("UserService class method");
        let mut structural = CandidateHit::lexical_stub("schema/users.sql", 0.99);
        let mut graph = CandidateHit::lexical_stub("src/user_service.rs", 0.8);
        structural.source = CandidateSource::Lexical;
        graph.source = CandidateSource::Lexical;
        let ranked = rank_candidates(&features, vec![structural, graph]);
        assert_eq!(ranked[0].file_path, "src/user_service.rs");
    }

    #[test]
    fn ranker_drops_phantom_hits_by_default() {
        let features = classify_query("search pipeline");
        let candidates = vec![
            CandidateHit::with_source("lexical:search", None, 0.9, CandidateSource::Lexical),
            CandidateHit::lexical_stub("crates/core/search.rs", 0.7),
        ];
        let ranked = rank_candidates(&features, candidates);
        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0].file_path, "crates/core/search.rs");
    }

    #[test]
    fn ranker_keeps_exact_symbol_above_semantic_expansion() {
        let features = classify_query("IndexManifest");
        let mut exact = CandidateHit::with_source(
            "crates/runtime/src/index.rs",
            Some("codestory::IndexManifest".into()),
            0.55,
            CandidateSource::Lexical,
        );
        exact.file_role = Some(FileRole::Source);
        let mut semantic = CandidateHit::with_source(
            "docs/retrieval.md",
            Some("manifest overview".into()),
            0.99,
            CandidateSource::Semantic,
        );
        semantic.file_role = Some(FileRole::Docs);

        let ranked = rank_candidates(&features, vec![semantic, exact]);
        assert_eq!(
            ranked.first().map(|hit| hit.symbol_name.as_deref()),
            Some(Some("codestory::IndexManifest"))
        );
    }

    #[test]
    fn ranker_excludes_non_primary_roles_from_an_ordinary_source_query() {
        let features = classify_query("explain request json output event processing");
        let semantic_test = CandidateHit::with_source(
            "workspace/app/tests/event_processor_with_json_output.rs",
            Some("event processor json output".into()),
            0.99,
            CandidateSource::Semantic,
        );
        let colocated_test = CandidateHit::with_source(
            "workspace/app/src/event_processor_with_jsonl_output_tests.rs",
            Some("jsonl event test output".into()),
            0.98,
            CandidateSource::Semantic,
        );
        let source = CandidateHit::with_source(
            "workspace/app/src/event_processor.rs",
            Some("EventProcessor".into()),
            0.72,
            CandidateSource::Lexical,
        );

        let ranked = rank_candidates(&features, vec![semantic_test, colocated_test, source]);

        assert!(ranked.iter().any(|hit| {
            hit.file_path == "workspace/app/src/event_processor.rs"
                && hit.file_role == Some(FileRole::Source)
        }));
        assert!(
            ranked
                .iter()
                .all(|hit| hit.file_role != Some(FileRole::Test))
        );
    }

    #[test]
    fn ranker_does_not_mutate_scores_from_symbol_name_heuristics() {
        let features = classify_query("explain runtime orchestration and search projection");
        let test_helper = CandidateHit::with_source(
            "crates/runtime/src/search/engine.rs",
            Some("EmbeddingRuntime::test_runtime".into()),
            0.99,
            CandidateSource::Lexical,
        );
        let production = CandidateHit::with_source(
            "crates/runtime/src/services.rs",
            Some("IndexService::run_indexing_blocking".into()),
            0.72,
            CandidateSource::Lexical,
        );

        let ranked = rank_candidates(&features, vec![test_helper, production]);

        assert_eq!(
            ranked.first().and_then(|hit| hit.symbol_name.as_deref()),
            Some("EmbeddingRuntime::test_runtime")
        );
    }

    #[test]
    fn ranker_does_not_demote_tests_when_prompt_asks_for_tests() {
        let features = classify_query("event processor tests json output");
        let semantic_test = CandidateHit::with_source(
            "workspace/app/tests/event_processor_with_json_output.rs",
            Some("event processor json output".into()),
            0.99,
            CandidateSource::Semantic,
        );
        let source = CandidateHit::with_source(
            "workspace/app/src/event_processor.rs",
            Some("EventProcessor".into()),
            0.72,
            CandidateSource::Lexical,
        );

        let ranked = rank_candidates(&features, vec![source, semantic_test]);

        assert_eq!(
            ranked.first().map(|hit| hit.file_path.as_str()),
            Some("workspace/app/tests/event_processor_with_json_output.rs")
        );
        assert_eq!(ranked[0].file_role, Some(FileRole::Test));
    }

    #[test]
    fn ranker_prefers_lexical_source_anchor_over_dense_dto_distractor() {
        let features = classify_query("delivery adapter emits ranked findings with provenance");
        let mut source = CandidateHit::with_source(
            "src/delivery/output.rs",
            Some("append_ranked_findings".into()),
            0.92,
            CandidateSource::Lexical,
        );
        source.file_role = Some(FileRole::Source);
        let mut dense_dto = CandidateHit::with_source(
            "src/contracts/delivery.rs",
            Some("DeliveryTraceSummary".into()),
            0.99,
            CandidateSource::Semantic,
        );
        dense_dto.file_role = Some(FileRole::Source);

        let ranked = rank_candidates(&features, vec![dense_dto, source]);

        assert_eq!(
            ranked.first().map(|hit| hit.file_path.as_str()),
            Some("src/delivery/output.rs")
        );
    }

    #[test]
    fn ranker_keeps_broad_lexical_source_anchor_inside_resolved_window() {
        let features = classify_query("delivery adapter emits ranked findings with provenance");
        let mut source = CandidateHit::with_source(
            "src/delivery/evidence.rs",
            Some("decorate_ranked_finding".into()),
            0.82,
            CandidateSource::Lexical,
        );
        source.file_role = Some(FileRole::Source);
        let dense_distractors = [
            ("DeliveryTraceDto", "src/contracts/delivery.rs"),
            ("FindingTraceDto", "src/contracts/findings.rs"),
            ("RankedResultDto", "src/contracts/results.rs"),
            ("IndexedRecordDto", "src/contracts/records.rs"),
            ("FindingShadowDto", "src/contracts/shadow.rs"),
        ]
        .into_iter()
        .map(|(symbol, path)| {
            let mut hit = CandidateHit::with_source(
                path,
                Some(symbol.into()),
                0.99,
                CandidateSource::Semantic,
            );
            hit.file_role = Some(FileRole::Source);
            hit
        });

        let ranked = rank_candidates(&features, dense_distractors.chain([source]).collect());

        assert!(
            ranked.iter().take(5).any(|hit| {
                hit.file_path == "src/delivery/evidence.rs"
                    && hit.symbol_name.as_deref() == Some("decorate_ranked_finding")
            }),
            "direct lexical source evidence should stay inside the resolved top-5 window: {ranked:#?}"
        );
    }

    #[test]
    fn ranker_does_not_export_graph_for_same_file_name_affinity() {
        let features = classify_query("how does service startup flow");
        let mut fused = CandidateHit::with_source(
            "src/service.rs",
            Some("ExtensionService".into()),
            0.85,
            CandidateSource::Lexical,
        );
        fused.provenance = vec![
            "lexical_source".into(),
            "dense_anchor".into(),
            "same_file_name_affinity".into(),
        ];
        fused.scip_hop_distance = Some(1);

        let ranked = rank_candidates(&features, vec![fused]);
        let rank_features = ranked[0].rank_features.as_ref().expect("rank features");

        assert_eq!(rank_features.lexical, 0.85);
        assert_eq!(rank_features.semantic, 0.0);
        assert_eq!(rank_features.scip_distance, 0.0);
    }

    #[test]
    fn ranker_reports_the_measured_dense_similarity_without_a_floor() {
        let features = classify_query("how does the request deadline reach a worker");
        let mut weak_dense = CandidateHit::with_source(
            "docs/notes/glossary.md",
            Some("glossary".into()),
            0.02,
            CandidateSource::Semantic,
        );
        weak_dense.node_id = Some("91".into());
        weak_dense.provenance = vec!["dense_anchor".into()];
        let strong_lexical = CandidateHit::lexical_stub("src/worker/deadline.rs", 0.81);

        let ranked = rank_candidates(&features, vec![weak_dense, strong_lexical]);

        let dense = ranked
            .iter()
            .find(|hit| hit.file_path == "docs/notes/glossary.md");
        assert!(
            dense.is_none_or(
                |hit| hit.rank_features.as_ref().expect("rank features").semantic < 0.4
            ),
            "a barely related vector must not report a floored dense feature: {ranked:#?}"
        );
        assert_eq!(
            ranked.first().map(|hit| hit.file_path.as_str()),
            Some("src/worker/deadline.rs"),
            "measured lexical evidence must outrank an unrelated vector: {ranked:#?}"
        );
    }

    #[test]
    fn ranker_reserves_definition_quality_for_anchored_definitions() {
        let features = classify_query("how does the worker drain requests");
        let unanchored = CandidateHit::with_source(
            "src/worker/pool.rs",
            Some("worker pool overview".into()),
            0.6,
            CandidateSource::Semantic,
        );
        let mut anchored = unanchored.clone();
        anchored.node_id = Some("77".into());
        anchored.start_line = Some(31);
        anchored.structural_kind = Some(NodeKind::FUNCTION);

        let unanchored_quality = rank_candidates(&features, vec![unanchored])[0]
            .rank_features
            .as_ref()
            .expect("rank features")
            .definition_quality;
        let anchored_quality = rank_candidates(&features, vec![anchored])[0]
            .rank_features
            .as_ref()
            .expect("rank features")
            .definition_quality;

        assert_eq!(anchored_quality, 1.0);
        assert_eq!(
            unanchored_quality, 0.0,
            "a display name with no definition site is not definition evidence"
        );
    }

    #[test]
    fn structural_kind_prevents_macro_invocations_from_claiming_definition_quality() {
        let features = classify_query("how does packet evidence reach the output adapter");
        let mut function = CandidateHit::with_source(
            "src/output.rs",
            Some("append_packet_evidence".into()),
            0.7,
            CandidateSource::Semantic,
        );
        function.node_id = Some("1".into());
        function.start_line = Some(40);
        function.structural_kind = Some(NodeKind::FUNCTION);
        let mut invocation = CandidateHit::with_source(
            "src/output.rs",
            Some("assert_eq".into()),
            0.7,
            CandidateSource::Semantic,
        );
        invocation.node_id = Some("2".into());
        invocation.start_line = Some(90);
        invocation.structural_kind = Some(NodeKind::MACRO);

        let ranked = rank_candidates(&features, vec![invocation, function]);
        let quality = |name: &str| {
            ranked
                .iter()
                .find(|candidate| candidate.symbol_name.as_deref() == Some(name))
                .and_then(|candidate| candidate.rank_features.as_ref())
                .map(|features| features.definition_quality)
                .expect("ranked candidate")
        };
        assert_eq!(quality("append_packet_evidence"), 1.0);
        assert_eq!(quality("assert_eq"), 0.25);
    }

    #[test]
    fn ranker_exports_graph_only_for_explicit_graph_provenance() {
        let features = classify_query("how does service startup flow");
        let mut graph = CandidateHit::with_source(
            "src/service.rs",
            Some("ExtensionService".into()),
            0.85,
            CandidateSource::Scip,
        );
        graph.provenance = vec!["graph_neighbor".into()];
        graph.scip_hop_distance = Some(1);

        let ranked = rank_candidates(&features, vec![graph]);
        let rank_features = ranked[0].rank_features.as_ref().expect("rank features");

        assert_eq!(rank_features.lexical, 0.0);
        assert_eq!(rank_features.semantic, 0.0);
        assert_eq!(rank_features.scip_distance, 0.85);
    }

    #[test]
    fn ranker_preserves_independent_lane_scores_through_rrf() {
        let features = classify_query("explain how request dispatch calls a worker");
        let mut candidate = CandidateHit::with_source(
            "src/dispatch.rs",
            Some("dispatch".into()),
            0.31,
            CandidateSource::Lexical,
        );
        candidate.record_lane(CandidateLane::Semantic, 0.83, 4, "dense_anchor");
        candidate.record_lane(CandidateLane::Graph, 0.57, 2, "graph_neighbor");
        candidate.add_provenance("graph_neighbor");

        let ranked = rank_candidates(&features, vec![candidate]);
        let rank_features = ranked[0].rank_features.as_ref().expect("rank features");

        assert_eq!(rank_features.lexical, 0.31);
        assert_eq!(rank_features.semantic, 0.83);
        assert_eq!(rank_features.scip_distance, 0.57);
    }

    #[test]
    fn lexical_score_scale_does_not_cross_contaminate_rrf() {
        let features = classify_query("dispatch worker");
        let first = vec![
            CandidateHit::lexical_stub("src/a.rs", 0.9),
            CandidateHit::lexical_stub("src/b.rs", 0.8),
        ];
        let rescaled = vec![
            CandidateHit::lexical_stub("src/a.rs", 90.0),
            CandidateHit::lexical_stub("src/b.rs", 80.0),
        ];

        let ranked = rank_candidates(&features, first);
        let rescaled_ranked = rank_candidates(&features, rescaled);

        assert_eq!(ranked[0].file_path, rescaled_ranked[0].file_path);
        assert_eq!(ranked[1].file_path, rescaled_ranked[1].file_path);
        assert_eq!(ranked[0].score, rescaled_ranked[0].score);
        assert_eq!(ranked[1].score, rescaled_ranked[1].score);
    }

    #[test]
    fn producer_lane_ranks_are_not_rewritten_from_raw_scores() {
        let features = classify_query("worker");
        let mut first = CandidateHit::lexical_stub("src/first.rs", 0.20);
        first.record_lane(CandidateLane::Lexical, 0.20, 1, "lexical_source");
        let mut second = CandidateHit::lexical_stub("src/second.rs", 0.90);
        second.record_lane(CandidateLane::Lexical, 0.90, 2, "lexical_source");

        let ranked = rank_candidates(&features, vec![second, first]);

        assert_eq!(ranked[0].file_path, "src/first.rs");
        assert_eq!(ranked[0].lane_scores.lexical.as_ref().unwrap().rank, 1);
        assert_eq!(ranked[1].lane_scores.lexical.as_ref().unwrap().rank, 2);
    }

    #[test]
    fn final_score_is_the_declared_convex_combination() {
        let features = classify_query("explain dispatch worker flow");
        let mut candidate = CandidateHit::with_source(
            "src/dispatch_worker.rs",
            Some("dispatch_worker".into()),
            0.73,
            CandidateSource::Lexical,
        );
        candidate.node_id = Some("7".into());
        candidate.record_lane(CandidateLane::Semantic, 0.81, 1, "dense_anchor");
        candidate.record_lane(CandidateLane::Graph, 0.60, 1, "graph_neighbor");
        candidate.add_provenance("graph_neighbor");

        let ranked = rank_candidates(&features, vec![candidate]);
        let candidate = &ranked[0];
        let rank_features = candidate.rank_features.as_ref().expect("rank features");
        let expected = 0.70
            + 0.15 * rank_features.scip_distance
            + 0.05 * rank_features.text_quality
            + 0.05 * rank_features.requested_role_agreement
            + 0.05 * rank_features.token_overlap;

        assert!((candidate.score - expected).abs() < f32::EPSILON * 8.0);
        assert!((0.0..=1.0).contains(&candidate.score));
    }
}
