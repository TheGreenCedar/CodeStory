use super::super::from_api_repo_text_mode;
use super::super::rendering::{
    RepoTextOutputConfig, SearchOutputParts, build_search_output, collect_search_hit_occurrences,
};
use crate::args::SearchOutput;
use crate::runtime::RuntimeContext;

pub(in crate::app) fn search_output_from_results(
    runtime: &RuntimeContext,
    search_results: &codestory_contracts::api::SearchResultsDto,
    include_score_details: bool,
) -> SearchOutput {
    let occurrences = collect_search_hit_occurrences(
        runtime,
        search_results
            .indexed_symbol_hits
            .iter()
            .chain(search_results.suggestions.iter()),
    );
    build_search_output(SearchOutputParts {
        project_root: &runtime.project_root,
        query: &search_results.query,
        retrieval: &search_results.retrieval,
        retrieval_shadow: search_results.retrieval_shadow.as_ref(),
        freshness: search_results.freshness.as_ref(),
        symbol_hits: &search_results.indexed_symbol_hits,
        repo_text_hits: &search_results.repo_text_hits,
        repo_text_stats: search_results.repo_text_stats.as_ref(),
        query_assessment: search_results.query_assessment.as_ref(),
        search_plan: search_results.search_plan.as_ref(),
        suggestions: &search_results.suggestions,
        occurrences_by_node: &occurrences,
        limit_per_source: search_results.limit_per_source,
        repo_text: RepoTextOutputConfig {
            mode: from_api_repo_text_mode(search_results.repo_text_mode),
            enabled: search_results.repo_text_enabled,
        },
        explain: include_score_details,
    })
}
