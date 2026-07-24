mod execution;
mod reporting;
mod search_adapter;
mod suite;
mod summary_decision;
mod summary_evidence;

pub(super) use execution::run_drill;
pub(super) use search_adapter::search_output_from_results;
pub(super) use suite::run_drill_suite;

#[cfg(test)]
pub(super) use execution::{
    drill_packet_anchors, drill_packet_bridges, drill_packet_citation_is_typed_resolvable,
    drill_packet_citations, drill_packet_verification_targets,
    drill_search_hit_from_packet_citation, execute_drill_packet, write_drill_outputs,
};
#[cfg(test)]
pub(super) use summary_evidence::drill_packet_claim_readiness;
