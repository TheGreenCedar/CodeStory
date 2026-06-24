use crate::candidate::{CandidateHit, CandidateSource, RankFeatures, is_phantom_sidecar_hit};
use crate::query_features::{QueryFeatures, QueryShape};
use codestory_store::FileRole;
use std::path::Path;

#[derive(Debug, Clone, Copy)]
struct RankWeights {
    lexical: f32,
    semantic: f32,
    scip_distance: f32,
    file_role_prior: f32,
    definition_quality: f32,
    token_overlap: f32,
}

const STRUCTURAL_BASELINE_MULTIPLIER: f32 = 0.5;
const STRUCTURAL_DYNAMIC_TOKEN_THRESHOLD: f32 = 0.35;
const STRUCTURAL_DYNAMIC_SEMANTIC_THRESHOLD: f32 = 0.55;
const STRUCTURAL_DYNAMIC_BOOST: f32 = 1.35;

pub fn rank_candidates(
    features: &QueryFeatures,
    mut candidates: Vec<CandidateHit>,
) -> Vec<CandidateHit> {
    let weights = weights_for_shape(features.shape);
    let query_tokens = tokenize(&features.raw_query);
    candidates.retain(|candidate| !is_phantom_sidecar_hit(candidate));

    let query_lower = features.raw_query.to_ascii_lowercase();
    let code_intent = query_has_code_intent(&query_tokens);
    let structural_intent = query_mentions_structural_role(&query_tokens);
    let prefer_primary_code = prefers_primary_code_evidence(features.shape, &query_tokens);
    for candidate in &mut candidates {
        let file_role = effective_file_role(candidate);
        candidate.file_role.get_or_insert(file_role);
        let rank_features = build_rank_features(candidate, &query_tokens);
        let mut fused = score_features(&rank_features, weights);
        fused *= structural_fusion_multiplier(candidate, &rank_features);
        fused *= primary_code_role_multiplier(file_role, prefer_primary_code);
        candidate.score = fused;
        if looks_like_repo_relative_path(&candidate.file_path) {
            candidate.score += 0.08;
        }
        if matches!(features.shape, QueryShape::PathLike)
            && query_lower.contains(&candidate.file_path.to_ascii_lowercase())
        {
            candidate.score += 0.25;
        }
        if matches!(
            features.shape,
            QueryShape::NaturalLanguage | QueryShape::Mixed
        ) {
            let strong_token_hits = strong_query_token_hits(candidate, &query_tokens);
            candidate.score += (strong_token_hits as f32) * 0.06;
            if candidate.source == CandidateSource::Zoekt && strong_token_hits >= 3 {
                candidate.score += 0.08;
            }
        }
        if prefer_primary_code && matches!(file_role, FileRole::Source | FileRole::Entrypoint) {
            candidate.score += primary_source_path_bonus(&candidate.file_path, &query_tokens);
        }
        if prefer_primary_code && symbol_name_looks_test_like(candidate.symbol_name.as_deref()) {
            candidate.score *= 0.55;
        }
        candidate.rank_features = Some(rank_features);
    }

    apply_exact_code_evidence_anchor(features, &query_tokens, &mut candidates);

    let min_rank_score = min_rank_score(features.shape);
    candidates.retain(|candidate| candidate.score >= min_rank_score);

    if code_intent && !structural_intent {
        apply_structural_code_intent_cap(&mut candidates);
    }
    if prefer_primary_code {
        cap_non_primary_below_best_primary(&mut candidates);
        cap_dense_below_strong_lexical_source(&mut candidates, &query_tokens);
    }

    candidates.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| file_role_sort_rank(left).cmp(&file_role_sort_rank(right)))
            .then_with(|| source_sort_rank(left).cmp(&source_sort_rank(right)))
            .then_with(|| left.file_path.cmp(&right.file_path))
            .then_with(|| left.symbol_name.cmp(&right.symbol_name))
    });
    candidates
}

fn looks_like_repo_relative_path(file_path: &str) -> bool {
    let trimmed = file_path.trim();
    !trimmed.is_empty()
        && !trimmed.contains(':')
        && (trimmed.contains('/') || trimmed.contains('\\') || trimmed.contains(char::from(46)))
}

