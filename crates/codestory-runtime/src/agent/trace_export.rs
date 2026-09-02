//! Per-step packet retrieval trace export for golden scoring and latency triage.
#![allow(clippy::items_after_test_module)]

use codestory_contracts::api::{
    AgentAnswerDto, AgentRetrievalStepDto, AgentRetrievalStepKindDto, AgentRetrievalStepStatusDto,
    AgentRetrievalTraceDto, PacketRetrievalTraceSummaryDto, RetrievalAnnotationDto,
    RetrievalAnnotationKindDto,
};
use serde_json::{Value, json};

pub(crate) const PACKET_STEP_TRACE_ANNOTATION_PREFIX: &str = "packet_step_trace ";

const RETAINED_STEP_TRACE_MARKER: &str = " retained_steps_v1=";
const MAX_RETAINED_STEP_ROWS: usize = 64;
const MAX_RETAINED_STEP_PROOF_BYTES: usize = 6 * 1024;
const MAX_RETAINED_QUERY_BYTES: usize = 160;
const MAX_RETAINED_MODE_BYTES: usize = 48;
const MAX_RETAINED_MESSAGE_BYTES: usize = 160;

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
    pub batch_query_wall_ms: Option<u32>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct RetainedPacketStepTraceProof {
    #[serde(rename = "n")]
    source_step_count: usize,
    #[serde(rename = "r")]
    rows: Vec<RetainedPacketStepTraceRow>,
    #[serde(rename = "rt", default)]
    rows_truncated: bool,
    #[serde(rename = "ft", default)]
    fields_truncated: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct RetainedPacketStepTraceRow {
    #[serde(rename = "i")]
    step_index: usize,
    #[serde(rename = "k")]
    kind: AgentRetrievalStepKindDto,
    #[serde(rename = "s")]
    status: AgentRetrievalStepStatusDto,
    #[serde(rename = "d")]
    duration_ms: u32,
    #[serde(rename = "q", default, skip_serializing_if = "Option::is_none")]
    query: Option<String>,
    #[serde(rename = "h", default, skip_serializing_if = "Option::is_none")]
    hits: Option<u32>,
    #[serde(rename = "m", default, skip_serializing_if = "Option::is_none")]
    mode: Option<String>,
    #[serde(rename = "sq", default, skip_serializing_if = "Option::is_none")]
    sidecar_query_ms: Option<u32>,
    #[serde(rename = "cr", default, skip_serializing_if = "Option::is_none")]
    candidate_resolution_ms: Option<u32>,
    #[serde(rename = "st", default, skip_serializing_if = "Option::is_none")]
    sidecar_total_ms: Option<u32>,
    #[serde(rename = "bw", default, skip_serializing_if = "Option::is_none")]
    batch_query_wall_ms: Option<u32>,
    #[serde(rename = "g", default, skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

#[derive(Debug)]
struct PacketStepTraceData {
    rows: Vec<PacketStepTraceRow>,
    retention: Option<PacketStepTraceRetention>,
}

#[derive(Debug)]
struct PacketStepTraceRetention {
    source_step_count: usize,
    rows_truncated: bool,
    fields_truncated: bool,
}

#[derive(Debug)]
struct ParsedRetainedPacketStepTrace {
    search_total_ms: u32,
    proof: RetainedPacketStepTraceProof,
}

impl RetainedPacketStepTraceRow {
    fn from_step(index: usize, step: &AgentRetrievalStepDto) -> (Self, bool) {
        let (query, query_truncated) =
            bounded_optional_string(step_input_string(step, "query"), MAX_RETAINED_QUERY_BYTES);
        let (mode, mode_truncated) =
            bounded_optional_string(step_output_string(step, "mode"), MAX_RETAINED_MODE_BYTES);
        let (message, message_truncated) =
            bounded_optional_string(step.message.clone(), MAX_RETAINED_MESSAGE_BYTES);
        (
            Self {
                step_index: index,
                kind: step.kind,
                status: step.status,
                duration_ms: step.duration_ms,
                query,
                hits: step_output_u32(step, "hits"),
                mode,
                sidecar_query_ms: step_output_u32(step, "sidecar_query_ms"),
                candidate_resolution_ms: step_output_u32(step, "candidate_resolution_ms"),
                sidecar_total_ms: step_output_u32(step, "sidecar_total_ms"),
                batch_query_wall_ms: step_output_u32(step, "batch_query_wall_ms"),
                message,
            },
            query_truncated || mode_truncated || message_truncated,
        )
    }

    fn into_export_row(self) -> PacketStepTraceRow {
        PacketStepTraceRow {
            step_index: self.step_index,
            kind: format!("{:?}", self.kind),
            status: format!("{:?}", self.status),
            duration_ms: self.duration_ms,
            query: self.query,
            hits: self.hits,
            mode: self.mode,
            sidecar_query_ms: self.sidecar_query_ms,
            candidate_resolution_ms: self.candidate_resolution_ms,
            sidecar_total_ms: self.sidecar_total_ms,
            batch_query_wall_ms: self.batch_query_wall_ms,
            message: self.message,
        }
    }
}

/// Preserve a bounded, export-only proof before the public packet drops verbose trace steps.
///
/// The proof replaces the already-retained scalar annotation; it does not add a second large
/// packet field. Later steps may be appended after one budget pass, so repeated compaction folds
/// them into the same proof instead of discarding the earlier rows.
pub(crate) fn retain_packet_step_trace_for_export(trace: &mut AgentRetrievalTraceDto) -> bool {
    if trace.steps.is_empty() {
        return false;
    }

    let existing = retained_step_trace_proof(&trace.annotations).filter(|retained| {
        retained
            .proof
            .source_step_count
            .checked_add(trace.steps.len())
            .is_some()
    });
    let prior_source_step_count = existing
        .as_ref()
        .map(|retained| retained.proof.source_step_count)
        .unwrap_or_default();
    let prior_search_total_ms = existing
        .as_ref()
        .map(|retained| retained.search_total_ms)
        .unwrap_or_default();
    let live_search_total_ms = trace
        .steps
        .iter()
        .filter(|step| {
            step.kind == AgentRetrievalStepKindDto::Search
                && step.status != AgentRetrievalStepStatusDto::Skipped
        })
        .map(|step| step.duration_ms)
        .sum::<u32>();
    let live_step_count = trace.steps.len();

    let mut proof =
        existing
            .map(|retained| retained.proof)
            .unwrap_or(RetainedPacketStepTraceProof {
                source_step_count: 0,
                rows: Vec::new(),
                rows_truncated: false,
                fields_truncated: false,
            });
    let available_rows = if proof.rows_truncated {
        0
    } else {
        MAX_RETAINED_STEP_ROWS.saturating_sub(proof.rows.len())
    };
    for (offset, step) in trace.steps.iter().take(available_rows).enumerate() {
        let Some(step_index) = prior_source_step_count.checked_add(offset) else {
            proof.rows_truncated = true;
            break;
        };
        let (row, fields_truncated) = RetainedPacketStepTraceRow::from_step(step_index, step);
        proof.rows.push(row);
        proof.fields_truncated |= fields_truncated;
    }
    let Some(source_step_count) = prior_source_step_count.checked_add(live_step_count) else {
        return false;
    };
    proof.source_step_count = source_step_count;
    proof.rows_truncated |= live_step_count > available_rows;
    fit_retained_step_trace_proof(&mut proof);

    let annotation = retained_step_trace_annotation(
        prior_search_total_ms.saturating_add(live_search_total_ms),
        &proof,
    );
    trace.annotations.retain(|annotation| {
        annotation.kind != RetrievalAnnotationKindDto::Observation
            || !annotation
                .text
                .starts_with(PACKET_STEP_TRACE_ANNOTATION_PREFIX)
    });
    trace
        .annotations
        .push(RetrievalAnnotationDto::observation(annotation));
    true
}

/// Shrink an already-retained step proof to satisfy a public packet budget.
///
/// Optional diagnostic fields are discarded before any core row. If rows must be removed, the
/// source count and explicit truncation receipts remain, and rows are removed from the end so the
/// retained prefix keeps stable step indexes. The caller remeasures the complete adapter shape and
/// may call again with the remaining deficit.
pub(crate) fn compact_retained_packet_step_trace_for_budget(
    trace: &mut AgentRetrievalTraceDto,
    required_savings: usize,
) -> bool {
    let Some((annotation_index, retained)) =
        trace
            .annotations
            .iter()
            .enumerate()
            .rev()
            .find_map(|(index, annotation)| {
                parse_retained_step_trace_annotation(annotation).map(|retained| (index, retained))
            })
    else {
        return false;
    };

    let original_len = trace.annotations[annotation_index].text.len();
    let target_len = original_len.saturating_sub(required_savings.max(1));
    let search_total_ms = retained.search_total_ms;
    let mut proof = retained.proof;
    let mut changed = false;

    macro_rules! trim_optional_field {
        ($field:ident) => {
            for row in &mut proof.rows {
                if row.$field.take().is_some() {
                    changed = true;
                }
            }
            if changed {
                proof.fields_truncated = true;
            }
            if changed
                && retained_step_trace_annotation(search_total_ms, &proof).len() <= target_len
            {
                trace.annotations[annotation_index].text =
                    retained_step_trace_annotation(search_total_ms, &proof);
                return true;
            }
        };
    }

    trim_optional_field!(message);
    trim_optional_field!(query);
    trim_optional_field!(mode);
    trim_optional_field!(hits);
    trim_optional_field!(sidecar_query_ms);
    trim_optional_field!(candidate_resolution_ms);
    trim_optional_field!(sidecar_total_ms);
    trim_optional_field!(batch_query_wall_ms);

    while !proof.rows.is_empty()
        && retained_step_trace_annotation(search_total_ms, &proof).len() > target_len
    {
        proof.rows.pop();
        proof.rows_truncated = true;
        changed = true;
    }

    if !changed {
        return false;
    }
    proof.rows_truncated = proof.rows.len() < proof.source_step_count;
    trace.annotations[annotation_index].text =
        retained_step_trace_annotation(search_total_ms, &proof);
    true
}

fn retained_step_trace_annotation(
    search_total_ms: u32,
    proof: &RetainedPacketStepTraceProof,
) -> String {
    let payload = serde_json::to_string(proof)
        .expect("bounded retained packet step trace proof must serialize");
    format!(
        "{PACKET_STEP_TRACE_ANNOTATION_PREFIX}search_total_ms={search_total_ms} step_count={}{}{}",
        proof.source_step_count, RETAINED_STEP_TRACE_MARKER, payload
    )
}

fn fit_retained_step_trace_proof(proof: &mut RetainedPacketStepTraceProof) {
    if retained_step_trace_proof_len(proof) <= MAX_RETAINED_STEP_PROOF_BYTES {
        return;
    }

    proof.fields_truncated = true;
    for row in &mut proof.rows {
        row.message = None;
    }
    if retained_step_trace_proof_len(proof) <= MAX_RETAINED_STEP_PROOF_BYTES {
        return;
    }

    for row in &mut proof.rows {
        let (query, _) = bounded_optional_string(row.query.take(), MAX_RETAINED_QUERY_BYTES / 2);
        row.query = query;
    }
    if retained_step_trace_proof_len(proof) <= MAX_RETAINED_STEP_PROOF_BYTES {
        return;
    }

    for row in &mut proof.rows {
        row.query = None;
    }
    if retained_step_trace_proof_len(proof) <= MAX_RETAINED_STEP_PROOF_BYTES {
        return;
    }

    for row in &mut proof.rows {
        row.mode = None;
        row.hits = None;
        row.sidecar_query_ms = None;
        row.candidate_resolution_ms = None;
        row.sidecar_total_ms = None;
        row.batch_query_wall_ms = None;
    }
    while retained_step_trace_proof_len(proof) > MAX_RETAINED_STEP_PROOF_BYTES
        && !proof.rows.is_empty()
    {
        proof.rows.pop();
        proof.rows_truncated = true;
    }
}

fn retained_step_trace_proof_len(proof: &RetainedPacketStepTraceProof) -> usize {
    serde_json::to_vec(proof)
        .map(|payload| payload.len())
        .unwrap_or(usize::MAX)
}

fn retained_step_trace_proof(
    annotations: &[RetrievalAnnotationDto],
) -> Option<ParsedRetainedPacketStepTrace> {
    annotations
        .iter()
        .rev()
        .find_map(parse_retained_step_trace_annotation)
}

fn parse_retained_step_trace_annotation(
    annotation: &RetrievalAnnotationDto,
) -> Option<ParsedRetainedPacketStepTrace> {
    if annotation.kind != RetrievalAnnotationKindDto::Observation {
        return None;
    }
    let body = annotation
        .text
        .strip_prefix(PACKET_STEP_TRACE_ANNOTATION_PREFIX)?;
    let (header, payload) = body.split_once(RETAINED_STEP_TRACE_MARKER)?;
    if payload.len() > MAX_RETAINED_STEP_PROOF_BYTES {
        return None;
    }
    let mut fields = header.split(' ');
    let search_total_ms = fields
        .next()?
        .strip_prefix("search_total_ms=")?
        .parse::<u32>()
        .ok()?;
    let source_step_count = fields
        .next()?
        .strip_prefix("step_count=")?
        .parse::<usize>()
        .ok()?;
    if fields.next().is_some() {
        return None;
    }
    let proof = serde_json::from_str::<RetainedPacketStepTraceProof>(payload).ok()?;
    let invalid_rows = proof.rows.len() > MAX_RETAINED_STEP_ROWS
        || proof.rows.len() > proof.source_step_count
        || proof.source_step_count != source_step_count
        || proof.rows_truncated != (proof.rows.len() < proof.source_step_count)
        || proof
            .rows
            .iter()
            .enumerate()
            .any(|(expected_index, row)| row.step_index != expected_index)
        || proof
            .rows
            .iter()
            .any(|row| !retained_step_row_fields_within_bounds(row));
    if invalid_rows {
        return None;
    }
    Some(ParsedRetainedPacketStepTrace {
        search_total_ms,
        proof,
    })
}

fn retained_step_row_fields_within_bounds(row: &RetainedPacketStepTraceRow) -> bool {
    row.query
        .as_ref()
        .is_none_or(|value| value.len() <= MAX_RETAINED_QUERY_BYTES)
        && row
            .mode
            .as_ref()
            .is_none_or(|value| value.len() <= MAX_RETAINED_MODE_BYTES)
        && row
            .message
            .as_ref()
            .is_none_or(|value| value.len() <= MAX_RETAINED_MESSAGE_BYTES)
}

fn bounded_optional_string(value: Option<String>, max_bytes: usize) -> (Option<String>, bool) {
    let Some(value) = value else {
        return (None, false);
    };
    if value.len() <= max_bytes {
        return (Some(value), false);
    }

    let suffix = "...";
    let mut end = max_bytes.saturating_sub(suffix.len());
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    (Some(format!("{}{}", &value[..end], suffix)), true)
}

fn step_input_string(step: &AgentRetrievalStepDto, key: &str) -> Option<String> {
    step.input
        .iter()
        .find(|field| field.key == key)
        .map(|field| field.value.clone())
}

fn packet_step_trace_data(answer: &AgentAnswerDto) -> PacketStepTraceData {
    let retained =
        retained_step_trace_proof(&answer.retrieval_trace.annotations).filter(|retained| {
            retained
                .proof
                .source_step_count
                .checked_add(answer.retrieval_trace.steps.len())
                .is_some()
        });
    let live_index_offset = retained
        .as_ref()
        .map(|retained| retained.proof.source_step_count)
        .unwrap_or_default();
    let retention = retained.as_ref().map(|retained| PacketStepTraceRetention {
        source_step_count: retained
            .proof
            .source_step_count
            .saturating_add(answer.retrieval_trace.steps.len()),
        rows_truncated: retained.proof.rows_truncated,
        fields_truncated: retained.proof.fields_truncated,
    });
    let mut rows = retained
        .map(|retained| {
            retained
                .proof
                .rows
                .into_iter()
                .map(RetainedPacketStepTraceRow::into_export_row)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    rows.extend(
        answer
            .retrieval_trace
            .steps
            .iter()
            .enumerate()
            .filter_map(|(index, step)| {
                live_index_offset
                    .checked_add(index)
                    .map(|step_index| packet_step_row(step_index, step))
            }),
    );
    PacketStepTraceData { rows, retention }
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
        batch_query_wall_ms: step_output_u32(step, "batch_query_wall_ms"),
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

/// Export packet retrieval timing and sidecar diagnostics as JSON.
///
/// This is an observability surface for scoring and latency triage. It should not be treated as
/// proof of answer correctness or completeness.
pub fn packet_step_trace_json(answer: &AgentAnswerDto) -> Value {
    let data = packet_step_trace_data(answer);
    let rows = data.rows;
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
    if let Some(retention) = data.retention {
        payload["retained_step_trace"] = json!({
            "source_step_count": retention.source_step_count,
            "retained_step_count": rows.len(),
            "rows_truncated": retention.rows_truncated,
            "fields_truncated": retention.fields_truncated,
        });
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

/// Env-gated developer step-trace artifact. Detailed execution diagnostics
/// stay here rather than in budget-visible public annotations.
pub(crate) fn write_packet_step_trace_from_env(answer: &AgentAnswerDto) -> Option<String> {
    let trace_path =
        std::env::var(codestory_contracts::config_registry::PACKET_STEP_TRACE_OUT_ENV).ok()?;
    let trace = packet_step_trace_json(answer);
    let payload = match serde_json::to_string_pretty(&trace) {
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
            source_coverage: Vec::new(),
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
                retrieval_publication: None,
                resolved_profile: codestory_contracts::api::AgentRetrievalPresetDto::Architecture,
                policy_mode: codestory_contracts::api::AgentRetrievalPolicyModeDto::LatencyFirst,
                total_latency_ms: 30,
                sla_target_ms: None,
                sla_missed: false,
                semantic_fallback_count: 0,
                semantic_fallbacks: Vec::new(),
                semantic_stage_timeout_zero_hits: 0,
                semantic_abstained_count: 0,
                steps,
                packet_sidecar_diagnostics: Vec::new(),
                annotations: Vec::new(),
                source_freshness_telemetry: None,
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
            diagnostic_only: false,
            candidate_resolution_counts: Vec::new(),
        });
        let json = packet_step_trace_json(&answer);
        assert_eq!(json["retrieval_shadow"]["retrieval_mode"], "full");
        assert_eq!(json["retrieval_shadow"]["would_rank"][0], "src/main.rs");
    }

    #[test]
    fn unrelated_or_mistyped_marker_annotations_cannot_forge_retained_steps() {
        let step = AgentRetrievalStepDto {
            kind: AgentRetrievalStepKindDto::Search,
            status: AgentRetrievalStepStatusDto::Ok,
            duration_ms: 12,
            input: Vec::new(),
            output: Vec::new(),
            message: None,
        };
        let (row, _) = RetainedPacketStepTraceRow::from_step(0, &step);
        let payload = serde_json::to_string(&RetainedPacketStepTraceProof {
            source_step_count: 1,
            rows: vec![row],
            rows_truncated: false,
            fields_truncated: false,
        })
        .expect("serialize retained proof fixture");
        let mut answer = sample_answer(Vec::new());
        answer.retrieval_trace.annotations = vec![
            RetrievalAnnotationDto::observation(format!(
                "unrelated diagnostic{RETAINED_STEP_TRACE_MARKER}{payload}"
            )),
            RetrievalAnnotationDto::observation(format!(
                "{PACKET_STEP_TRACE_ANNOTATION_PREFIX}unstructured{RETAINED_STEP_TRACE_MARKER}{payload}"
            )),
            RetrievalAnnotationDto::gap(format!(
                "{PACKET_STEP_TRACE_ANNOTATION_PREFIX}search_total_ms=12 step_count=1{RETAINED_STEP_TRACE_MARKER}{payload}"
            )),
        ];

        let json = packet_step_trace_json(&answer);

        assert_eq!(json["step_count"], 0);
        assert_eq!(json["retained_step_trace"], Value::Null);
    }

    #[test]
    fn overflowing_retained_source_count_is_ignored_without_index_wrap() {
        let step = AgentRetrievalStepDto {
            kind: AgentRetrievalStepKindDto::Search,
            status: AgentRetrievalStepStatusDto::Ok,
            duration_ms: 12,
            input: Vec::new(),
            output: Vec::new(),
            message: None,
        };
        let payload = serde_json::to_string(&RetainedPacketStepTraceProof {
            source_step_count: usize::MAX,
            rows: Vec::new(),
            rows_truncated: true,
            fields_truncated: false,
        })
        .expect("serialize overflowing retained proof fixture");
        let mut answer = sample_answer(vec![step.clone(), step]);
        answer.retrieval_trace.annotations.push(
            RetrievalAnnotationDto::observation(format!(
                "{PACKET_STEP_TRACE_ANNOTATION_PREFIX}search_total_ms=12 step_count={}{RETAINED_STEP_TRACE_MARKER}{payload}",
                usize::MAX
            )),
        );

        let json = packet_step_trace_json(&answer);
        assert_eq!(json["step_count"], 2);
        assert_eq!(json["steps"][0]["step_index"], 0);
        assert_eq!(json["steps"][1]["step_index"], 1);
        assert_eq!(json["retained_step_trace"], Value::Null);

        assert!(retain_packet_step_trace_for_export(
            &mut answer.retrieval_trace
        ));
        answer.retrieval_trace.steps.clear();
        let rebuilt = packet_step_trace_json(&answer);
        assert_eq!(rebuilt["step_count"], 2);
        assert_eq!(rebuilt["retained_step_trace"]["source_step_count"], 2);
        assert_eq!(rebuilt["steps"][1]["step_index"], 1);
    }

    #[test]
    fn false_row_truncation_receipt_is_ignored_and_live_step_survives() {
        let step = AgentRetrievalStepDto {
            kind: AgentRetrievalStepKindDto::Search,
            status: AgentRetrievalStepStatusDto::Ok,
            duration_ms: 12,
            input: Vec::new(),
            output: Vec::new(),
            message: None,
        };
        let payload = serde_json::to_string(&RetainedPacketStepTraceProof {
            source_step_count: 0,
            rows: Vec::new(),
            rows_truncated: true,
            fields_truncated: false,
        })
        .expect("serialize false truncation receipt fixture");
        let mut answer = sample_answer(vec![step]);
        answer.retrieval_trace.annotations.push(
            RetrievalAnnotationDto::observation(format!(
                "{PACKET_STEP_TRACE_ANNOTATION_PREFIX}search_total_ms=0 step_count=0{RETAINED_STEP_TRACE_MARKER}{payload}"
            )),
        );

        let json = packet_step_trace_json(&answer);
        assert_eq!(json["step_count"], 1);
        assert_eq!(json["steps"][0]["step_index"], 0);
        assert_eq!(json["retained_step_trace"], Value::Null);

        assert!(retain_packet_step_trace_for_export(
            &mut answer.retrieval_trace
        ));
        answer.retrieval_trace.steps.clear();
        let rebuilt = packet_step_trace_json(&answer);
        assert_eq!(rebuilt["step_count"], 1);
        assert_eq!(rebuilt["retained_step_trace"]["source_step_count"], 1);
        assert_eq!(rebuilt["retained_step_trace"]["retained_step_count"], 1);
        assert_eq!(rebuilt["retained_step_trace"]["rows_truncated"], false);
        assert_eq!(rebuilt["steps"][0]["step_index"], 0);
    }

    #[test]
    fn retained_step_proof_rejects_overbound_fields_even_with_clean_receipt() {
        let step = AgentRetrievalStepDto {
            kind: AgentRetrievalStepKindDto::Search,
            status: AgentRetrievalStepStatusDto::Ok,
            duration_ms: 12,
            input: Vec::new(),
            output: Vec::new(),
            message: None,
        };

        for (field, max_bytes) in [
            ("query", MAX_RETAINED_QUERY_BYTES),
            ("mode", MAX_RETAINED_MODE_BYTES),
            ("message", MAX_RETAINED_MESSAGE_BYTES),
        ] {
            let (mut row, _) = RetainedPacketStepTraceRow::from_step(0, &step);
            let overbound = "x".repeat(max_bytes + 1);
            match field {
                "query" => row.query = Some(overbound),
                "mode" => row.mode = Some(overbound),
                "message" => row.message = Some(overbound),
                _ => unreachable!(),
            }
            let payload = serde_json::to_string(&RetainedPacketStepTraceProof {
                source_step_count: 1,
                rows: vec![row],
                rows_truncated: false,
                fields_truncated: false,
            })
            .expect("serialize overbound retained proof fixture");
            let mut answer = sample_answer(Vec::new());
            answer.retrieval_trace.annotations.push(
                RetrievalAnnotationDto::observation(format!(
                    "{PACKET_STEP_TRACE_ANNOTATION_PREFIX}search_total_ms=12 step_count=1{RETAINED_STEP_TRACE_MARKER}{payload}"
                )),
            );

            let json = packet_step_trace_json(&answer);
            assert_eq!(json["step_count"], 0, "field={field}");
            assert_eq!(json["retained_step_trace"], Value::Null, "field={field}");
        }
    }

    #[test]
    fn budget_compaction_keeps_core_rows_and_truthful_retention_receipts() {
        let steps = (0..12)
            .map(|index| AgentRetrievalStepDto {
                kind: AgentRetrievalStepKindDto::Search,
                status: AgentRetrievalStepStatusDto::Ok,
                duration_ms: 10 + index,
                input: vec![codestory_contracts::api::AgentRetrievalSummaryFieldDto {
                    key: "query".to_string(),
                    value: format!("query-{index}-{}", "q".repeat(140)),
                }],
                output: vec![
                    codestory_contracts::api::AgentRetrievalSummaryFieldDto {
                        key: "mode".to_string(),
                        value: "packet_fused_batch".to_string(),
                    },
                    codestory_contracts::api::AgentRetrievalSummaryFieldDto {
                        key: "hits".to_string(),
                        value: "7".to_string(),
                    },
                    codestory_contracts::api::AgentRetrievalSummaryFieldDto {
                        key: "sidecar_query_ms".to_string(),
                        value: "8".to_string(),
                    },
                    codestory_contracts::api::AgentRetrievalSummaryFieldDto {
                        key: "candidate_resolution_ms".to_string(),
                        value: "9".to_string(),
                    },
                ],
                message: Some(format!("step-{index}-{}", "diagnostic".repeat(14))),
            })
            .collect();
        let mut answer = sample_answer(steps);
        assert!(retain_packet_step_trace_for_export(
            &mut answer.retrieval_trace
        ));
        answer.retrieval_trace.steps.clear();

        let retained = retained_step_trace_proof(&answer.retrieval_trace.annotations)
            .expect("retained proof fixture");
        let original_len =
            retained_step_trace_annotation(retained.search_total_ms, &retained.proof).len();
        let mut expected = retained.proof.clone();
        for row in &mut expected.rows {
            row.query = None;
            row.hits = None;
            row.mode = None;
            row.sidecar_query_ms = None;
            row.candidate_resolution_ms = None;
            row.sidecar_total_ms = None;
            row.batch_query_wall_ms = None;
            row.message = None;
        }
        expected.fields_truncated = true;
        expected.rows.truncate(3);
        expected.rows_truncated = true;
        let target_len = retained_step_trace_annotation(retained.search_total_ms, &expected).len();

        assert!(compact_retained_packet_step_trace_for_budget(
            &mut answer.retrieval_trace,
            original_len.saturating_sub(target_len),
        ));

        let compacted = retained_step_trace_proof(&answer.retrieval_trace.annotations)
            .expect("compacted proof remains parseable");
        assert_eq!(compacted.proof.source_step_count, 12);
        assert_eq!(compacted.proof.rows.len(), 3);
        assert!(compacted.proof.fields_truncated);
        assert!(compacted.proof.rows_truncated);
        assert!(compacted.proof.rows.iter().all(|row| row.query.is_none()
            && row.mode.is_none()
            && row.message.is_none()
            && row.sidecar_query_ms.is_none()
            && row.candidate_resolution_ms.is_none()));
        let exported = packet_step_trace_json(&answer);
        assert_eq!(exported["step_count"], 3);
        assert_eq!(exported["steps"][0]["kind"], "Search");
        assert_eq!(exported["retained_step_trace"]["source_step_count"], 12);
        assert_eq!(exported["retained_step_trace"]["retained_step_count"], 3);
        assert_eq!(exported["retained_step_trace"]["fields_truncated"], true);
        assert_eq!(exported["retained_step_trace"]["rows_truncated"], true);
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
