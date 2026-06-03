use super::engine::{HybridSearchHit, search_symbols_with_scores};
use codestory_contracts::graph::NodeId;
use nucleo_matcher::Utf32String;
use std::collections::{HashMap, HashSet};

use crate::{exact_symbol_query_terms, mixed_natural_language_query, normalize_symbol_query};

pub(crate) fn lexical_hybrid_hits_for_symbols(
    symbols: &[(Utf32String, NodeId)],
    query: &str,
    graph_boosts: &HashMap<NodeId, f32>,
) -> Vec<HybridSearchHit> {
    let lexical = search_symbols_with_scores(symbols, query);
    let lexical_max = lexical
        .iter()
        .map(|(_, score)| *score)
        .fold(0.0_f32, f32::max)
        .max(1.0);
    lexical
        .into_iter()
        .map(|(node_id, score)| {
            let lexical_score = (score / lexical_max).clamp(0.0, 1.0);
            let graph_score = graph_boosts
                .get(&node_id)
                .copied()
                .unwrap_or(0.0)
                .clamp(0.0, 1.0);
            HybridSearchHit {
                node_id,
                lexical_score,
                semantic_score: 0.0,
                graph_score,
                total_score: (0.85 * lexical_score + 0.15 * graph_score).clamp(0.0, 1.0),
            }
        })
        .collect()
}

pub(crate) fn exact_symbol_merged_lexical_hybrid_hits_for_symbols(
    symbols: &[(Utf32String, NodeId)],
    query: &str,
    graph_boosts: &HashMap<NodeId, f32>,
) -> Vec<HybridSearchHit> {
    let mut hits = Vec::new();
    for term in exact_symbol_merged_lexical_queries(query) {
        let additional = lexical_hybrid_hits_for_symbols(symbols, &term, graph_boosts);
        merge_hybrid_hits_by_node_id(&mut hits, additional);
    }
    hits
}

pub(crate) fn exact_symbol_merged_lexical_queries(query: &str) -> Vec<String> {
    let mut queries = Vec::new();
    let mut seen = HashSet::new();
    push_unique_lexical_query(query, &mut queries, &mut seen);
    if mixed_natural_language_query(query) {
        return queries;
    }
    for term in exact_symbol_query_terms(query) {
        push_unique_lexical_query(&term, &mut queries, &mut seen);
    }
    queries
}

fn push_unique_lexical_query(query: &str, queries: &mut Vec<String>, seen: &mut HashSet<String>) {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return;
    }
    let normalized = normalize_symbol_query(trimmed);
    let key = if normalized.is_empty() {
        trimmed.to_ascii_lowercase()
    } else {
        normalized
    };
    if seen.insert(key) {
        queries.push(trimmed.to_string());
    }
}

fn merge_hybrid_hits_by_node_id(hits: &mut Vec<HybridSearchHit>, additional: Vec<HybridSearchHit>) {
    let mut existing = hits
        .iter()
        .enumerate()
        .map(|(index, hit)| (hit.node_id, index))
        .collect::<HashMap<_, _>>();

    for hit in additional {
        if let Some(index) = existing.get(&hit.node_id).copied() {
            let current = &mut hits[index];
            current.lexical_score = current.lexical_score.max(hit.lexical_score);
            current.semantic_score = current.semantic_score.max(hit.semantic_score);
            current.graph_score = current.graph_score.max(hit.graph_score);
            current.total_score = current.total_score.max(hit.total_score);
            continue;
        }

        existing.insert(hit.node_id, hits.len());
        hits.push(hit);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nucleo_matcher::Utf32String;

    #[test]
    fn exact_symbol_merged_lexical_hits_finds_terminal_match() {
        let symbols = vec![(Utf32String::from("run_index"), NodeId(1))];
        let hits = exact_symbol_merged_lexical_hybrid_hits_for_symbols(
            &symbols,
            "run_index",
            &HashMap::new(),
        );
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].node_id, NodeId(1));
    }
}