fn weights_for_shape(shape: QueryShape) -> RankWeights {
    match shape {
        QueryShape::SymbolLike => RankWeights {
            lexical: 0.25,
            semantic: 0.15,
            scip_distance: 0.30,
            file_role_prior: 0.10,
            definition_quality: 0.15,
            token_overlap: 0.05,
        },
        QueryShape::PathLike => RankWeights {
            lexical: 0.45,
            semantic: 0.05,
            scip_distance: 0.15,
            file_role_prior: 0.20,
            definition_quality: 0.10,
            token_overlap: 0.05,
        },
        QueryShape::NaturalLanguage => RankWeights {
            lexical: 0.15,
            semantic: 0.40,
            scip_distance: 0.10,
            file_role_prior: 0.10,
            definition_quality: 0.10,
            token_overlap: 0.15,
        },
        QueryShape::Mixed => RankWeights {
            lexical: 0.25,
            semantic: 0.25,
            scip_distance: 0.20,
            file_role_prior: 0.10,
            definition_quality: 0.10,
            token_overlap: 0.10,
        },
    }
}

fn min_rank_score(shape: QueryShape) -> f32 {
    match shape {
        QueryShape::NaturalLanguage => 0.04,
        QueryShape::Mixed => 0.06,
        QueryShape::SymbolLike | QueryShape::PathLike => 0.08,
    }
}

fn build_rank_features(candidate: &CandidateHit, query_tokens: &[String]) -> RankFeatures {
    let path_lower = candidate.file_path.to_ascii_lowercase();
    let symbol_lower = candidate
        .symbol_name
        .as_deref()
        .unwrap_or("")
        .to_ascii_lowercase();

    let has_lexical = matches!(candidate.source, CandidateSource::Zoekt)
        || candidate_has_provenance(candidate, "lexical_source");
    let has_semantic = matches!(candidate.source, CandidateSource::Qdrant)
        || candidate_has_provenance(candidate, "dense_anchor")
        || candidate_has_provenance(candidate, "component_report");
    let has_graph = matches!(candidate.source, CandidateSource::Scip)
        || candidate_has_provenance(candidate, "graph_neighbor")
        || candidate_has_provenance(candidate, "exact");

    let lexical = if matches!(candidate.source, CandidateSource::Legacy) {
        candidate.score * 0.5
    } else if has_lexical {
        candidate.score.max(0.35)
    } else {
        candidate.score * 0.6
    };

    let semantic = if has_semantic {
        candidate.score.max(0.4)
    } else {
        candidate.score * 0.25
    };

    let scip_distance = if has_graph {
        1.0 / (1.0 + candidate.scip_hop_distance.unwrap_or(0) as f32)
    } else {
        0.2
    };

    let file_role_prior = file_role_prior(effective_file_role(candidate));
    let definition_quality = if candidate.symbol_name.is_some() {
        0.85
    } else {
        0.45
    };
    let token_overlap = token_overlap_score(query_tokens, &path_lower, &symbol_lower);

    RankFeatures {
        lexical,
        semantic,
        scip_distance,
        file_role_prior,
        definition_quality,
        token_overlap,
    }
}

fn score_features(features: &RankFeatures, weights: RankWeights) -> f32 {
    weights.lexical * features.lexical
        + weights.semantic * features.semantic
        + weights.scip_distance * features.scip_distance
        + weights.file_role_prior * features.file_role_prior
        + weights.definition_quality * features.definition_quality
        + weights.token_overlap * features.token_overlap
}

