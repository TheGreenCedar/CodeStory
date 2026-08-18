//! Verdict-visible retrieval degradation (EV-8).
//!
//! Retrieval already records, per stage, whether it finished, was cut off, or declined to run.
//! Until now none of that reached the packet verdict: a query whose dense lane ran out of budget
//! produced a shorter ranked list and nothing else, so the packet reported the same `sufficient`
//! it would have reported on a complete run. Losing evidence and having no evidence to find are
//! not the same fact, and a caller acting on the answer needs them separated.
//!
//! This module reads the stage record and answers three questions with types:
//!
//! * did the primary retrieval lose candidates it had planned to collect (`primary_truncated`)?
//! * did a query's semantic stage time out and contribute nothing (`timed_out_zero_hits`)?
//! * did a query's semantic stage decline to run at all (`abstained`)?
//!
//! The first caps the packet verdict at `partial`. The second demotes the *specific* query
//! obligation that lost its lane, so the demotion lands on the evidence that is actually missing
//! rather than on the packet as a whole. The third is counted, not blocking: an abstention on a
//! repository with no dense anchors is correct behavior, and its rate is the reconsideration
//! trigger recorded against the retrieval backend non-claim.

#[cfg(test)]
use codestory_contracts::wire::{
    RETRIEVAL_STAGE1_LEXICAL_LABEL, RETRIEVAL_STAGE2_SCIP_EXPAND_LABEL,
};
use codestory_contracts::{
    api::{AgentAnswerDto, RetrievalShadowDto, RetrievalStageTimingDto},
    wire::RETRIEVAL_STAGE1B_SEMANTIC_LABEL,
};

/// Query-level stop reason the planner takes deliberately once marginal gain flattens.
///
/// This is a planned stop with candidates already in hand, not lost evidence, so it must not be
/// read as truncation. Every other query-level cancel reason means the run ended early.
const PLANNED_STOP_CANCEL_REASON: &str = "marginal_gain";

/// A stage that never ran because an earlier signal made it unnecessary.
const STAGE_SKIPPED: &str = "skipped";
/// A stage that was still executing when its deadline passed; its candidates were dropped.
const STAGE_PENDING_AFTER_DEADLINE: &str = "pending_after_deadline";
/// A stage that never started because admission or the deadline cancelled it first.
const STAGE_CANCELLED_BEFORE_START: &str = "cancelled_before_start";
/// A stage that finished after its deadline; its candidates were dropped.
const STAGE_COMPLETED_LATE: &str = "completed_late";

/// How one query's semantic stage behaved.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SemanticStageDegradation {
    /// The semantic stage lost its budget and merged no candidate.
    pub timed_out_zero_hits: bool,
    /// The semantic stage declined to run, or produced only stub candidates.
    pub abstained: bool,
}

/// Whether a stage record means the stage's candidates were lost to a deadline.
fn stage_lost_to_deadline(stage: &RetrievalStageTimingDto) -> bool {
    matches!(
        stage.completion_status.as_str(),
        STAGE_PENDING_AFTER_DEADLINE | STAGE_CANCELLED_BEFORE_START | STAGE_COMPLETED_LATE
    )
}

fn is_semantic_stage(stage: &RetrievalStageTimingDto) -> bool {
    stage.stage == RETRIEVAL_STAGE1B_SEMANTIC_LABEL
}

/// Read one query's stage record for semantic degradation.
pub fn semantic_stage_degradation(stages: &[RetrievalStageTimingDto]) -> SemanticStageDegradation {
    let mut degradation = SemanticStageDegradation::default();
    for stage in stages.iter().filter(|stage| is_semantic_stage(stage)) {
        if stage_lost_to_deadline(stage) && stage.candidates_added == 0 {
            degradation.timed_out_zero_hits = true;
        }
        if stage.completion_status == STAGE_SKIPPED || stage.degraded {
            degradation.abstained = true;
        }
    }
    degradation
}

/// Whether the primary retrieval run ended before collecting the evidence it planned to collect.
///
/// Fails closed on the shapes that lose candidates and stays quiet on the planned marginal-gain
/// stop, so a healthy packet is not permanently demoted by normal early termination.
pub fn primary_retrieval_truncated(shadow: &RetrievalShadowDto) -> bool {
    if shadow.error.is_some() {
        return true;
    }
    if shadow
        .cancel_reason
        .as_deref()
        .is_some_and(|reason| reason != PLANNED_STOP_CANCEL_REASON)
    {
        return true;
    }
    shadow.stage_timings.iter().any(stage_lost_to_deadline)
}

