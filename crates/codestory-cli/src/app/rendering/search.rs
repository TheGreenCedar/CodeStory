use super::hit::{build_search_hit_output, occurrences_for_hit};
use crate::args::{RepoTextMode, SearchHitOutput, SearchOutput, VerificationTargetOutput};
use codestory_contracts::api::{
    IndexFreshnessDto, NodeId, RepoTextScanStatsDto, RetrievalShadowDto, SearchHit,
    SearchQueryAssessmentDto, SourceOccurrenceDto,
};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Copy)]
pub(in crate::app) struct RepoTextOutputConfig {
    pub(in crate::app) mode: RepoTextMode,
    pub(in crate::app) enabled: bool,
}

pub(in crate::app) struct SearchOutputParts<'a> {
    pub(in crate::app) project_root: &'a std::path::Path,
    pub(in crate::app) query: &'a str,
    pub(in crate::app) retrieval: &'a codestory_contracts::api::RetrievalStateDto,
    pub(in crate::app) retrieval_shadow: Option<&'a RetrievalShadowDto>,
    pub(in crate::app) freshness: Option<&'a IndexFreshnessDto>,
    pub(in crate::app) symbol_hits: &'a [SearchHit],
    pub(in crate::app) repo_text_hits: &'a [SearchHit],
    pub(in crate::app) repo_text_stats: Option<&'a RepoTextScanStatsDto>,
    pub(in crate::app) query_assessment: Option<&'a SearchQueryAssessmentDto>,
    pub(in crate::app) search_plan: Option<&'a codestory_contracts::api::SearchPlanDto>,
    pub(in crate::app) suggestions: &'a [SearchHit],
    pub(in crate::app) occurrences_by_node: &'a HashMap<NodeId, Vec<SourceOccurrenceDto>>,
    pub(in crate::app) limit_per_source: u32,
    pub(in crate::app) repo_text: RepoTextOutputConfig,
    pub(in crate::app) explain: bool,
}

pub(in crate::app) fn build_search_output(parts: SearchOutputParts<'_>) -> SearchOutput {
    let indexed_symbol_hits = parts
        .symbol_hits
        .iter()
        .map(|hit| {
            build_search_hit_output(
                parts.project_root,
                hit,
                Some(parts.query),
                parts.explain,
                occurrences_for_hit(parts.occurrences_by_node, hit),
            )
        })
        .collect::<Vec<_>>();
    let mut duplicate_index = HashMap::new();
    for hit in &indexed_symbol_hits {
        if let Some(key) = search_hit_location_key(hit) {
            duplicate_index
                .entry(key)
                .or_insert_with(|| hit.node_id.clone());
        }
    }
    let repo_text_hits = parts
        .repo_text_hits
        .iter()
        .map(|hit| {
            let mut output = build_search_hit_output(
                parts.project_root,
                hit,
                Some(parts.query),
                parts.explain,
                &[],
            );
            if let Some(key) = search_hit_location_key(&output) {
                output.duplicate_of = duplicate_index.get(&key).cloned();
            }
            output
        })
        .collect::<Vec<_>>();
    let query_hints = search_query_hints(parts.query, &indexed_symbol_hits, &repo_text_hits);

    SearchOutput {
        query: parts.query.to_string(),
        retrieval: parts.retrieval.clone(),
        retrieval_shadow: parts.retrieval_shadow.cloned(),
        freshness: parts.freshness.cloned(),
        limit_per_source: parts.limit_per_source,
        repo_text_mode: parts.repo_text.mode,
        repo_text_enabled: parts.repo_text.enabled,
        query_assessment: parts.query_assessment.cloned(),
        search_plan: parts.search_plan.cloned(),
        explain: parts.explain,
        query_hints,
        suggestions: parts
            .suggestions
            .iter()
            .map(|hit| {
                build_search_hit_output(
                    parts.project_root,
                    hit,
                    Some(parts.query),
                    parts.explain,
                    occurrences_for_hit(parts.occurrences_by_node, hit),
                )
            })
            .collect(),
        indexed_symbol_hits,
        repo_text_hits,
        repo_text_stats: parts.repo_text_stats.cloned(),
    }
}

pub(in crate::app) fn dedupe_verification_targets(targets: &mut Vec<VerificationTargetOutput>) {
    let mut seen = HashSet::new();
    targets.retain(|target| {
        seen.insert((
            target.role.clone(),
            target.path.clone(),
            target.line,
            target.reason.clone(),
        ))
    });
}

pub(in crate::app) fn search_query_hints(
    query: &str,
    indexed_hits: &[SearchHitOutput],
    repo_text_hits: &[SearchHitOutput],
) -> Vec<String> {
    if !indexed_hits.is_empty() {
        return Vec::new();
    }
    let mut hints = Vec::new();
    if repo_text_hits.is_empty() {
        hints.push(
            "No indexed symbol or repo-text hits; try a shorter symbol name, module path, or run index --refresh full."
                .to_string(),
        );
    } else {
        hints.push(
            "Only repo-text hits matched; try a concrete identifier from an excerpt to resolve a symbol."
                .to_string(),
        );
    }
    let terms = query
        .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))
        .filter(|term| term.len() >= 3)
        .take(4)
        .collect::<Vec<_>>();
    if !terms.is_empty() {
        hints.push(format!("Possible query terms: {}", terms.join(", ")));
    }
    hints
}

pub(in crate::app) fn search_hit_location_key(hit: &SearchHitOutput) -> Option<(String, u32)> {
    Some((hit.file_path.clone()?, hit.line?))
}