fn candidate_has_provenance(candidate: &CandidateHit, label: &str) -> bool {
    candidate
        .provenance
        .iter()
        .any(|candidate_label| candidate_label == label)
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

fn effective_file_role(candidate: &CandidateHit) -> FileRole {
    candidate
        .file_role
        .unwrap_or_else(|| FileRole::classify_path(Path::new(&candidate.file_path)))
}

fn prefers_primary_code_evidence(shape: QueryShape, query_tokens: &[String]) -> bool {
    matches!(shape, QueryShape::NaturalLanguage | QueryShape::Mixed)
        && !query_mentions_non_primary_role(query_tokens)
        && !query_mentions_structural_role(query_tokens)
}

fn query_mentions_non_primary_role(query_tokens: &[String]) -> bool {
    const NON_PRIMARY_ROLE_TERMS: &[&str] = &[
        "test",
        "tests",
        "spec",
        "specs",
        "fixture",
        "fixtures",
        "doc",
        "docs",
        "documentation",
        "readme",
        "benchmark",
        "benchmarks",
        "bench",
        "generated",
        "vendor",
    ];
    query_tokens
        .iter()
        .any(|token| NON_PRIMARY_ROLE_TERMS.contains(&token.as_str()))
}

fn query_mentions_structural_role(query_tokens: &[String]) -> bool {
    const STRUCTURAL_ROLE_TERMS: &[&str] = &[
        "css",
        "html",
        "layout",
        "style",
        "styles",
        "stylesheet",
        "sql",
    ];
    query_tokens
        .iter()
        .any(|token| STRUCTURAL_ROLE_TERMS.contains(&token.as_str()))
}

fn symbol_name_looks_test_like(symbol_name: Option<&str>) -> bool {
    let Some(symbol_name) = symbol_name else {
        return false;
    };
    let symbol = symbol_name.to_ascii_lowercase();
    let local_name = symbol.rsplit("::").next().unwrap_or(symbol.as_str());
    symbol.starts_with("tests::")
        || symbol.contains("::tests::")
        || local_name.starts_with("test_")
        || local_name.ends_with("_test")
        || local_name.ends_with("_tests")
        || local_name.contains("_test_")
        || local_name.contains("_tests_")
}

fn primary_code_role_multiplier(file_role: FileRole, prefer_primary_code: bool) -> f32 {
    if !prefer_primary_code {
        return 1.0;
    }
    match file_role {
        FileRole::Entrypoint => 1.08,
        FileRole::Source => 1.02,
        FileRole::Test => 0.58,
        FileRole::Docs => 0.68,
        FileRole::Benchmark => 0.55,
        FileRole::Generated => 0.42,
        FileRole::Vendor => 0.35,
    }
}

fn primary_source_path_bonus(file_path: &str, query_tokens: &[String]) -> f32 {
    let path_lower = file_path.replace('\\', "/").to_ascii_lowercase();
    let mut bonus = 0.0;
    if path_lower.contains("/src/") || path_lower.starts_with("src/") {
        bonus += 0.04;
    }
    if query_tokens
        .iter()
        .filter(|token| token.len() >= 4)
        .any(|token| path_lower.contains(token.as_str()))
    {
        bonus += 0.03;
    }
    bonus
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
        CandidateSource::Zoekt => 0,
        CandidateSource::Scip => 1,
        CandidateSource::Qdrant => 2,
        CandidateSource::Legacy => 3,
    }
}

