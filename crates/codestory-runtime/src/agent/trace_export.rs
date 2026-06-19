//! Per-step packet retrieval trace export for golden scoring and latency triage.
#![allow(clippy::items_after_test_module)]

use codestory_contracts::api::{
    AgentAnswerDto, AgentRetrievalStepDto, AgentRetrievalStepKindDto, AgentRetrievalStepStatusDto,
    PacketRetrievalTraceSummaryDto,
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
    pub sidecar_query_ms: Option<u32>,
    pub candidate_resolution_ms: Option<u32>,
    pub sidecar_total_ms: Option<u32>,
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
    let hits = step_output_u32(step, "hits");
    let mode = step_output_string(step, "mode");
    PacketStepTraceRow {
        step_index: index,
        kind: format!("{:?}", step.kind),
        status: format!("{:?}", step.status),
        duration_ms: step.duration_ms,
        query,
        hits,
        mode,
        sidecar_query_ms: step_output_u32(step, "sidecar_query_ms"),
        candidate_resolution_ms: step_output_u32(step, "candidate_resolution_ms"),
        sidecar_total_ms: step_output_u32(step, "sidecar_total_ms"),
        message: step.message.clone(),
    }
}

fn step_output_string(step: &AgentRetrievalStepDto, key: &str) -> Option<String> {
    step.output
        .iter()
        .find(|field| field.key == key)
        .map(|field| field.value.clone())
}

fn step_output_u32(step: &AgentRetrievalStepDto, key: &str) -> Option<u32> {
    step_output_string(step, key).and_then(|value| value.parse::<u32>().ok())
}

pub fn packet_step_trace_json(answer: &AgentAnswerDto) -> Value {
    let rows = packet_step_trace_rows(answer);
    let attributable_rows = attributable_step_rows(&rows);
    let by_kind = aggregate_by_kind(&attributable_rows);
    let semantic_fallback_count = answer.retrieval_trace.semantic_fallback_count;
    let mut payload = json!({
        "total_latency_ms": answer.retrieval_trace.total_latency_ms,
        "attributed_step_duration_ms": attributable_step_duration_ms(&rows),
        "unattributed_trace_ms": unattributed_trace_ms(answer, &rows),
        "sla_target_ms": answer.retrieval_trace.sla_target_ms,
        "sla_missed": answer.retrieval_trace.sla_missed,
        "semantic_fallback_count": semantic_fallback_count,
        "semantic_fallbacks": answer.retrieval_trace.semantic_fallbacks,
        "step_count": rows.len(),
        "attributed_step_count": attributable_rows.len(),
        "steps": rows,
        "by_kind_ms": by_kind,
        "search_phase_summary": search_phase_summary(&attributable_rows),
        "top_cost_buckets": top_cost_buckets(&by_kind, 3),
    });
    if let Some(shadow) = &answer.retrieval_trace.retrieval_shadow {
        payload["retrieval_shadow"] = serde_json::to_value(shadow).unwrap_or(Value::Null);
    }
    if !answer.retrieval_trace.packet_sidecar_diagnostics.is_empty() {
        payload["packet_sidecar_diagnostics"] =
            serde_json::to_value(&answer.retrieval_trace.packet_sidecar_diagnostics)
                .unwrap_or(Value::Null);
    }
    payload
}

pub(crate) fn packet_retrieval_trace_summary(
    answer: &AgentAnswerDto,
) -> PacketRetrievalTraceSummaryDto {
    let mut source_read_steps = 0;
    let mut search_steps = 0;
    let mut trail_steps = 0;
    for step in &answer.retrieval_trace.steps {
        match step.kind {
            AgentRetrievalStepKindDto::SourceRead => source_read_steps += 1,
            AgentRetrievalStepKindDto::Search
            | AgentRetrievalStepKindDto::SemanticQueryEmbedding
            | AgentRetrievalStepKindDto::SemanticCandidateRetrieval
            | AgentRetrievalStepKindDto::HybridRerank
            | AgentRetrievalStepKindDto::QueryExpansion => search_steps += 1,
            AgentRetrievalStepKindDto::Trail
            | AgentRetrievalStepKindDto::Neighborhood
            | AgentRetrievalStepKindDto::TrailFilterOptions => trail_steps += 1,
            AgentRetrievalStepKindDto::NodeDetails
            | AgentRetrievalStepKindDto::NodeOccurrences
            | AgentRetrievalStepKindDto::EdgeOccurrences
            | AgentRetrievalStepKindDto::RepoTextFallback
            | AgentRetrievalStepKindDto::MermaidSynthesis
            | AgentRetrievalStepKindDto::AnswerSynthesis => {}
        }
    }

    let mut trace_summary = answer.retrieval_trace.clone();
    // The full step trace already lives under answer.retrieval_trace. Keep the
    // retrieval trace summary scalar-sized so compact packets do not serialize it twice.
    trace_summary.annotations.clear();
    trace_summary.steps.clear();

    PacketRetrievalTraceSummaryDto {
        retrieval_trace: trace_summary,
        source_read_steps,
        search_steps,
        trail_steps,
    }
}

