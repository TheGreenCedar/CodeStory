// Deliberate residuals of the S4 agent-planning extraction (#1673). Planning
// moved to `codestory-agent`, which depends on `codestory-contracts` and
// nothing else in this workspace. Everything still declared below stays in the
// runtime on purpose, because each module needs a power the contracts-only
// planning crate must never have. The six execution residuals the epic
// ratchets:
//
// - `orchestrator`: takes `&AppController` and runs the ask/packet loop —
//   resolves targets, executes trails and searches, reads sources, and
//   assembles the answer from live retrieval results.
// - `retrieval_primary`: executes pinned-publication retrieval; reaches
//   `codestory_store::Store`, `codestory_retrieval`, and the controller's
//   active publication state.
// - `packet_batch`: batch search execution over `&AppController`, including
//   wall-clock step recording into the retrieval trace.
// - `packet_probe`: resolves probe requests against `&AppController`,
//   `target_resolution`, and `codestory_workspace` path checks.
// - `packet_search`: `impl AppController` — the batch-search entry points the
//   packet path calls on the controller itself.
// - `nucleo_policy`: thread-local execution policy read by
//   `crate::search::engine` to suppress the Nucleo full-table scan while
//   sidecar-primary retrieval runs.
//
// Runtime-owned for reasons beside `AppController` reach:
//
// - `packet_budget`: folds the runtime's step-trace summary into the budget
//   verdict.
// - `packet_capping`: leans on `packet_batch` matching and the runtime's
//   retrieval file-role classification; a confidence-pin contract names its
//   runtime path as a gap-marker producer.
// - `trace`, `trace_export`, `packet_trace` (the trace surfaces): they record
//   and export what retrieval *executed* — wall-clock steps, shadow output,
//   and the `CODESTORY_PACKET_STEP_TRACE_OUT` file write, which the planning
//   crate is forbidden to spell (`fs::write` is contract-banned there).
pub(crate) mod nucleo_policy;
pub(crate) mod orchestrator;
pub(crate) mod packet_batch;
pub(crate) mod packet_budget;
pub(crate) mod packet_candidate;
pub(crate) mod packet_capping;
pub(crate) mod packet_compiler;
pub(crate) mod packet_follow_up;

#[cfg(any(test, feature = "test-support"))]
pub(crate) mod packet_execution_record_v3;

pub(crate) mod packet_probe;
pub(crate) mod packet_search;
pub(crate) mod packet_trace;
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
    citation, packet_citations, packet_claim_profile_registry, packet_claim_profiles,
    packet_claims, packet_coverage, packet_degradation, packet_evidence, packet_evidence_carriers,
    packet_evidence_roles, packet_flow_requirements, packet_freshness, packet_obligations,
    packet_plan, packet_profile_telemetry, packet_required_probes, packet_scoring, packet_terms,
    planning, profiles,
};

pub(crate) use orchestrator::{agent_ask, agent_packet};
pub use packet_budget::enforce_packet_output_budget_for_representation;
pub use packet_follow_up::bind_packet_follow_up_program;
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