fn cap_non_primary_below_best_primary(candidates: &mut [CandidateHit]) {
    let best_primary = candidates
        .iter()
        .filter(|candidate| {
            matches!(
                effective_file_role(candidate),
                FileRole::Source | FileRole::Entrypoint
            )
        })
        .map(|candidate| candidate.score)
        .max_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));
    let Some(best_primary) = best_primary else {
        return;
    };
    for candidate in candidates {
        if !matches!(
            effective_file_role(candidate),
            FileRole::Source | FileRole::Entrypoint
        ) && candidate.score >= best_primary
        {
            candidate.score = (best_primary - 0.001).max(0.0);
        }
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

fn strong_query_token_hits(candidate: &CandidateHit, query_tokens: &[String]) -> usize {
    let path_lower = candidate.file_path.to_ascii_lowercase();
    let symbol_lower = candidate
        .symbol_name
        .as_deref()
        .unwrap_or("")
        .to_ascii_lowercase();
    query_tokens
        .iter()
        .filter(|token| token.len() >= 4)
        .filter(|token| {
            path_lower.contains(token.as_str()) || symbol_lower.contains(token.as_str())
        })
        .count()
}

fn cap_dense_below_strong_lexical_source(candidates: &mut [CandidateHit], query_tokens: &[String]) {
    let Some((anchor_hits, anchor_score)) = candidates
        .iter()
        .filter(|candidate| candidate.source == CandidateSource::Zoekt)
        .filter(|candidate| {
            matches!(
                effective_file_role(candidate),
                FileRole::Source | FileRole::Entrypoint
            )
        })
        .filter_map(|candidate| {
            let hits = strong_query_token_hits(candidate, query_tokens);
            (hits >= 3).then_some((hits, candidate.score))
        })
        .max_by(|left, right| {
            left.1
                .partial_cmp(&right.1)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
    else {
        return;
    };

    for candidate in candidates {
        if candidate.source == CandidateSource::Qdrant
            && strong_query_token_hits(candidate, query_tokens) < anchor_hits
            && candidate.score >= anchor_score
        {
            candidate.score = (anchor_score - 0.001).max(0.0);
        }
    }
}

fn apply_exact_code_evidence_anchor(
    features: &QueryFeatures,
    query_tokens: &[String],
    candidates: &mut [CandidateHit],
) {
    if !matches!(
        features.shape,
        QueryShape::SymbolLike | QueryShape::PathLike | QueryShape::Mixed
    ) {
        return;
    }
    let top_exact_code_score = candidates
        .iter()
        .filter(|candidate| is_exact_code_evidence(candidate, query_tokens))
        .map(|candidate| candidate.score)
        .max_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));
    let Some(top_exact_code_score) = top_exact_code_score else {
        return;
    };
    for candidate in candidates.iter_mut() {
        if matches!(candidate.source, CandidateSource::Qdrant)
            && !is_exact_code_evidence(candidate, query_tokens)
            && candidate.score >= top_exact_code_score
        {
            candidate.score = (top_exact_code_score - 0.001).max(0.0);
        }
    }
}

fn is_exact_code_evidence(candidate: &CandidateHit, query_tokens: &[String]) -> bool {
    if matches!(
        candidate.source,
        CandidateSource::Qdrant | CandidateSource::Legacy
    ) {
        return false;
    }
    let Some(symbol) = candidate.symbol_name.as_deref() else {
        return exact_path_match(&candidate.file_path, query_tokens);
    };
    let symbol_tail = symbol
        .rsplit("::")
        .next()
        .unwrap_or(symbol)
        .rsplit('.')
        .next()
        .unwrap_or(symbol)
        .to_ascii_lowercase();
    query_tokens.iter().any(|token| token == &symbol_tail)
        || exact_path_match(&candidate.file_path, query_tokens)
}

fn exact_path_match(file_path: &str, query_tokens: &[String]) -> bool {
    let path = std::path::Path::new(file_path);
    let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
        return false;
    };
    let stem = stem.to_ascii_lowercase();
    query_tokens.iter().any(|token| token == &stem)
}

fn structural_fusion_multiplier(candidate: &CandidateHit, features: &RankFeatures) -> f32 {
    if !is_structural_candidate_path(&candidate.file_path) {
        return 1.0;
    }
    let mut multiplier = STRUCTURAL_BASELINE_MULTIPLIER;
    if features.token_overlap >= STRUCTURAL_DYNAMIC_TOKEN_THRESHOLD
        || features.semantic >= STRUCTURAL_DYNAMIC_SEMANTIC_THRESHOLD
    {
        multiplier *= STRUCTURAL_DYNAMIC_BOOST;
    }
    multiplier.min(1.0)
}

fn is_structural_candidate_path(file_path: &str) -> bool {
    let path = std::path::Path::new(file_path);
    if let Some(ext) = path.extension() {
        let ext_str = ext.to_string_lossy().to_ascii_lowercase();
        matches!(ext_str.as_str(), "html" | "htm" | "css" | "sql")
    } else {
        false
    }
}

fn query_has_code_intent(query_tokens: &[String]) -> bool {
    const CODE_INTENT: &[&str] = &[
        "function",
        "func",
        "method",
        "class",
        "struct",
        "trait",
        "interface",
        "enum",
        "impl",
        "def",
        "fn",
        "type",
        "module",
        "namespace",
    ];
    query_tokens
        .iter()
        .any(|token| CODE_INTENT.contains(&token.as_str()))
}