/// Whether this packet's primary retrieval run is verdict-visibly truncated.
pub fn packet_primary_retrieval_truncated(answer: &AgentAnswerDto) -> bool {
    answer
        .retrieval_trace
        .retrieval_shadow
        .as_ref()
        .is_some_and(primary_retrieval_truncated)
}

/// Roll the per-query semantic degradation records up onto the packet's retrieval trace.
///
/// The counters are recomputed from the answer rather than accumulated during retrieval so that
/// budget enforcement, which may drop diagnostics and the primary shadow, cannot leave a counter
/// describing evidence the packet no longer carries.
pub fn apply_packet_semantic_degradation_counters(answer: &mut AgentAnswerDto) {
    let primary = answer
        .retrieval_trace
        .retrieval_shadow
        .as_ref()
        .map(|shadow| semantic_stage_degradation(&shadow.stage_timings))
        .unwrap_or_default();
    let mut timed_out = u32::from(primary.timed_out_zero_hits);
    let mut abstained = u32::from(primary.abstained);
    for diagnostic in &answer.retrieval_trace.packet_sidecar_diagnostics {
        timed_out =
            timed_out.saturating_add(u32::from(diagnostic.semantic_stage_timeout_zero_hits));
        abstained = abstained.saturating_add(u32::from(diagnostic.semantic_abstained));
    }
    answer.retrieval_trace.semantic_stage_timeout_zero_hits = timed_out;
    answer.retrieval_trace.semantic_abstained_count = abstained;
}

#[cfg(test)]
mod tests {
    use super::*;
    use codestory_contracts::api::{
        AgentRetrievalPolicyModeDto, AgentRetrievalPresetDto, AgentRetrievalTraceDto,
        PacketQueryCompletionDto, PacketSidecarQueryDiagnosticDto,
    };

    fn stage(
        label: &str,
        completion_status: &str,
        candidates_added: u32,
    ) -> RetrievalStageTimingDto {
        RetrievalStageTimingDto {
            stage: label.to_string(),
            deadline_ms: Some(50),
            elapsed_ms: 50,
            admission_wait_ms: None,
            queue_wait_ms: None,
            execution_ms: None,
            candidates_added,
            marginal_gain: 0.0,
            cancel_reason: None,
            cache_hit: false,
            sidecar_latency_ms: None,
            degraded: false,
            stub_reason: None,
            completion_status: completion_status.to_string(),
        }
    }

    fn shadow(stages: Vec<RetrievalStageTimingDto>) -> RetrievalShadowDto {
        RetrievalShadowDto {
            retrieval_mode: "full".to_string(),
            degraded_reason: None,
            retrieval_total_ms: 10,
            total_budget_ms: Some(100),
            cancel_reason: None,
            cache_hit: false,
            stage_timings: stages,
            candidates: Vec::new(),
            would_rank: Vec::new(),
            error: None,
            candidate_count: 0,
            resolved_hit_count: 0,
            unresolved_candidate_count: 0,
            diagnostic_only: false,
            candidate_resolution_counts: Vec::new(),
        }
    }

    #[test]
    fn semantic_deadline_loss_with_no_candidates_is_a_timeout() {
        let degradation = semantic_stage_degradation(&[stage(
            RETRIEVAL_STAGE1B_SEMANTIC_LABEL,
            "pending_after_deadline",
            0,
        )]);

        assert!(degradation.timed_out_zero_hits);
        assert!(!degradation.abstained);
    }

    #[test]
    fn semantic_deadline_loss_that_still_merged_candidates_is_not_a_zero_hit_timeout() {
        let degradation = semantic_stage_degradation(&[stage(
            RETRIEVAL_STAGE1B_SEMANTIC_LABEL,
            "completed_late",
            3,
        )]);

        assert!(!degradation.timed_out_zero_hits);
    }

