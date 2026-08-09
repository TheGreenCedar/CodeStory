pub(crate) mod citation;
pub(crate) mod nucleo_policy;
pub(crate) mod orchestrator;
pub(crate) mod packet_batch;
pub(crate) mod packet_budget;
pub(crate) mod packet_capping;
pub(crate) mod packet_claim_profile_registry;

pub(crate) mod packet_claim_profiles;
pub(crate) mod packet_claims;
pub(crate) mod packet_coverage;
pub(crate) mod packet_degradation;
pub(crate) mod packet_freshness;
#[cfg(test)]
mod packet_obligations_runtime_tests;
pub(crate) mod packet_probe;
pub(crate) mod packet_profile_telemetry;
pub(crate) mod packet_search;
pub(crate) mod packet_source_patterns;
pub(crate) mod packet_sufficiency;
pub(crate) mod packet_trace;
pub(crate) mod profiles;
pub(crate) mod retrieval_primary;
pub(crate) mod trace;
pub(crate) mod trace_export;

// Planning lives in `codestory-agent`. These aliases keep the runtime's own
// module paths spelling the same names they always did, so the extraction is a
// crate move rather than a rename of every call site.
#[cfg(test)]
pub(crate) use codestory_agent::eval_probes;
#[allow(unused_imports)]
pub(crate) use codestory_agent::{
    packet_citations, packet_command_profiles, packet_evidence, packet_evidence_roles,
    packet_flow_requirements, packet_obligations, packet_plan, packet_required_probes,
    packet_scoring, packet_terms, planning,
};

pub(crate) use orchestrator::{agent_ask, agent_packet};
pub use trace_export::packet_step_trace_json;

/// Build the same bounded query plan used by `agent_packet` without executing retrieval.
pub fn plan_packet(
    request: &codestory_contracts::api::AgentPacketRequestDto,
) -> Result<codestory_contracts::api::PacketPlanDto, codestory_contracts::api::ApiError> {
    let question = request.question.trim();
    if question.is_empty() {
        return Err(codestory_contracts::api::ApiError::invalid_argument(
            "Question cannot be empty.",
        ));
    }
    codestory_contracts::api::validate_packet_probe_request(&request.probes, &request.extra_probes)
        .map_err(codestory_contracts::api::ApiError::invalid_argument)?;
    let probes =
        packet_probe::normalize_packet_probe_request(&request.probes, &request.extra_probes);
    let extra_probes = packet_probe::unresolved_packet_probe_queries(&probes);
    Ok(packet_plan::build_packet_plan_with_extra(
        question,
        request.task_class,
        request.budget,
        &extra_probes,
    ))
}