pub(crate) fn write_packet_step_trace_from_env(answer: &AgentAnswerDto) -> Option<String> {
    let trace_path = std::env::var("CODESTORY_PACKET_STEP_TRACE_OUT").ok()?;
    let payload = match serde_json::to_string_pretty(&packet_step_trace_json(answer)) {
        Ok(payload) => payload,
        Err(error) => {
            return Some(format!(
                "packet_step_trace_out error=serialize path={} message={error}",
                trace_path
            ));
        }
    };
    match std::fs::write(&trace_path, payload) {
        Ok(()) => None,
        Err(error) => Some(format!(
            "packet_step_trace_out error=write path={} message={error}",
            trace_path
        )),
    }
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

fn unattributed_trace_ms(answer: &AgentAnswerDto, rows: &[PacketStepTraceRow]) -> u32 {
    answer
        .retrieval_trace
        .total_latency_ms
        .saturating_sub(attributable_step_duration_ms(rows))
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

fn search_phase_summary(rows: &[&PacketStepTraceRow]) -> Vec<Value> {
    let mut phases: std::collections::HashMap<String, Vec<&PacketStepTraceRow>> =
        std::collections::HashMap::new();
    for row in rows {
        if row.kind != format!("{:?}", AgentRetrievalStepKindDto::Search) {
            continue;
        }
        let phase = row
            .mode
            .clone()
            .unwrap_or_else(|| "unclassified_search".to_string());
        phases.entry(phase).or_default().push(*row);
    }
    let mut summaries = phases
        .into_iter()
        .map(|(phase, rows)| {
            let total_duration_ms = rows
                .iter()
                .map(|row| u64::from(row.duration_ms))
                .sum::<u64>();
            let top = rows.iter().max_by(|left, right| {
                left.duration_ms
                    .cmp(&right.duration_ms)
                    .then_with(|| left.query.cmp(&right.query))
            });
            json!({
                "phase": phase,
                "step_count": rows.len(),
                "total_duration_ms": total_duration_ms,
                "max_duration_ms": top.map(|row| row.duration_ms),
                "top_query": top.and_then(|row| row.query.clone()),
            })
        })
        .collect::<Vec<_>>();
    summaries.sort_by(|left, right| {
        right["total_duration_ms"]
            .as_u64()
            .cmp(&left["total_duration_ms"].as_u64())
            .then_with(|| left["phase"].as_str().cmp(&right["phase"].as_str()))
    });
    summaries
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
                packet_sidecar_diagnostics: Vec::new(),
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
    fn env_step_trace_write_error_is_reported() {
        let _lock = crate::process_env_test_lock();
        let missing_parent = std::env::temp_dir().join(format!(
            "codestory-missing-trace-parent-{}",
            std::process::id()
        ));
        let trace_path = missing_parent.join("trace.json");
        // SAFETY: this test holds the process env lock and restores the variable below.
        unsafe {
            std::env::set_var("CODESTORY_PACKET_STEP_TRACE_OUT", &trace_path);
        }

        let answer = sample_answer(Vec::new());
        let diagnostic = write_packet_step_trace_from_env(&answer)
            .expect("missing parent should produce a write diagnostic");
        assert!(
            diagnostic.starts_with("packet_step_trace_out error=write "),
            "diagnostic should report the write error: {diagnostic}"
        );
        assert!(
            diagnostic.contains(trace_path.to_string_lossy().as_ref()),
            "diagnostic should include the configured trace path: {diagnostic}"
        );

        // SAFETY: this test holds the process env lock.
        unsafe {
            std::env::remove_var("CODESTORY_PACKET_STEP_TRACE_OUT");
        }
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
