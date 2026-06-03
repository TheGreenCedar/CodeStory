//! Per-step packet retrieval trace export for golden scoring and latency triage.
#![allow(clippy::items_after_test_module)]

use codestory_contracts::api::{
    AgentAnswerDto, AgentRetrievalStepDto, AgentRetrievalStepKindDto, AgentRetrievalStepStatusDto,
};
use serde_json::{Value, json};

#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct PacketStepTraceRow {
    pub step_index: usize,
    pub kind: String,
    pub status: String,
    pub duration_ms: u32,
    pub query: Option<String>,
    pub hits: Option<u32>,
    pub mode: Option<String>,
    pub message: Option<String>,
}

pub(crate) fn packet_step_trace_rows(answer: &AgentAnswerDto) -> Vec<PacketStepTraceRow> {
    answer
        .retrieval_trace
        .steps
        .iter()
        .enumerate()
        .map(|(index, step)| packet_step_row(index, step))
        .collect()
}

fn packet_step_row(index: usize, step: &AgentRetrievalStepDto) -> PacketStepTraceRow {
    let query = step
        .input
        .iter()
        .find(|field| field.key == "query")
        .map(|field| field.value.clone());
    let hits = step
        .output
        .iter()
        .find(|field| field.key == "hits")
        .and_then(|field| field.value.parse::<u32>().ok());
    let mode = step
        .output
        .iter()
        .find(|field| field.key == "mode")
        .map(|field| field.value.clone());
    PacketStepTraceRow {
        step_index: index,
        kind: format!("{:?}", step.kind),
        status: format!("{:?}", step.status),
        duration_ms: step.duration_ms,
        query,
        hits,
        mode,
        message: step.message.clone(),
    }
}

pub fn packet_step_trace_json(answer: &AgentAnswerDto) -> Value {
    let rows = packet_step_trace_rows(answer);
    let attributable_rows = attributable_step_rows(&rows);
    let by_kind = aggregate_by_kind(&attributable_rows);
    let semantic_fallback_count = answer.retrieval_trace.semantic_fallback_count;
    let mut payload = json!({
        "total_latency_ms": answer.retrieval_trace.total_latency_ms,
        "attributed_step_duration_ms": attributable_step_duration_ms(&rows),
        "sla_target_ms": answer.retrieval_trace.sla_target_ms,
        "sla_missed": answer.retrieval_trace.sla_missed,
        "semantic_fallback_count": semantic_fallback_count,
        "semantic_fallbacks": answer.retrieval_trace.semantic_fallbacks,
        "step_count": rows.len(),
        "attributed_step_count": attributable_rows.len(),
        "steps": rows,
        "by_kind_ms": by_kind,
        "top_cost_buckets": top_cost_buckets(&by_kind, 3),
    });
    if let Some(shadow) = &answer.retrieval_trace.retrieval_shadow {
        payload["retrieval_shadow"] = serde_json::to_value(shadow).unwrap_or(Value::Null);
    }
    payload
}

fn attributable_step_rows(rows: &[PacketStepTraceRow]) -> Vec<&PacketStepTraceRow> {
    rows.iter()
        .filter(|row| row.status != format!("{:?}", AgentRetrievalStepStatusDto::Skipped))
        .collect()
}

fn attributable_step_duration_ms(rows: &[PacketStepTraceRow]) -> u32 {
    attributable_step_rows(rows)
        .iter()
        .map(|row| row.duration_ms)
        .sum()
}

fn aggregate_by_kind(rows: &[&PacketStepTraceRow]) -> serde_json::Map<String, Value> {
    let mut totals: std::collections::HashMap<String, u64> = std::collections::HashMap::new();
    for row in rows {
        *totals.entry(row.kind.clone()).or_default() += u64::from(row.duration_ms);
    }
    let mut map = serde_json::Map::new();
    for (kind, ms) in totals {
        map.insert(kind, json!(ms));
    }
    map
}