fn apply_structural_code_intent_cap(candidates: &mut [CandidateHit]) {
    let top_graph_score = candidates
        .iter()
        .filter(|candidate| !is_structural_candidate_path(&candidate.file_path))
        .map(|candidate| candidate.score)
        .max_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));
    let Some(top_graph_score) = top_graph_score else {
        return;
    };
    for candidate in candidates.iter_mut() {
        if is_structural_candidate_path(&candidate.file_path) && candidate.score > top_graph_score {
            candidate.score = top_graph_score;
        }
    }
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
        hit.source = CandidateSource::Zoekt;
        hit.file_role = Some(FileRole::Source);
        let ranked = rank_candidates(&features, vec![hit]);
        let rf = ranked[0].rank_features.as_ref().expect("features");
        assert!(rf.file_role_prior > 0.0);
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
    fn ranker_soft_prior_downweights_structural_by_default() {
        let features = classify_query("layout styles for dashboard");
        let mut structural = CandidateHit::lexical_stub("src/ui/layout.css", 0.9);
        let mut graph = CandidateHit::lexical_stub("src/app/dashboard.rs", 0.55);
        structural.source = CandidateSource::Zoekt;
        graph.source = CandidateSource::Zoekt;
        let ranked = rank_candidates(&features, vec![structural, graph]);
        assert_eq!(ranked[0].file_path, "src/app/dashboard.rs");
    }

    #[test]
    fn ranker_boosts_structural_on_strong_token_overlap() {
        let features = classify_query("primary button layout css class");
        let mut structural = CandidateHit::lexical_stub("src/ui/primary.css", 0.7);
        structural.symbol_name = Some("primary".to_string());
        structural.source = CandidateSource::Zoekt;
        let mut graph = CandidateHit::lexical_stub("src/ui/components.rs", 0.72);
        graph.source = CandidateSource::Zoekt;
        let ranked = rank_candidates(&features, vec![structural, graph]);
        assert_eq!(ranked[0].file_path, "src/ui/primary.css");
    }

    #[test]
    fn ranker_caps_structural_on_code_intent_queries() {
        let features = classify_query("UserService class method");
        let mut structural = CandidateHit::lexical_stub("schema/users.sql", 0.99);
        let mut graph = CandidateHit::lexical_stub("src/user_service.rs", 0.8);
        structural.source = CandidateSource::Zoekt;
        graph.source = CandidateSource::Zoekt;
        let ranked = rank_candidates(&features, vec![structural, graph]);
        assert_eq!(ranked[0].file_path, "src/user_service.rs");
    }

    #[test]
    fn ranker_drops_phantom_hits_by_default() {
        let features = classify_query("search pipeline");
        let candidates = vec![
            CandidateHit::with_source("zoekt:search", None, 0.9, CandidateSource::Zoekt),
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
            CandidateSource::Zoekt,
        );
        exact.file_role = Some(FileRole::Source);
        let mut semantic = CandidateHit::with_source(
            "docs/retrieval.md",
            Some("manifest overview".into()),
            0.99,
            CandidateSource::Qdrant,
        );
        semantic.file_role = Some(FileRole::Docs);

        let ranked = rank_candidates(&features, vec![semantic, exact]);
        assert_eq!(
            ranked.first().map(|hit| hit.symbol_name.as_deref()),
            Some(Some("codestory::IndexManifest"))
        );
    }

    #[test]
    fn ranker_infers_missing_roles_and_demotes_semantic_tests_for_prompts() {
        let features = classify_query("explain request json output event processing");
        let semantic_test = CandidateHit::with_source(
            "workspace/app/tests/event_processor_with_json_output.rs",
            Some("event processor json output".into()),
            0.99,
            CandidateSource::Qdrant,
        );
        let colocated_test = CandidateHit::with_source(
            "workspace/app/src/event_processor_with_jsonl_output_tests.rs",
            Some("jsonl event test output".into()),
            0.98,
            CandidateSource::Qdrant,
        );
        let source = CandidateHit::with_source(
            "workspace/app/src/event_processor.rs",
            Some("EventProcessor".into()),
            0.72,
            CandidateSource::Zoekt,
        );

        let ranked = rank_candidates(&features, vec![semantic_test, colocated_test, source]);

        assert_eq!(
            ranked.first().map(|hit| hit.file_path.as_str()),
            Some("workspace/app/src/event_processor.rs")
        );
        assert_eq!(ranked[0].file_role, Some(FileRole::Source));
        assert!(
            ranked
                .iter()
                .filter(|hit| hit.file_role == Some(FileRole::Test))
                .count()
                >= 2
        );
    }

    #[test]
    fn ranker_demotes_test_named_source_helpers_for_production_prompts() {
        let features = classify_query("explain runtime orchestration and search projection");
        let test_helper = CandidateHit::with_source(
            "crates/runtime/src/search/engine.rs",
            Some("EmbeddingRuntime::test_runtime".into()),
            0.99,
            CandidateSource::Zoekt,
        );
        let production = CandidateHit::with_source(
            "crates/runtime/src/services.rs",
            Some("IndexService::run_indexing_blocking".into()),
            0.72,
            CandidateSource::Zoekt,
        );

        let ranked = rank_candidates(&features, vec![test_helper, production]);

        assert_eq!(
            ranked.first().and_then(|hit| hit.symbol_name.as_deref()),
            Some("IndexService::run_indexing_blocking")
        );
    }

    #[test]
    fn ranker_does_not_demote_tests_when_prompt_asks_for_tests() {
        let features = classify_query("event processor tests json output");
        let semantic_test = CandidateHit::with_source(
            "workspace/app/tests/event_processor_with_json_output.rs",
            Some("event processor json output".into()),
            0.99,
            CandidateSource::Qdrant,
        );
        let source = CandidateHit::with_source(
            "workspace/app/src/event_processor.rs",
            Some("EventProcessor".into()),
            0.72,
            CandidateSource::Zoekt,
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
        let features = classify_query(
            "packet search output evidence packet indexed symbol hits retrieval shadow",
        );
        let mut source = CandidateHit::with_source(
            "crates/codestory-cli/src/output.rs",
            Some("append_search_evidence_packet".into()),
            0.92,
            CandidateSource::Zoekt,
        );
        source.file_role = Some(FileRole::Source);
        let mut dense_dto = CandidateHit::with_source(
            "crates/codestory-contracts/src/api/dto.rs",
            Some("PacketRetrievalTraceSummaryDto".into()),
            0.99,
            CandidateSource::Qdrant,
        );
        dense_dto.file_role = Some(FileRole::Source);

        let ranked = rank_candidates(&features, vec![dense_dto, source]);

        assert_eq!(
            ranked.first().map(|hit| hit.file_path.as_str()),
            Some("crates/codestory-cli/src/output.rs")
        );
    }

    #[test]
    fn ranker_keeps_broad_lexical_source_anchor_inside_resolved_window() {
        let features = classify_query(
            "packet search output evidence packet indexed symbol hits retrieval shadow",
        );
        let mut source = CandidateHit::with_source(
            "crates/codestory-runtime/src/agent/packet_evidence.rs",
            Some("decorate_search_hit_evidence".into()),
            0.82,
            CandidateSource::Zoekt,
        );
        source.file_role = Some(FileRole::Source);
        let dense_distractors = [
            (
                "PacketTraceDto",
                "crates/codestory-contracts/src/api/dto.rs",
            ),
            (
                "RetrievalTraceDto",
                "crates/codestory-contracts/src/api/retrieval.rs",
            ),
            (
                "SearchResultDto",
                "crates/codestory-contracts/src/api/search.rs",
            ),
            (
                "IndexedSymbolDto",
                "crates/codestory-contracts/src/api/symbol.rs",
            ),
            (
                "SearchShadowDto",
                "crates/codestory-contracts/src/api/shadow.rs",
            ),
        ]
        .into_iter()
        .map(|(symbol, path)| {
            let mut hit =
                CandidateHit::with_source(path, Some(symbol.into()), 0.99, CandidateSource::Qdrant);
            hit.file_role = Some(FileRole::Source);
            hit
        });

        let ranked = rank_candidates(&features, dense_distractors.chain([source]).collect());

        assert!(
            ranked.iter().take(5).any(|hit| {
                hit.file_path == "crates/codestory-runtime/src/agent/packet_evidence.rs"
                    && hit.symbol_name.as_deref() == Some("decorate_search_hit_evidence")
            }),
            "direct lexical source evidence should stay inside the resolved top-5 window: {ranked:#?}"
        );
    }

    #[test]
    fn ranker_fuses_duplicate_lane_provenance_into_rank_features() {
        let features = classify_query("how does service startup flow");
        let mut fused = CandidateHit::with_source(
            "src/service.rs",
            Some("ExtensionService".into()),
            0.85,
            CandidateSource::Zoekt,
        );
        fused.provenance = vec![
            "lexical_source".into(),
            "dense_anchor".into(),
            "graph_neighbor".into(),
        ];
        fused.scip_hop_distance = Some(1);

        let ranked = rank_candidates(&features, vec![fused]);
        let rank_features = ranked[0].rank_features.as_ref().expect("rank features");

        assert_eq!(rank_features.lexical, 0.85);
        assert_eq!(rank_features.semantic, 0.85);
        assert_eq!(rank_features.scip_distance, 0.5);
    }
}
