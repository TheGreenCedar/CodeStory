use codestory_contracts::api::{
    AgentRetrievalPolicyModeDto, AgentRetrievalPresetDto, AgentRetrievalStepDto,
    AgentRetrievalStepKindDto, AgentRetrievalStepStatusDto, AgentRetrievalSummaryFieldDto,
    AgentRetrievalTraceDto, RetrievalAnnotationDto, RetrievalShadowDto,
};
use std::time::Instant;

pub(crate) struct TraceRecorder {
    started_at: Instant,
    steps: Vec<AgentRetrievalStepDto>,
    annotations: Vec<RetrievalAnnotationDto>,
    sla_target_ms: Option<u32>,
    retrieval_shadow: Option<RetrievalShadowDto>,
}

pub(crate) struct StepToken {
    kind: AgentRetrievalStepKindDto,
    started_at: Instant,
    input: Vec<AgentRetrievalSummaryFieldDto>,
}

pub(crate) fn field<K: Into<String>, V: Into<String>>(
    key: K,
    value: V,
) -> AgentRetrievalSummaryFieldDto {
    AgentRetrievalSummaryFieldDto {
        key: key.into(),
        value: value.into(),
    }
}

impl TraceRecorder {
    pub(crate) fn new(sla_target_ms: Option<u32>) -> Self {
        Self {
            started_at: Instant::now(),
            steps: Vec::new(),
            annotations: Vec::new(),
            sla_target_ms,
            retrieval_shadow: None,
        }
    }

    pub(crate) fn set_retrieval_shadow(&mut self, shadow: RetrievalShadowDto) {
        self.retrieval_shadow = Some(shadow);
    }

    pub(crate) fn start_step(
        &mut self,
        kind: AgentRetrievalStepKindDto,
        input: Vec<AgentRetrievalSummaryFieldDto>,
    ) -> StepToken {
        StepToken {
            kind,
            started_at: Instant::now(),
            input,
        }
    }

    pub(crate) fn finish_ok(
        &mut self,
        token: StepToken,
        output: Vec<AgentRetrievalSummaryFieldDto>,
    ) {
        self.finish_with_status(token, AgentRetrievalStepStatusDto::Ok, output, None);
    }

    pub(crate) fn finish_ok_with_duration_ms(
        &mut self,
        token: StepToken,
        output: Vec<AgentRetrievalSummaryFieldDto>,
        duration_ms: u32,
    ) {
        self.finish_with_status_and_duration(
            token,
            AgentRetrievalStepStatusDto::Ok,
            output,
            None,
            Some(duration_ms),
        );
    }

    pub(crate) fn finish_skipped(
        &mut self,
        token: StepToken,
        message: impl Into<String>,
        output: Vec<AgentRetrievalSummaryFieldDto>,
    ) {
        self.finish_with_status(
            token,
            AgentRetrievalStepStatusDto::Skipped,
            output,
            Some(message.into()),
        );
    }

    pub(crate) fn finish_truncated(
        &mut self,
        token: StepToken,
        message: impl Into<String>,
        output: Vec<AgentRetrievalSummaryFieldDto>,
    ) {
        self.finish_with_status(
            token,
            AgentRetrievalStepStatusDto::Truncated,
            output,
            Some(message.into()),
        );
    }

    pub(crate) fn finish_err(&mut self, token: StepToken, message: impl Into<String>) {
        self.finish_with_status(
            token,
            AgentRetrievalStepStatusDto::Error,
            Vec::new(),
            Some(message.into()),
        );
    }

    /// Record an evidence gap: retrieval could not produce evidence the answer needed.
    ///
    /// Consumers downgrade reported confidence for every gap annotation. Use
    /// [`TraceRecorder::observe`] for routine notes about the run.
    pub(crate) fn annotate_gap(&mut self, message: impl Into<String>) {
        self.annotations.push(RetrievalAnnotationDto::gap(message));
    }

    /// Record a routine observation about the retrieval run.
    ///
    /// Observations never move reported confidence, whatever words they contain.
    pub(crate) fn observe(&mut self, message: impl Into<String>) {
        self.annotations
            .push(RetrievalAnnotationDto::observation(message));
    }