    #[test]
    fn a_lexical_stage_timeout_is_not_a_semantic_timeout() {
        let degradation = semantic_stage_degradation(&[stage(
            RETRIEVAL_STAGE1_LEXICAL_LABEL,
            "pending_after_deadline",
            0,
        )]);

        assert_eq!(degradation, SemanticStageDegradation::default());
    }

    #[test]
    fn a_skipped_semantic_stage_abstains_without_timing_out() {
        let degradation =
            semantic_stage_degradation(&[stage(RETRIEVAL_STAGE1B_SEMANTIC_LABEL, "skipped", 0)]);

        assert!(degradation.abstained);
        assert!(!degradation.timed_out_zero_hits);
    }

    #[test]
    fn a_stub_only_semantic_stage_abstains() {
        let mut degraded = stage(RETRIEVAL_STAGE1B_SEMANTIC_LABEL, "completed", 2);
        degraded.degraded = true;
        degraded.stub_reason = Some("phantom_stub_hits".to_string());

        assert!(semantic_stage_degradation(&[degraded]).abstained);
    }

    #[test]
    fn a_complete_primary_run_is_not_truncated() {
        let shadow = shadow(vec![stage(RETRIEVAL_STAGE1_LEXICAL_LABEL, "completed", 5)]);

        assert!(!primary_retrieval_truncated(&shadow));
    }

    #[test]
    fn the_planned_marginal_gain_stop_is_not_truncation() {
        let mut shadow = shadow(vec![stage(RETRIEVAL_STAGE1_LEXICAL_LABEL, "completed", 5)]);
        shadow.cancel_reason = Some("marginal_gain".to_string());

        assert!(!primary_retrieval_truncated(&shadow));
    }

    #[test]
    fn a_deadline_cancel_truncates_the_primary_run() {
        let mut shadow = shadow(vec![stage(RETRIEVAL_STAGE1_LEXICAL_LABEL, "completed", 5)]);
        shadow.cancel_reason = Some("deadline".to_string());

        assert!(primary_retrieval_truncated(&shadow));
    }

    #[test]
    fn a_skipped_stage_alone_does_not_truncate_the_primary_run() {
        let shadow = shadow(vec![stage(RETRIEVAL_STAGE1B_SEMANTIC_LABEL, "skipped", 0)]);

        assert!(!primary_retrieval_truncated(&shadow));
    }

    #[test]
    fn a_stage_lost_to_its_deadline_truncates_the_primary_run() {
        let shadow = shadow(vec![stage(
            RETRIEVAL_STAGE2_SCIP_EXPAND_LABEL,
            "pending_after_deadline",
            0,
        )]);

        assert!(primary_retrieval_truncated(&shadow));
    }

    /// The three declared deadline-loss states are wire strings the retrieval layer emits, and
    /// each one means the same thing to the verdict: candidates the run had planned to collect
    /// were dropped. Naming only the first in a test leaves the other two free to be deleted from
    /// `stage_lost_to_deadline` without any suite noticing, so every state is pinned by literal.
    #[test]
    fn every_declared_deadline_loss_state_truncates_the_primary_run() {
        for completion_status in [
            "pending_after_deadline",
            "cancelled_before_start",
            "completed_late",
        ] {
            let shadow = shadow(vec![stage(
                RETRIEVAL_STAGE2_SCIP_EXPAND_LABEL,
                completion_status,
                0,
            )]);

            assert!(
                primary_retrieval_truncated(&shadow),
                "`{completion_status}` drops planned candidates and must truncate the run"
            );
        }
    }

    /// The same three states on the semantic stage, with nothing merged, are the zero-hit timeout
    /// that demotes the owning query obligation.
    #[test]
    fn every_declared_deadline_loss_state_with_no_candidates_is_a_semantic_timeout() {
        for completion_status in [
            "pending_after_deadline",
            "cancelled_before_start",
            "completed_late",
        ] {
            let degradation = semantic_stage_degradation(&[stage(
                RETRIEVAL_STAGE1B_SEMANTIC_LABEL,
                completion_status,
                0,
            )]);

            assert!(
                degradation.timed_out_zero_hits,
                "`{completion_status}` with zero merged candidates is a lost semantic lane"
            );
        }
    }

