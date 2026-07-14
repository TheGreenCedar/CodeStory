pub(crate) mod citation;
#[cfg(test)]
pub(crate) mod eval_probes;
pub(crate) mod nucleo_policy;
pub(crate) mod orchestrator;
pub(crate) mod packet_batch;
pub(crate) mod packet_budget;
pub(crate) mod packet_capping;
pub(crate) mod packet_citations;
pub(crate) mod packet_claim_profiles;
pub(crate) mod packet_claims;
pub(crate) mod packet_command_profiles;
pub(crate) mod packet_evidence;
pub(crate) mod packet_evidence_roles;
pub(crate) mod packet_flow_requirements;
pub(crate) mod packet_plan;
pub(crate) mod packet_required_probes;
pub(crate) mod packet_scoring;
pub(crate) mod packet_search;
pub(crate) mod packet_source_patterns;
pub(crate) mod packet_sufficiency;
pub(crate) mod packet_terms;
pub(crate) mod packet_trace;
pub(crate) mod planning;
pub(crate) mod profiles;
pub(crate) mod retrieval_primary;
pub(crate) mod trace;
pub(crate) mod trace_export;

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
    let extra_probes = packet_plan::packet_request_extra_probes(request.extra_probes.clone());
    Ok(packet_plan::build_packet_plan_with_extra(
        question,
        request.task_class,
        request.budget,
        &extra_probes,
    ))
}