fn top_cost_buckets(by_kind: &serde_json::Map<String, Value>, limit: usize) -> Vec<Value> {
    let mut entries: Vec<(String, u64)> = by_kind
        .iter()
        .filter_map(|(kind, value)| value.as_u64().map(|ms| (kind.clone(), ms)))
        .collect();
    entries.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    entries
        .into_iter()
        .take(limit)
        .map(|(kind, ms)| json!({ "kind": kind, "duration_ms": ms }))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use codestory_contracts::api::{
        AgentAnswerDto, AgentRetrievalStepDto, AgentRetrievalStepKindDto,
        AgentRetrievalStepStatusDto, AgentRetrievalTraceDto,
    };

    fn sample_answer(steps: Vec<AgentRetrievalStepDto>) -> AgentAnswerDto {
        AgentAnswerDto {
            answer_id: "a1".to_string(),
            prompt: "q".to_string(),
            summary: "s".to_string(),
            freshness: None,
            sections: Vec::new(),
            citations: Vec::new(),
            subgraph_ids: Vec::new(),
            retrieval_version: "hybrid-v1".to_string(),
            graphs: Vec::new(),
            retrieval_trace: AgentRetrievalTraceDto {
                request_id: "r1".to_string(),
                resolved_profile: codestory_contracts::api::AgentRetrievalPresetDto::Architecture,
                policy_mode: codestory_contracts::api::AgentRetrievalPolicyModeDto::LatencyFirst,
                total_latency_ms: 30,
                sla_target_ms: None,
                sla_missed: false,
                semantic_fallback_count: 0,
                semantic_fallbacks: Vec::new(),
                steps,
                annotations: Vec::new(),
                retrieval_shadow: None,
            },
        }
    }

    #[test]
    fn packet_step_trace_json_aggregates_search_steps() {
        let answer = sample_answer(vec![
            AgentRetrievalStepDto {
                kind: AgentRetrievalStepKindDto::Search,
                status: AgentRetrievalStepStatusDto::Ok,
                duration_ms: 20,
                input: Vec::new(),
                output: Vec::new(),
                message: None,
            },
            AgentRetrievalStepDto {
                kind: AgentRetrievalStepKindDto::Trail,
                status: AgentRetrievalStepStatusDto::Ok,
                duration_ms: 10,
                input: Vec::new(),
                output: Vec::new(),
                message: None,
            },
        ]);
        assert_eq!(search_step_total_ms(&answer), 20);
        let json = packet_step_trace_json(&answer);
        assert_eq!(json["step_count"], 2);
        assert_eq!(json["attributed_step_count"], 2);
        assert_eq!(json["attributed_step_duration_ms"], 30);
        assert_eq!(json["by_kind_ms"]["Search"], 20);
    }

    #[test]
    fn skipped_steps_do_not_inflate_stage_attribution() {
        let answer = sample_answer(vec![
            AgentRetrievalStepDto {
                kind: AgentRetrievalStepKindDto::Search,
                status: AgentRetrievalStepStatusDto::Ok,
                duration_ms: 20,
                input: Vec::new(),
                output: Vec::new(),
                message: None,
            },
            AgentRetrievalStepDto {
                kind: AgentRetrievalStepKindDto::SemanticQueryEmbedding,
                status: AgentRetrievalStepStatusDto::Skipped,
                duration_ms: 0,
                input: Vec::new(),
                output: Vec::new(),
                message: Some("Hybrid retrieval disabled.".to_string()),
            },
            AgentRetrievalStepDto {
                kind: AgentRetrievalStepKindDto::HybridRerank,
                status: AgentRetrievalStepStatusDto::Ok,
                duration_ms: 5,
                input: Vec::new(),
                output: Vec::new(),
                message: None,
            },
        ]);

        let json = packet_step_trace_json(&answer);
        assert_eq!(json["attributed_step_count"], 2);
        assert_eq!(json["attributed_step_duration_ms"], 25);
        assert_eq!(json["by_kind_ms"]["SemanticQueryEmbedding"], Value::Null);
        assert_eq!(json["by_kind_ms"]["Search"], 20);
        assert_eq!(json["by_kind_ms"]["HybridRerank"], 5);
    }

    #[test]
    fn packet_step_trace_json_includes_retrieval_shadow_when_present() {
        use codestory_contracts::api::RetrievalShadowDto;

        let mut answer = sample_answer(vec![]);
        answer.retrieval_trace.retrieval_shadow = Some(RetrievalShadowDto {
            retrieval_mode: "full".to_string(),
            degraded_reason: None,
            retrieval_total_ms: 12,
            total_budget_ms: Some(1_000),
            cancel_reason: None,
            cache_hit: false,
            stage_timings: Vec::new(),
            candidates: Vec::new(),
            would_rank: vec!["src/main.rs".to_string()],
            error: None,
            candidate_count: 0,
            resolved_hit_count: 0,
            unresolved_candidate_count: 0,
            candidate_resolution_counts: Vec::new(),
        });
        let json = packet_step_trace_json(&answer);
        assert_eq!(json["retrieval_shadow"]["retrieval_mode"], "full");
        assert_eq!(json["retrieval_shadow"]["would_rank"][0], "src/main.rs");
    }

    #[test]
    fn search_step_total_ms_excludes_skipped_search_steps() {
        let answer = sample_answer(vec![
            AgentRetrievalStepDto {
                kind: AgentRetrievalStepKindDto::Search,
                status: AgentRetrievalStepStatusDto::Skipped,
                duration_ms: 0,
                input: Vec::new(),
                output: Vec::new(),
                message: Some("budget exhausted".to_string()),
            },
            AgentRetrievalStepDto {
                kind: AgentRetrievalStepKindDto::Search,
                status: AgentRetrievalStepStatusDto::Ok,
                duration_ms: 12,
                input: Vec::new(),
                output: Vec::new(),
                message: None,
            },
        ]);

        assert_eq!(search_step_total_ms(&answer), 12);
    }
}

pub(crate) fn search_step_total_ms(answer: &AgentAnswerDto) -> u32 {
    answer
        .retrieval_trace
        .steps
        .iter()
        .filter(|step| {
            step.kind == AgentRetrievalStepKindDto::Search
                && step.status != AgentRetrievalStepStatusDto::Skipped
        })
        .map(|step| step.duration_ms)
        .sum()
}
