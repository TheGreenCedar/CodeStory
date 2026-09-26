mod hit;
mod query;
#[allow(dead_code)]
mod search;
mod trail;

pub(super) use hit::build_numbered_search_hit_output;
pub(crate) use hit::build_search_hit_output;
pub(crate) use query::build_query_resolution_output;
pub(super) use query::{
    build_query_resolution_output_from_occurrences, build_query_resolution_output_with_runtime,
};
pub(super) use search::dedupe_verification_targets;
#[cfg(test)]
pub(super) use search::{RepoTextOutputConfig, SearchOutputParts, build_search_output};
#[cfg(test)]
pub(super) use trail::hide_speculative_trail_edges;
