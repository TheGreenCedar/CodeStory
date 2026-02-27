use codestory_api::{
    AgentRetrievalPolicyModeDto, AgentRetrievalPresetDto, AgentRetrievalStepDto,
    AgentRetrievalStepKindDto, AgentRetrievalStepStatusDto, AgentRetrievalSummaryFieldDto,
    AgentRetrievalTraceDto,
};
use std::time::Instant;

pub(crate) struct TraceRecorder {
    started_at: Instant,
    steps: Vec<AgentRetrievalStepDto>,
    annotations: Vec<String>,
    sla_target_ms: Option<u32>,
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
        }
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

    pub(crate) fn annotate(&mut self, message: impl Into<String>) {
        self.annotations.push(message.into());
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
            resolved_profile,
            policy_mode,
            total_latency_ms,
            sla_target_ms: self.sla_target_ms,
            sla_missed,
            annotations: self.annotations,
            steps: self.steps,
        }
    }

    fn finish_with_status(
        &mut self,
        token: StepToken,
        status: AgentRetrievalStepStatusDto,
        output: Vec<AgentRetrievalSummaryFieldDto>,
        message: Option<String>,
    ) {
        let duration_ms = token.started_at.elapsed().as_millis().min(u32::MAX as u128) as u32;
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