    pub(crate) fn finish(
        self,
        request_id: String,
        resolved_profile: AgentRetrievalPresetDto,
        policy_mode: AgentRetrievalPolicyModeDto,
    ) -> AgentRetrievalTraceDto {
        let total_latency_ms = self.started_at.elapsed().as_millis().min(u32::MAX as u128) as u32;
        let sla_missed = self
            .sla_target_ms
            .map(|target| total_latency_ms > target)
            .unwrap_or(false);

        AgentRetrievalTraceDto {
            request_id,
            retrieval_publication: None,
            resolved_profile,
            policy_mode,
            total_latency_ms,
            sla_target_ms: self.sla_target_ms,
            sla_missed,
            semantic_fallback_count: 0,
            semantic_fallbacks: Vec::new(),
            semantic_stage_timeout_zero_hits: 0,
            semantic_abstained_count: 0,
            annotations: self.annotations,
            packet_claim_profile_telemetry: None,
            source_freshness_telemetry: None,
            steps: self.steps,
            packet_sidecar_diagnostics: Vec::new(),
            retrieval_shadow: self.retrieval_shadow,
        }
    }

    fn finish_with_status(
        &mut self,
        token: StepToken,
        status: AgentRetrievalStepStatusDto,
        output: Vec<AgentRetrievalSummaryFieldDto>,
        message: Option<String>,
    ) {
        self.finish_with_status_and_duration(token, status, output, message, None);
    }

    fn finish_with_status_and_duration(
        &mut self,
        token: StepToken,
        status: AgentRetrievalStepStatusDto,
        output: Vec<AgentRetrievalSummaryFieldDto>,
        message: Option<String>,
        explicit_duration_ms: Option<u32>,
    ) {
        let duration_ms = if status == AgentRetrievalStepStatusDto::Skipped {
            0
        } else {
            explicit_duration_ms.unwrap_or_else(|| {
                token.started_at.elapsed().as_millis().min(u32::MAX as u128) as u32
            })
        };
        self.steps.push(AgentRetrievalStepDto {
            kind: token.kind,
            status,
            duration_ms,
            input: token.input,
            output,
            message,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codestory_contracts::api::RetrievalAnnotationKindDto;

    /// EV-6c (#1775). `TraceRecorder` is the single front door every orchestrator gap producer
    /// goes through, so the body of [`TraceRecorder::annotate_gap`] is a one-line mutation that
    /// reclassifies every genuine evidence gap the agent reports as routine telemetry. That
    /// direction *inflates* reported confidence: `agent_gap_notes` drops the annotation, the
    /// packet keeps `agent_confidence=high`, and the operator is told an answer is ready when
    /// retrieval never produced the evidence behind it.
    ///
    /// EV-6b pinned only the opposite direction (prose must not downgrade). This pins the kind
    /// on the DTO the recorder actually publishes — not on its private buffer — so swapping
    /// `RetrievalAnnotationDto::gap` for `::observation` in either entry point fails here.
    #[test]
    fn trace_recorder_publishes_gaps_as_gap_kind_and_observations_as_observation_kind() {
        let mut trace = TraceRecorder::new(Some(500));
        trace.annotate_gap("Latency-first cutoff skipped source reads.");
        trace.observe("index_freshness status=Fresh indexed_files=12");
        trace.annotate_gap(String::from(
            "Index freshness not checked: retrieval sidecar is down",
        ));

        let published = trace.finish(
            "request-ev6c".to_string(),
            AgentRetrievalPresetDto::Architecture,
            AgentRetrievalPolicyModeDto::LatencyFirst,
        );

        let classified = published
            .annotations
            .iter()
            .map(|annotation| (annotation.kind, annotation.text.as_str()))
            .collect::<Vec<_>>();
        assert_eq!(
            classified,
            vec![
                (
                    RetrievalAnnotationKindDto::Gap,
                    "Latency-first cutoff skipped source reads."
                ),
                (
                    RetrievalAnnotationKindDto::Observation,
                    "index_freshness status=Fresh indexed_files=12"
                ),
                (
                    RetrievalAnnotationKindDto::Gap,
                    "Index freshness not checked: retrieval sidecar is down"
                ),
            ],
            "annotate_gap must publish Gap and observe must publish Observation, in push order"
        );
        assert!(published.annotations[0].is_gap());
        assert!(!published.annotations[1].is_gap());
        assert!(published.annotations[2].is_gap());
        assert_eq!(
            published
                .annotations
                .iter()
                .filter(|annotation| annotation.is_gap())
                .count(),
            2,
            "two recorded evidence gaps must survive publication as gaps"
        );
    }
}