    /// The complement of the state list: a status outside it is not a deadline loss, so widening
    /// the match to every non-`completed` state would not silently pass either.
    #[test]
    fn a_completed_stage_is_not_a_deadline_loss() {
        let shadow = shadow(vec![stage(
            RETRIEVAL_STAGE2_SCIP_EXPAND_LABEL,
            "completed",
            0,
        )]);

        assert!(!primary_retrieval_truncated(&shadow));
        assert!(
            !semantic_stage_degradation(&[
                stage(RETRIEVAL_STAGE1B_SEMANTIC_LABEL, "completed", 0,)
            ])
            .timed_out_zero_hits
        );
    }

    fn counter_answer() -> AgentAnswerDto {
        AgentAnswerDto {
            source_coverage: Vec::new(),
            answer_id: "packet-degradation-test".to_string(),
            prompt: "how does activation admit a lease".to_string(),
            summary: String::new(),
            freshness: None,
            sections: Vec::new(),
            citations: Vec::new(),
            subgraph_ids: Vec::new(),
            retrieval_version: "test".to_string(),
            graphs: Vec::new(),
            retrieval_trace: AgentRetrievalTraceDto {
                request_id: "packet-degradation-test".to_string(),
                retrieval_publication: None,
                resolved_profile: AgentRetrievalPresetDto::Architecture,
                policy_mode: AgentRetrievalPolicyModeDto::LatencyFirst,
                total_latency_ms: 1,
                sla_target_ms: None,
                sla_missed: false,
                semantic_fallback_count: 0,
                semantic_fallbacks: Vec::new(),
                semantic_stage_timeout_zero_hits: 0,
                semantic_abstained_count: 0,
                annotations: Vec::new(),
                packet_claim_profile_telemetry: None,
                steps: Vec::new(),
                packet_sidecar_diagnostics: Vec::new(),
                retrieval_shadow: None,
                source_freshness_telemetry: None,
            },
        }
    }

    fn counter_diagnostic(
        query: &str,
        timed_out: bool,
        abstained: bool,
    ) -> PacketSidecarQueryDiagnosticDto {
        PacketSidecarQueryDiagnosticDto {
            query: query.to_string(),
            completion: PacketQueryCompletionDto::Completed,
            retrieval_mode: "full".to_string(),
            sidecar_query_ms: Some(1),
            candidate_resolution_ms: Some(1),
            total_elapsed_ms: Some(2),
            sidecar_stage_count: 1,
            sidecar_stage_total_ms: Some(1),
            batch_query_wall_ms: Some(2),
            candidate_count: 0,
            resolved_hit_count: 0,
            unresolved_candidate_count: 0,
            blocking_unresolved_candidate_count: 0,
            semantic_stage_timeout_zero_hits: timed_out,
            semantic_abstained: abstained,
            diagnostic: None,
        }
    }

    #[test]
    fn counters_sum_the_primary_run_and_every_sidecar_query() {
        let mut answer = counter_answer();
        answer.retrieval_trace.retrieval_shadow = Some(shadow(vec![stage(
            RETRIEVAL_STAGE1B_SEMANTIC_LABEL,
            "pending_after_deadline",
            0,
        )]));
        answer.retrieval_trace.packet_sidecar_diagnostics = vec![
            counter_diagnostic("first", true, false),
            counter_diagnostic("second", false, true),
            counter_diagnostic("third", false, false),
        ];

        apply_packet_semantic_degradation_counters(&mut answer);

        assert_eq!(
            answer.retrieval_trace.semantic_stage_timeout_zero_hits, 2,
            "the primary run counts alongside its subqueries"
        );
        assert_eq!(answer.retrieval_trace.semantic_abstained_count, 1);
    }

    #[test]
    fn counters_stay_zero_for_a_run_with_no_semantic_degradation() {
        let mut answer = counter_answer();
        answer.retrieval_trace.retrieval_shadow = Some(shadow(vec![stage(
            RETRIEVAL_STAGE1B_SEMANTIC_LABEL,
            "completed",
            6,
        )]));
        answer.retrieval_trace.packet_sidecar_diagnostics =
            vec![counter_diagnostic("first", false, false)];

        apply_packet_semantic_degradation_counters(&mut answer);

        assert_eq!(answer.retrieval_trace.semantic_stage_timeout_zero_hits, 0);
        assert_eq!(answer.retrieval_trace.semantic_abstained_count, 0);
    }
}
