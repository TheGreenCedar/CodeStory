use super::hit::{build_search_hit_output, collect_search_hit_occurrences, occurrences_for_hit};
use crate::args::QueryResolutionOutput;
use crate::runtime;
use crate::runtime::RuntimeContext;
use codestory_contracts::api::{NodeId, SourceOccurrenceDto};
use std::collections::HashMap;
use std::path::Path;

pub(crate) fn build_query_resolution_output(
    project_root: &std::path::Path,
    target: &runtime::ResolvedTarget,
) -> QueryResolutionOutput {
    build_query_resolution_output_from_occurrences(project_root, target, &HashMap::new())
}

pub(in crate::app) fn build_query_resolution_output_with_runtime(
    runtime: &RuntimeContext,
    target: &runtime::ResolvedTarget,
) -> QueryResolutionOutput {
    let occurrences = collect_search_hit_occurrences(
        runtime,
        std::iter::once(&target.selected).chain(target.alternatives.iter()),
    );
    build_query_resolution_output_from_occurrences(&runtime.project_root, target, &occurrences)
}

pub(in crate::app) fn build_query_resolution_output_from_occurrences(
    project_root: &Path,
    target: &runtime::ResolvedTarget,
    occurrences: &HashMap<NodeId, Vec<SourceOccurrenceDto>>,
) -> QueryResolutionOutput {
    QueryResolutionOutput {
        selector: target.selector,
        requested: target.requested.clone(),
        file_filter: target
            .file_filter
            .as_deref()
            .map(crate::display::clean_path_string),
        resolved: build_search_hit_output(
            project_root,
            &target.selected,
            Some(&target.requested),
            false,
            occurrences_for_hit(occurrences, &target.selected),
        ),
        alternatives: target
            .alternatives
            .iter()
            .skip(1)
            .map(|hit| {
                build_search_hit_output(
                    project_root,
                    hit,
                    Some(&target.requested),
                    false,
                    occurrences_for_hit(occurrences, hit),
                )
            })
            .collect(),
    }
}
