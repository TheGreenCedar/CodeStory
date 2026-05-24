use crate::agent::profiles::{ResolvedProfile, TrailPlan, resolve_profile};
use crate::agent::trace::{TraceRecorder, field};
use crate::{
    AppController, FocusedSourceContext, HybridSearchScoredHit, clamp_u128_to_u32,
    fallback_mermaid, hybrid_retrieval_enabled, mermaid_flowchart, mermaid_gantt, mermaid_sequence,
};
use codestory_contracts::api::{
    AgentAnswerDto, AgentAskRequest, AgentCitationDto, AgentCustomRetrievalConfigDto,
    AgentPacketDto, AgentPacketRequestDto, AgentResponseBlockDto, AgentResponseModeDto,
    AgentResponseSectionDto, AgentRetrievalPolicyModeDto, AgentRetrievalPresetDto,
    AgentRetrievalProfileSelectionDto, AgentRetrievalStepDto, AgentRetrievalStepKindDto,
    AgentRetrievalStepStatusDto, ApiError, EdgeId, GraphArtifactDto, GraphRequest, GraphResponse,
    GroundingBudgetDto, IndexFreshnessDto, IndexFreshnessStatusDto, NodeDetailsDto,
    NodeDetailsRequest, NodeId, NodeOccurrencesRequest, PacketBenchmarkTraceDto, PacketBudgetDto,
    PacketBudgetLimitsDto, PacketBudgetModeDto, PacketBudgetUsageDto, PacketClaimDto,
    PacketPlanDto, PacketPlanQueryDto, PacketSufficiencyDto, PacketSufficiencyStatusDto,
    PacketTaskClassDto, RetrievalScoreBreakdownDto, SearchHit, SearchHitOrigin,
    SearchMatchQualityDto, SearchRepoTextMode, SearchRequest, TrailConfigDto,
    TrailFilterOptionsDto,
};
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

const DEFAULT_MAX_RESULTS: u32 = 8;
const DEFAULT_MAX_EDGES: u32 = 260;
const DEFAULT_SLA_TARGET_MS: u32 = 18_000;
const MIN_PHASE_DEADLINE_MS: u128 = 750;
const MIN_PACKET_RETRIEVAL_BUDGET_MS: u128 = 1_000;
const WEAK_INITIAL_HIT_COUNT: usize = 3;
const WEAK_INITIAL_TOP_SCORE: f32 = 0.30;
const WEAK_INITIAL_MIN_LEXICAL_ANCHOR: f32 = 0.01;
const WEAK_INITIAL_MIN_GRAPH_ANCHOR: f32 = 0.25;
const SOURCE_SNIPPET_TRUNCATION_SUFFIX: &str =
    "\n// ... source snippet truncated by investigation byte cap\n```";
const GRAPH_ARTIFACT_BUNDLE_BYTE_CAP: usize = 512 * 1024;
const RETRIEVAL_VERSION_HYBRID: &str = "hybrid-v1";
const RETRIEVAL_VERSION_LEXICAL_ROLLBACK: &str = "lexical-rollback-v1";

fn retrieval_version() -> &'static str {
    if hybrid_retrieval_enabled() {
        RETRIEVAL_VERSION_HYBRID
    } else {
        RETRIEVAL_VERSION_LEXICAL_ROLLBACK
    }
}

fn stale_freshness_annotation(freshness: &IndexFreshnessDto) -> Option<String> {
    if freshness.status != IndexFreshnessStatusDto::Stale {
        return None;
    }
    let samples = freshness
        .samples
        .iter()
        .map(|sample| format!("{:?}:{}", sample.kind, sample.path))
        .collect::<Vec<_>>();
    Some(format!(
        "Index freshness stale: changed={} new={} removed={}{}.",
        freshness.changed_file_count,
        freshness.new_file_count,
        freshness.removed_file_count,
        if samples.is_empty() {
            String::new()
        } else {
            format!(" samples={}", samples.join(", "))
        }
    ))
}

fn latency_budget_ms(req: &AgentAskRequest) -> u128 {
    req.latency_budget_ms
        .unwrap_or(DEFAULT_SLA_TARGET_MS)
        .clamp(1_000, 120_000) as u128
}

fn phase_deadline_ms(req: &AgentAskRequest, numerator: u128, denominator: u128) -> u128 {
    let budget = latency_budget_ms(req);
    let scaled = budget
        .saturating_mul(numerator)
        .checked_div(denominator.max(1))
        .unwrap_or(budget);
    scaled.max(MIN_PHASE_DEADLINE_MS).min(budget)
}

fn should_truncate_phase(
    resolved_profile: &ResolvedProfile,
    ask_started_at: Instant,
    deadline_ms: u128,
) -> bool {
    matches!(
        resolved_profile.policy_mode,
        AgentRetrievalPolicyModeDto::LatencyFirst
    ) && ask_started_at.elapsed().as_millis() > deadline_ms
}

#[derive(Debug, Clone, Default)]
struct RetrievalBundle {
    hits: Vec<SearchHit>,
    citations: Vec<AgentCitationDto>,
    graphs: Vec<GraphArtifactDto>,
    focus_node_id: Option<codestory_contracts::api::NodeId>,
    focused_node: Option<NodeDetailsDto>,
    primary_graph: Option<GraphResponse>,
    fallback_used: bool,
    repo_explanation_fallback_used: bool,
    repo_text_fallback_used: bool,
}

#[derive(Debug, Clone, Copy, Default)]
struct GraphArtifactCapStats {
    retained_bytes: usize,
    omitted_count: usize,
    truncated: bool,
}

#[derive(Debug, Clone, Copy)]
struct PacketLatencyBudget {
    started_at: Instant,
    target_ms: u128,
}

impl PacketLatencyBudget {
    fn new(requested_ms: Option<u32>) -> Self {
        Self {
            started_at: Instant::now(),
            target_ms: requested_ms
                .unwrap_or(DEFAULT_SLA_TARGET_MS)
                .clamp(1_000, 120_000) as u128,
        }
    }

    fn remaining_for_agent_ask(self) -> Option<u32> {
        let elapsed = self.started_at.elapsed().as_millis();
        if elapsed.saturating_add(MIN_PACKET_RETRIEVAL_BUDGET_MS) > self.target_ms {
            return None;
        }
        Some(clamp_u128_to_u32(self.target_ms.saturating_sub(elapsed)))
    }

    fn exhausted(self) -> bool {
        self.started_at.elapsed().as_millis() >= self.target_ms
    }

    fn apply_to_trace(self, answer: &mut AgentAnswerDto) {
        answer.retrieval_trace.sla_target_ms = Some(clamp_u128_to_u32(self.target_ms));
        if (answer.retrieval_trace.total_latency_ms as u128) > self.target_ms || self.exhausted() {
            answer.retrieval_trace.sla_missed = true;
        }
    }
}

pub(crate) fn agent_ask(
    controller: &AppController,
    req: AgentAskRequest,
) -> Result<AgentAnswerDto, ApiError> {
    let prompt = req.prompt.trim().to_string();
    if prompt.is_empty() {
        return Err(ApiError::invalid_argument("Prompt cannot be empty."));
    }

    let request_id = next_request_id();
    let resolved_profile = resolve_profile(&prompt, &req.retrieval_profile);
    let sla_target_ms = req
        .latency_budget_ms
        .unwrap_or(DEFAULT_SLA_TARGET_MS)
        .clamp(1_000, 120_000);
    let mut trace = TraceRecorder::new(Some(sla_target_ms));
    let ask_started_at = Instant::now();

    let mut bundle = execute_retrieval(
        controller,
        &req,
        &prompt,
        ask_started_at,
        &resolved_profile,
        &mut trace,
    )?;
    let freshness = match controller.index_freshness() {
        Ok(freshness) => {
            if let Some(annotation) = stale_freshness_annotation(&freshness) {
                trace.annotate(annotation);
            }
            Some(freshness)
        }
        Err(error) => {
            trace.annotate(format!("Index freshness not checked: {}", error.message));
            None
        }
    };

    let source_context = maybe_read_source_context(
        controller,
        SourceContextRequest {
            req: &req,
            prompt: &prompt,
            resolved_profile: &resolved_profile,
            ask_started_at,
            focused_node: bundle.focused_node.as_ref(),
            fallback_focus: bundle.fallback_used,
        },
        &mut trace,
    );

    let mermaid_graphs = build_mermaid_artifacts(
        &resolved_profile,
        &req,
        &prompt,
        ask_started_at,
        &bundle,
        &mut trace,
    );
    bundle.graphs.extend(mermaid_graphs);
    let graph_cap_stats = cap_graph_artifacts(&mut bundle.graphs, GRAPH_ARTIFACT_BUNDLE_BYTE_CAP);
    if graph_cap_stats.truncated {
        trace.annotate(format!(
            "Graph artifact bundle truncated at {} bytes; narrow focus or reduce trail depth for complete graph exports.",
            GRAPH_ARTIFACT_BUNDLE_BYTE_CAP
        ));
    }

    let synth_step = trace.start_step(
        AgentRetrievalStepKindDto::AnswerSynthesis,
        vec![field("citation_count", bundle.citations.len().to_string())],
    );

    let sections = build_sections(&prompt, &resolved_profile, &bundle, source_context.as_ref());

    trace.finish_ok(
        synth_step,
        vec![
            field("section_count", sections.len().to_string()),
            field("graph_count", bundle.graphs.len().to_string()),
            field(
                "graph_artifact_bytes",
                graph_cap_stats.retained_bytes.to_string(),
            ),
            field(
                "graph_artifact_byte_cap",
                GRAPH_ARTIFACT_BUNDLE_BYTE_CAP.to_string(),
            ),
            field(
                "graph_artifacts_omitted",
                graph_cap_stats.omitted_count.to_string(),
            ),
            field(
                "graph_artifact_truncated",
                graph_cap_stats.truncated.to_string(),
            ),
        ],
    );

    let mut trace_payload = trace.finish(
        request_id.clone(),
        resolved_profile.preset,
        resolved_profile.policy_mode,
    );

    if trace_payload.policy_mode == AgentRetrievalPolicyModeDto::CompletenessFirst
        && trace_payload.sla_missed
        && let Some(target_ms) = trace_payload.sla_target_ms
    {
        trace_payload.annotations.push(format!(
            "Completeness-first run exceeded SLA target ({} ms > {} ms).",
            trace_payload.total_latency_ms, target_ms
        ));
    }

    tracing::info!(
        request_id = %trace_payload.request_id,
        profile = ?trace_payload.resolved_profile,
        policy_mode = ?trace_payload.policy_mode,
        total_latency_ms = trace_payload.total_latency_ms,
        step_count = trace_payload.steps.len(),
        hit_count = bundle.hits.len(),
        graph_count = bundle.graphs.len(),
        "agent ask completed"
    );

    let summary = summarize_response(&resolved_profile, &bundle);

    Ok(AgentAnswerDto {
        answer_id: request_id,
        prompt,
        summary,
        freshness,
        sections,
        citations: bundle.citations,
        subgraph_ids: bundle
            .graphs
            .iter()
            .map(|graph| match graph {
                GraphArtifactDto::Uml { id, .. } => id.clone(),
                GraphArtifactDto::Mermaid { id, .. } => id.clone(),
            })
            .collect(),
        retrieval_version: retrieval_version().to_string(),
        graphs: bundle.graphs,
        retrieval_trace: trace_payload,
    })
}

pub(crate) fn agent_packet(
    controller: &AppController,
    req: AgentPacketRequestDto,
) -> Result<AgentPacketDto, ApiError> {
    let question = req.question.trim().to_string();
    if question.is_empty() {
        return Err(ApiError::invalid_argument("Question cannot be empty."));
    }

    let plan = build_packet_plan(&question, req.task_class);
    let limits = packet_budget_limits(req.budget);
    let packet_latency = PacketLatencyBudget::new(req.latency_budget_ms);
    let retrieval_profile = packet_retrieval_profile(Some(plan.task_class), req.budget, &limits);
    let retrieval_prompt = packet_retrieval_prompt(&question, &plan);
    let mut answer = agent_ask(
        controller,
        AgentAskRequest {
            prompt: retrieval_prompt,
            retrieval_profile,
            focus_node_id: None,
            max_results: Some(limits.max_anchors.clamp(1, 25)),
            response_mode: AgentResponseModeDto::Structured,
            latency_budget_ms: req.latency_budget_ms,
            include_evidence: req.include_evidence,
            hybrid_weights: None,
        },
    )?;
    answer
        .retrieval_trace
        .annotations
        .push(packet_plan_annotation(&plan));
    run_packet_planned_subqueries(
        controller,
        &plan,
        req.budget,
        &limits,
        req.include_evidence,
        packet_latency,
        &mut answer,
    );
    run_packet_anchor_expansion(
        controller,
        &plan,
        req.budget,
        &limits,
        req.include_evidence,
        packet_latency,
        &mut answer,
    );
    packet_latency.apply_to_trace(&mut answer);
    rank_packet_evidence(&question, &mut answer);
    append_packet_evidence_sections(&mut answer, plan.task_class, &limits);

    let budget = apply_packet_budget(&question, req.budget, limits, &mut answer);
    let sufficiency = build_packet_sufficiency(&question, plan.task_class, &answer, &budget);
    let benchmark_trace = packet_benchmark_trace(&answer);

    let mut packet = AgentPacketDto {
        packet_id: answer.answer_id.clone(),
        question,
        task_class: Some(plan.task_class),
        plan,
        answer,
        budget,
        sufficiency,
        benchmark_trace,
    };
    enforce_packet_output_budget(&mut packet);

    Ok(packet)
}

fn build_packet_plan(question: &str, requested: Option<PacketTaskClassDto>) -> PacketPlanDto {
    let task_class = requested.unwrap_or_else(|| infer_packet_task_class(question));
    let mut queries = Vec::new();
    push_packet_query(
        &mut queries,
        question,
        "original task phrasing for semantic and repo-text retrieval",
    );
    for term in extract_packet_query_terms(question) {
        push_packet_query(
            &mut queries,
            &term,
            "concrete symbol, file, route, or code term",
        );
    }
    for query in packet_symbol_probe_queries(question, task_class) {
        push_packet_query(
            &mut queries,
            &query,
            "symbol probe expanded from task wording",
        );
    }
    for query in task_class_seed_queries(task_class) {
        push_packet_query(&mut queries, query, "task-class retrieval seed");
    }
    for query in packet_concept_queries(question) {
        push_packet_query(
            &mut queries,
            &query,
            "natural-language concept from task wording",
        );
    }
    queries.truncate(32);

    let mut trace = vec![format!(
        "task_class={:?} source={}",
        task_class,
        if requested.is_some() {
            "request"
        } else {
            "heuristic"
        }
    )];
    trace.push(format!("planned_queries={}", queries.len()));

    PacketPlanDto {
        task_class,
        inferred_task_class: requested.is_none(),
        queries,
        trace,
    }
}

fn packet_symbol_probe_queries(question: &str, task_class: PacketTaskClassDto) -> Vec<String> {
    let terms = prompt_search_terms(question);
    let mut queries = Vec::new();

    push_generic_symbol_probe_queries(&terms, &mut queries);
    push_task_class_symbol_probe_queries(task_class, &mut queries);
    push_adjacent_packet_term_queries(&terms, &mut queries);

    queries.truncate(32);
    queries
}

const PACKET_QUERY_STOP_TERMS: &[&str] = &[
    "about", "answer", "change", "does", "explain", "files", "from", "have", "into", "this",
    "through", "what", "when", "where", "with",
];

fn push_generic_symbol_probe_queries(terms: &[String], queries: &mut Vec<String>) {
    for term in terms
        .iter()
        .filter(|term| term.len() >= 4 && !PACKET_QUERY_STOP_TERMS.contains(&term.as_str()))
        .take(12)
    {
        push_unique_term(queries, term);
        push_unique_term(queries, &packet_camel_case(&[term.as_str()]));
    }
}

fn push_task_class_symbol_probe_queries(task_class: PacketTaskClassDto, queries: &mut Vec<String>) {
    let class_queries = match task_class {
        PacketTaskClassDto::RouteTracing => {
            &["router", "handler", "route", "middleware", "dispatch"][..]
        }
        PacketTaskClassDto::BugLocalization => &["error", "validate"],
        PacketTaskClassDto::ChangeImpact => &["affected", "references"],
        PacketTaskClassDto::SymbolOwnership => &["references", "callers"],
        PacketTaskClassDto::EditPlanning => &["tests", "config"],
        PacketTaskClassDto::ArchitectureExplanation | PacketTaskClassDto::DataFlow => &[],
    };
    push_unique_terms(queries, class_queries);
}

fn push_adjacent_packet_term_queries(terms: &[String], queries: &mut Vec<String>) {
    for window in terms.windows(2).take(8) {
        if let [left, right] = window {
            push_unique_term(queries, &format!("{left}_{right}"));
            push_unique_term(
                queries,
                &packet_camel_case(&[left.as_str(), right.as_str()]),
            );
        }
    }
}

fn packet_concept_queries(question: &str) -> Vec<String> {
    prompt_search_terms(question)
        .into_iter()
        .filter(|term| {
            term.len() >= 4
                && !matches!(
                    term.as_str(),
                    "answer"
                        | "cite"
                        | "cites"
                        | "explain"
                        | "files"
                        | "full"
                        | "into"
                        | "moves"
                        | "support"
                        | "through"
                )
        })
        .take(8)
        .collect()
}

fn packet_camel_case(words: &[&str]) -> String {
    let mut value = String::new();
    for word in words {
        let mut chars = word.chars();
        if let Some(first) = chars.next() {
            value.push(first.to_ascii_uppercase());
            value.extend(chars.map(|ch| ch.to_ascii_lowercase()));
        }
    }
    value
}

fn infer_packet_task_class(question: &str) -> PacketTaskClassDto {
    let lower = question.to_ascii_lowercase();
    if contains_any(
        &lower,
        &["bug", "error", "failing", "failed", "broken", "crash"],
    ) {
        PacketTaskClassDto::BugLocalization
    } else if contains_any(
        &lower,
        &["impact", "affected", "regression", "risk", "blast radius"],
    ) {
        PacketTaskClassDto::ChangeImpact
    } else if contains_any(&lower, &["route", "endpoint", "handler", "api path"]) {
        PacketTaskClassDto::RouteTracing
    } else if contains_any(&lower, &["owner", "owns", "who calls", "references"]) {
        PacketTaskClassDto::SymbolOwnership
    } else if contains_any(&lower, &["data flow", "flow", "pipeline", "through"]) {
        PacketTaskClassDto::DataFlow
    } else if contains_any(
        &lower,
        &[
            "where to edit",
            "edit",
            "change",
            "modify",
            "implement",
            "add ",
        ],
    ) {
        PacketTaskClassDto::EditPlanning
    } else {
        PacketTaskClassDto::ArchitectureExplanation
    }
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

fn extract_packet_query_terms(question: &str) -> Vec<String> {
    let mut terms = Vec::new();
    let mut quoted = false;
    let mut quote = '\0';
    let mut start = 0usize;
    for (index, ch) in question.char_indices() {
        if matches!(ch, '`' | '"' | '\'') {
            if quoted && ch == quote {
                push_unique_term(&mut terms, question[start..index].trim());
                quoted = false;
            } else if !quoted {
                quoted = true;
                quote = ch;
                start = index + ch.len_utf8();
            }
        }
    }

    for token in question.split_whitespace() {
        let token = token.trim_matches(|ch: char| {
            matches!(
                ch,
                ',' | '.' | ';' | ':' | '?' | '!' | '(' | ')' | '[' | ']' | '{' | '}' | '"' | '`'
            )
        });
        if is_packet_code_like_term(token) {
            push_unique_term(&mut terms, token);
        }
    }
    terms.truncate(5);
    terms
}

fn is_packet_code_like_term(token: &str) -> bool {
    if token.len() < 3 {
        return false;
    }
    token.contains("::")
        || token.contains('/')
        || token.contains('\\')
        || token.contains('.')
        || token.contains('_')
        || token.contains('-')
        || token.chars().skip(1).any(|ch| ch.is_ascii_uppercase())
}

fn push_unique_term(terms: &mut Vec<String>, value: &str) {
    let value = value.trim();
    if value.len() < 3 {
        return;
    }
    if !terms.iter().any(|term| term.eq_ignore_ascii_case(value)) {
        terms.push(value.to_string());
    }
}

fn push_unique_terms(terms: &mut Vec<String>, values: &[&str]) {
    for value in values {
        push_unique_term(terms, value);
    }
}

fn task_class_seed_queries(task_class: PacketTaskClassDto) -> &'static [&'static str] {
    match task_class {
        PacketTaskClassDto::ArchitectureExplanation => &["architecture entrypoint", "runtime flow"],
        PacketTaskClassDto::BugLocalization => &["error path", "failure handling"],
        PacketTaskClassDto::ChangeImpact => &["affected symbols", "impacted tests"],
        PacketTaskClassDto::RouteTracing => &["route handler endpoint", "references"],
        PacketTaskClassDto::SymbolOwnership => &["definition references", "callers"],
        PacketTaskClassDto::DataFlow => &["pipeline flow", "storage handoff"],
        PacketTaskClassDto::EditPlanning => &["edit candidates", "test coverage"],
    }
}

fn push_packet_query(queries: &mut Vec<PacketPlanQueryDto>, query: &str, purpose: &str) {
    let query = query.trim();
    if query.is_empty() {
        return;
    }
    if queries
        .iter()
        .any(|existing| existing.query.eq_ignore_ascii_case(query))
    {
        return;
    }
    queries.push(PacketPlanQueryDto {
        query: query.to_string(),
        purpose: purpose.to_string(),
    });
}

fn packet_retrieval_prompt(question: &str, plan: &PacketPlanDto) -> String {
    if plan.queries.len() <= 1 {
        return question.to_string();
    }
    let mut prompt = String::from(question);
    prompt.push_str("\n\nPlanned CodeStory queries:");
    for query in &plan.queries {
        let _ = write!(prompt, "\n- {} ({})", query.query, query.purpose);
    }
    prompt
}

fn packet_plan_annotation(plan: &PacketPlanDto) -> String {
    let queries = plan
        .queries
        .iter()
        .map(|query| query.query.as_str())
        .collect::<Vec<_>>()
        .join(" | ");
    format!(
        "packet_plan task_class={:?} inferred={} queries={}",
        plan.task_class, plan.inferred_task_class, queries
    )
}

fn run_packet_planned_subqueries(
    controller: &AppController,
    plan: &PacketPlanDto,
    budget: PacketBudgetModeDto,
    limits: &PacketBudgetLimitsDto,
    include_evidence: bool,
    packet_latency: PacketLatencyBudget,
    answer: &mut AgentAnswerDto,
) {
    let limit = packet_subquery_limit(budget);
    if limit == 0 {
        answer
            .retrieval_trace
            .annotations
            .push("packet_subqueries skipped budget=tiny".to_string());
        return;
    }

    for query in plan.queries.iter().skip(1).take(limit) {
        let Some(remaining_latency_ms) = packet_latency.remaining_for_agent_ask() else {
            answer.retrieval_trace.sla_missed = true;
            answer.retrieval_trace.annotations.push(format!(
                "packet_subqueries stopped because packet latency budget {}ms was exhausted",
                packet_latency.target_ms
            ));
            break;
        };
        let retrieval_profile = packet_retrieval_profile(Some(plan.task_class), budget, limits);
        let subquery = agent_ask(
            controller,
            AgentAskRequest {
                prompt: query.query.clone(),
                retrieval_profile,
                focus_node_id: None,
                max_results: Some((limits.max_anchors / 2).clamp(1, 10)),
                response_mode: AgentResponseModeDto::Structured,
                latency_budget_ms: Some(remaining_latency_ms),
                include_evidence,
                hybrid_weights: None,
            },
        );
        match subquery {
            Ok(subanswer) => merge_packet_subanswer(answer, subanswer, query),
            Err(error) => answer.retrieval_trace.annotations.push(format!(
                "packet_subquery_failed query=`{}` error={:?}",
                query.query.replace('`', "'"),
                error
            )),
        }
        packet_latency.apply_to_trace(answer);
    }
}

fn packet_subquery_limit(budget: PacketBudgetModeDto) -> usize {
    match budget {
        PacketBudgetModeDto::Tiny => 0,
        PacketBudgetModeDto::Compact => 2,
        PacketBudgetModeDto::Standard => 4,
        PacketBudgetModeDto::Deep => 6,
    }
}

fn run_packet_anchor_expansion(
    controller: &AppController,
    plan: &PacketPlanDto,
    budget: PacketBudgetModeDto,
    limits: &PacketBudgetLimitsDto,
    include_evidence: bool,
    packet_latency: PacketLatencyBudget,
    answer: &mut AgentAnswerDto,
) {
    let query_limit = packet_anchor_probe_limit(budget);
    if query_limit == 0 {
        answer
            .retrieval_trace
            .annotations
            .push("packet_anchor_probes skipped budget=tiny".to_string());
        return;
    }

    let mut citation_keys = answer
        .citations
        .iter()
        .map(packet_citation_key)
        .collect::<HashSet<_>>();
    let per_query_limit = (limits.max_anchors / 2).clamp(2, 6) as usize;

    let queries = packet_anchor_probe_queries(plan)
        .into_iter()
        .take(query_limit)
        .collect::<Vec<_>>();
    if queries.is_empty() {
        return;
    }
    if packet_latency.exhausted() {
        answer.retrieval_trace.sla_missed = true;
        answer.retrieval_trace.annotations.push(format!(
            "packet_anchor_probes stopped because packet latency budget {}ms was exhausted",
            packet_latency.target_ms
        ));
        return;
    }

    let started_at = Instant::now();
    let result = controller.search_symbolic_packet_anchor_batch(&queries, per_query_limit);
    let duration_ms = clamp_u128_to_u32(started_at.elapsed().as_millis());
    answer.retrieval_trace.total_latency_ms = answer
        .retrieval_trace
        .total_latency_ms
        .saturating_add(duration_ms);
    match result {
        Ok(results) => {
            let per_step_duration = duration_ms / results.len().max(1) as u32;
            for (query, hits) in results {
                let mut added = 0usize;
                for hit in hits
                    .iter()
                    .filter(|hit| packet_anchor_hit_is_relevant(hit))
                    .take(3)
                {
                    let citation = to_citation_from_hit(hit, None, None, include_evidence);
                    if citation_keys.insert(packet_citation_key(&citation)) {
                        answer.citations.push(citation);
                        added = added.saturating_add(1);
                    }
                }
                answer.retrieval_trace.steps.push(AgentRetrievalStepDto {
                    kind: AgentRetrievalStepKindDto::Search,
                    status: AgentRetrievalStepStatusDto::Ok,
                    duration_ms: per_step_duration,
                    input: vec![field("query", query.clone())],
                    output: vec![
                        field("hits", hits.len().to_string()),
                        field("accepted_hits", added.to_string()),
                        field("mode", "symbolic_packet_anchor_probe"),
                    ],
                    message: Some("Packet symbol probe expanded broad task wording.".to_string()),
                });
                answer.retrieval_trace.annotations.push(format!(
                    "packet_anchor_probe query=`{}` hits={} added={}",
                    query.replace('`', "'"),
                    hits.len(),
                    added
                ));
            }
        }
        Err(error) => {
            for query in queries {
                answer.retrieval_trace.steps.push(AgentRetrievalStepDto {
                    kind: AgentRetrievalStepKindDto::Search,
                    status: AgentRetrievalStepStatusDto::Error,
                    duration_ms: 0,
                    input: vec![field("query", query.clone())],
                    output: Vec::new(),
                    message: Some(error.message.clone()),
                });
                answer.retrieval_trace.annotations.push(format!(
                    "packet_anchor_probe_failed query=`{}` error={}",
                    query.replace('`', "'"),
                    error.message
                ));
            }
        }
    }
    packet_latency.apply_to_trace(answer);
}

fn packet_anchor_probe_limit(budget: PacketBudgetModeDto) -> usize {
    match budget {
        PacketBudgetModeDto::Tiny => 0,
        PacketBudgetModeDto::Compact => 24,
        PacketBudgetModeDto::Standard => 24,
        PacketBudgetModeDto::Deep => 32,
    }
}

fn packet_anchor_probe_queries(plan: &PacketPlanDto) -> Vec<String> {
    plan.queries
        .iter()
        .skip(1)
        .filter(|query| {
            query.purpose.contains("symbol probe")
                || query.purpose.contains("concrete symbol")
                || is_packet_code_like_term(&query.query)
        })
        .map(|query| query.query.clone())
        .collect()
}

fn packet_anchor_hit_is_relevant(hit: &SearchHit) -> bool {
    if hit.origin != SearchHitOrigin::IndexedSymbol || !hit.resolvable {
        return false;
    }
    matches!(
        hit.match_quality,
        Some(
            SearchMatchQualityDto::Exact
                | SearchMatchQualityDto::NormalizedExact
                | SearchMatchQualityDto::Prefix
        )
    ) || hit
        .score_breakdown
        .as_ref()
        .is_some_and(|breakdown| breakdown.lexical >= 0.25 || breakdown.graph >= 0.25)
}

fn merge_packet_subanswer(
    answer: &mut AgentAnswerDto,
    subanswer: AgentAnswerDto,
    query: &PacketPlanQueryDto,
) {
    answer.retrieval_trace.total_latency_ms = answer
        .retrieval_trace
        .total_latency_ms
        .saturating_add(subanswer.retrieval_trace.total_latency_ms);
    answer.retrieval_trace.sla_missed |= subanswer.retrieval_trace.sla_missed;
    answer.retrieval_trace.annotations.push(format!(
        "packet_subquery query=`{}` purpose=`{}` citations={} sections={}",
        query.query.replace('`', "'"),
        query.purpose.replace('`', "'"),
        subanswer.citations.len(),
        subanswer.sections.len()
    ));
    answer
        .retrieval_trace
        .steps
        .extend(subanswer.retrieval_trace.steps);

    extend_unique_citations(&mut answer.citations, subanswer.citations);
    extend_unique_strings(&mut answer.subgraph_ids, subanswer.subgraph_ids);
    extend_unique_graphs(&mut answer.graphs, subanswer.graphs);

    answer.sections.push(AgentResponseSectionDto {
        id: format!("packet-subquery-{}", sanitize_section_id(&query.query)),
        title: format!("Planned query: {}", query.query),
        blocks: vec![AgentResponseBlockDto::Markdown {
            markdown: format!(
                "Purpose: {}\n\nSummary: {}\n\nUse the packet citations and retrieval trace for exact files, symbols, and confidence.",
                query.purpose, subanswer.summary
            ),
        }],
    });
}

fn packet_citation_key(citation: &AgentCitationDto) -> String {
    format!(
        "{}\t{}\t{}",
        citation.node_id.0,
        citation.file_path.as_deref().unwrap_or_default(),
        citation.line.unwrap_or_default()
    )
}

fn graph_artifact_id(graph: &GraphArtifactDto) -> String {
    match graph {
        GraphArtifactDto::Uml { id, .. } | GraphArtifactDto::Mermaid { id, .. } => id.clone(),
    }
}

fn extend_unique_citations(
    citations: &mut Vec<AgentCitationDto>,
    additional: Vec<AgentCitationDto>,
) {
    let mut keys = citations
        .iter()
        .map(packet_citation_key)
        .collect::<HashSet<_>>();
    for citation in additional {
        if keys.insert(packet_citation_key(&citation)) {
            citations.push(citation);
        }
    }
}

fn extend_unique_strings(values: &mut Vec<String>, additional: Vec<String>) {
    let mut seen = values.iter().cloned().collect::<HashSet<_>>();
    for value in additional {
        if seen.insert(value.clone()) {
            values.push(value);
        }
    }
}

fn extend_unique_graphs(graphs: &mut Vec<GraphArtifactDto>, additional: Vec<GraphArtifactDto>) {
    let mut ids = graphs.iter().map(graph_artifact_id).collect::<HashSet<_>>();
    for graph in additional {
        if ids.insert(graph_artifact_id(&graph)) {
            graphs.push(graph);
        }
    }
}

fn rank_packet_evidence(question: &str, answer: &mut AgentAnswerDto) {
    let terms = packet_rank_terms(question);
    answer.citations.sort_by(|left, right| {
        packet_citation_rank(right, &terms)
            .partial_cmp(&packet_citation_rank(left, &terms))
            .unwrap_or(Ordering::Equal)
    });
}

fn packet_rank_terms(question: &str) -> Vec<String> {
    let mut terms = prompt_search_terms(question);
    for query in packet_symbol_probe_queries(question, infer_packet_task_class(question)) {
        push_unique_term(&mut terms, &normalize_identifier(&query));
    }
    terms
}

fn packet_citation_rank(citation: &AgentCitationDto, terms: &[String]) -> f32 {
    let display = citation.display_name.to_ascii_lowercase();
    let normalized_display = normalize_identifier(&citation.display_name);
    let path = citation
        .file_path
        .as_deref()
        .map(packet_display_path)
        .unwrap_or_default()
        .to_ascii_lowercase();

    let mut score = citation.score;
    if citation.origin == SearchHitOrigin::IndexedSymbol {
        score += 1.0;
    }
    if citation.resolvable {
        score += 0.5;
    }
    if display.contains("::") {
        score += 0.25;
    }
    if path.contains("/benches/") || path_contains_test_segment(&path) || path.contains("__tests__")
    {
        score -= 20.0;
    }
    if path.contains("/lib/") || path.starts_with("lib/") {
        score += 2.0;
    }
    if let Some(breakdown) = citation.retrieval_score_breakdown.as_ref() {
        score += breakdown.lexical * 2.0;
        score += breakdown.graph;
    }

    for term in terms {
        if term.len() < 3 {
            continue;
        }
        let normalized_term = normalize_identifier(term);
        if !normalized_term.is_empty() && normalized_display.contains(&normalized_term) {
            score += 1.25;
        }
        if path.contains(term) {
            score += 0.5;
        }
    }

    score
}

fn normalize_identifier(value: &str) -> String {
    value
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(|ch| ch.to_lowercase())
        .collect()
}

fn append_packet_evidence_sections(
    answer: &mut AgentAnswerDto,
    _task_class: PacketTaskClassDto,
    limits: &PacketBudgetLimitsDto,
) {
    if answer.citations.is_empty() {
        return;
    }

    let ledger_markdown = packet_evidence_ledger_markdown(answer, limits);
    answer.sections.insert(
        0,
        AgentResponseSectionDto {
            id: "packet-evidence-ledger".to_string(),
            title: "Packet Evidence Ledger".to_string(),
            blocks: vec![AgentResponseBlockDto::Markdown {
                markdown: ledger_markdown,
            }],
        },
    );

    let claims = packet_supported_claims(answer);
    if !claims.is_empty() {
        answer.sections.insert(
            1,
            AgentResponseSectionDto {
                id: "packet-flow-claims".to_string(),
                title: "Packet Claims".to_string(),
                blocks: vec![AgentResponseBlockDto::Markdown {
                    markdown: packet_flow_claims_markdown(&claims),
                }],
            },
        );
    }
}

fn packet_evidence_ledger_markdown(
    answer: &AgentAnswerDto,
    limits: &PacketBudgetLimitsDto,
) -> String {
    let mut markdown = String::new();
    markdown.push_str(
        "Use these cited anchors first. They are ranked for the task wording before lower-confidence retrieval diagnostics.\n",
    );
    for citation in answer.citations.iter().take(limits.max_anchors as usize) {
        let _ = writeln!(markdown, "{}", packet_evidence_ledger_row(citation));
    }
    markdown
}

fn packet_evidence_ledger_row(citation: &AgentCitationDto) -> String {
    let path = citation
        .file_path
        .as_deref()
        .map(packet_display_path)
        .unwrap_or_else(|| "<unknown path>".to_string());
    let line = citation
        .line
        .map(|line| format!(":{line}"))
        .unwrap_or_default();
    let role = packet_evidence_role(citation).unwrap_or("supporting evidence");
    format!(
        "- `{}` ({:?}) - `{}`{} - {} - score {:.3}",
        citation.display_name, citation.kind, path, line, role, citation.score
    )
}

fn packet_flow_claims_markdown(claims: &[PacketClaimDto]) -> String {
    let mut markdown = String::new();
    markdown.push_str("Supported claims for a compact agent answer:\n");
    for claim in claims {
        let citation = claim.citations.first();
        let suffix = citation
            .and_then(|citation| citation.file_path.as_deref())
            .map(packet_display_path)
            .map(|path| format!(" (`{path}`)"))
            .unwrap_or_default();
        let _ = writeln!(markdown, "- {}{}", claim.claim, suffix);
    }
    markdown
}

fn packet_supported_claims(answer: &AgentAnswerDto) -> Vec<PacketClaimDto> {
    let mut claims = Vec::new();
    let mut seen_roles = HashSet::new();
    for citation in &answer.citations {
        let Some(role) = packet_evidence_role(citation) else {
            continue;
        };
        if !seen_roles.insert(role) {
            continue;
        }
        claims.push(PacketClaimDto {
            claim: packet_claim_for_role(role, citation),
            citations: vec![citation.clone()],
        });
        if claims.len() >= 12 {
            break;
        }
    }
    claims
}

fn packet_evidence_role(citation: &AgentCitationDto) -> Option<&'static str> {
    let display = citation.display_name.to_ascii_lowercase();
    let path = citation
        .file_path
        .as_deref()
        .map(packet_display_path)
        .unwrap_or_default()
        .to_ascii_lowercase();

    if path_contains_test_segment(&path) || path.ends_with("_test.go") || path.ends_with(".test.ts")
    {
        Some("tests and regression coverage")
    } else if display.contains("cli") || display.contains("command") || path.contains("/cli") {
        Some("command entrypoint")
    } else if display.contains("service")
        || display.contains("orchestrat")
        || display.contains("runtime")
        || path.contains("runtime")
    {
        Some("runtime orchestration")
    } else if display.contains("manifest") || display.contains("plan") || path.contains("workspace")
    {
        Some("workspace discovery and planning")
    } else if display.contains("snapshot") || display.contains("refresh") {
        Some("snapshot refresh")
    } else if display.contains("projection")
        || display.contains("persist")
        || display.contains("storage")
        || display.contains("store")
        || path.contains("store")
    {
        Some("persistence and search projection")
    } else if display.contains("indexer")
        || display.contains("index_file")
        || display.contains("symbol")
        || path.contains("indexer")
    {
        Some("symbol extraction")
    } else if display.contains("route") || display.contains("handler") || display.contains("router")
    {
        Some("route handling")
    } else {
        None
    }
}

fn packet_claim_for_role(role: &str, citation: &AgentCitationDto) -> String {
    let symbol = citation.display_name.as_str();
    match role {
        "command entrypoint" => format!(
            "The command or public entrypoint for this flow is anchored by `{symbol}`; inspect it before following downstream coordination."
        ),
        "runtime orchestration" => format!(
            "Runtime orchestration is anchored by `{symbol}`; verify coordination, state transitions, and downstream service calls there."
        ),
        "workspace discovery and planning" => format!(
            "Workspace discovery or planning is anchored by `{symbol}`; inspect it for file selection, manifest, or execution-plan behavior."
        ),
        "symbol extraction" => format!(
            "Symbol extraction is anchored by `{symbol}`; inspect it for nodes, edges, occurrences, or file-level indexing."
        ),
        "persistence and search projection" => format!(
            "Persistence or search projection is anchored by `{symbol}`; inspect it for durable graph/search state."
        ),
        "snapshot refresh" => format!(
            "Snapshot refresh is anchored by `{symbol}`; inspect it for post-write summary or cache refresh behavior."
        ),
        "route handling" => format!(
            "Route handling is anchored by `{symbol}`; inspect it before tracing request dispatch or handler ownership."
        ),
        "tests and regression coverage" => format!(
            "Regression coverage for this flow is anchored by `{symbol}`; use it to choose focused verification before broader suites."
        ),
        _ => format!("Supporting evidence is anchored by `{symbol}`."),
    }
}

fn path_contains_test_segment(path: &str) -> bool {
    path.starts_with("test/")
        || path.starts_with("tests/")
        || path.contains("/test/")
        || path.contains("/tests/")
        || path.starts_with("test\\")
        || path.starts_with("tests\\")
        || path.contains("\\test\\")
        || path.contains("\\tests\\")
}

fn packet_display_path(path: &str) -> String {
    let normalized = path.trim_start_matches("\\\\?\\").replace('\\', "/");
    for prefix in [
        "crates/",
        "src/",
        "packages/",
        "apps/",
        "lib/",
        "tests/",
        "benches/",
    ] {
        if normalized.starts_with(prefix) {
            return normalized;
        }
    }
    for marker in [
        "/crates/",
        "/src/",
        "/packages/",
        "/apps/",
        "/lib/",
        "/tests/",
        "/benches/",
    ] {
        if let Some(index) = normalized.find(marker) {
            return normalized[index + 1..].to_string();
        }
    }
    normalized
}

fn sanitize_section_id(value: &str) -> String {
    let mut id = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>();
    while id.contains("--") {
        id = id.replace("--", "-");
    }
    id.trim_matches('-').chars().take(48).collect()
}

fn packet_retrieval_profile(
    task_class: Option<PacketTaskClassDto>,
    budget: PacketBudgetModeDto,
    limits: &PacketBudgetLimitsDto,
) -> AgentRetrievalProfileSelectionDto {
    let preset = match task_class {
        Some(PacketTaskClassDto::BugLocalization) | Some(PacketTaskClassDto::EditPlanning) => {
            AgentRetrievalPresetDto::Investigate
        }
        Some(PacketTaskClassDto::ChangeImpact) | Some(PacketTaskClassDto::SymbolOwnership) => {
            AgentRetrievalPresetDto::Impact
        }
        Some(PacketTaskClassDto::RouteTracing) => AgentRetrievalPresetDto::Callflow,
        Some(PacketTaskClassDto::ArchitectureExplanation)
        | Some(PacketTaskClassDto::DataFlow)
        | None => AgentRetrievalPresetDto::Architecture,
    };

    if matches!(
        budget,
        PacketBudgetModeDto::Tiny | PacketBudgetModeDto::Compact
    ) {
        return AgentRetrievalProfileSelectionDto::Custom {
            config: AgentCustomRetrievalConfigDto {
                depth: if matches!(budget, PacketBudgetModeDto::Tiny) {
                    1
                } else {
                    2
                },
                max_nodes: limits.max_trail_edges.clamp(10, 2_000),
                include_edge_occurrences: matches!(
                    task_class,
                    Some(PacketTaskClassDto::ChangeImpact | PacketTaskClassDto::RouteTracing)
                ),
                enable_source_reads: true,
                ..AgentCustomRetrievalConfigDto::default()
            },
        };
    }

    AgentRetrievalProfileSelectionDto::Preset { preset }
}

fn packet_budget_limits(mode: PacketBudgetModeDto) -> PacketBudgetLimitsDto {
    match mode {
        PacketBudgetModeDto::Tiny => PacketBudgetLimitsDto {
            max_anchors: 3,
            max_files: 3,
            max_snippets: 6,
            max_trail_edges: 12,
            max_output_bytes: 24 * 1024,
        },
        PacketBudgetModeDto::Compact => PacketBudgetLimitsDto {
            max_anchors: 10,
            max_files: 10,
            max_snippets: 12,
            max_trail_edges: 30,
            max_output_bytes: 96 * 1024,
        },
        PacketBudgetModeDto::Standard => PacketBudgetLimitsDto {
            max_anchors: 10,
            max_files: 10,
            max_snippets: 24,
            max_trail_edges: 60,
            max_output_bytes: 128 * 1024,
        },
        PacketBudgetModeDto::Deep => PacketBudgetLimitsDto {
            max_anchors: 25,
            max_files: 25,
            max_snippets: 80,
            max_trail_edges: 240,
            max_output_bytes: 512 * 1024,
        },
    }
}

fn apply_packet_budget(
    question: &str,
    requested: PacketBudgetModeDto,
    limits: PacketBudgetLimitsDto,
    answer: &mut AgentAnswerDto,
) -> PacketBudgetDto {
    let mut truncated = false;
    let mut omitted_sections = Vec::new();

    if cap_citations(answer, &limits) {
        truncated = true;
        omitted_sections.push("citations".to_string());
    }
    if cap_graph_edges(answer, limits.max_trail_edges) {
        truncated = true;
        omitted_sections.push("trail_edges".to_string());
    }
    if truncate_answer_markdown_to_byte_cap(answer, limits.max_output_bytes as usize) {
        truncated = true;
        omitted_sections.push("markdown_blocks".to_string());
    }

    let used = packet_budget_usage(answer);
    if used.output_bytes > limits.max_output_bytes {
        truncated = true;
        omitted_sections.push("output_bytes".to_string());
    }

    omitted_sections.sort();
    omitted_sections.dedup();

    PacketBudgetDto {
        requested,
        limits,
        used,
        truncated,
        omitted_sections,
        next_deeper_command: next_deeper_packet_command(question, requested),
    }
}

fn enforce_packet_output_budget(packet: &mut AgentPacketDto) {
    for _ in 0..8 {
        let output_bytes = refresh_packet_output_bytes(packet);
        if output_bytes <= packet.budget.limits.max_output_bytes as usize {
            break;
        }

        packet.budget.truncated = true;
        push_omitted_section(&mut packet.budget, "output_bytes");
        push_omitted_section(&mut packet.budget, "packet_payload");

        let over_by = output_bytes.saturating_sub(packet.budget.limits.max_output_bytes as usize);
        let current_answer_bytes = serde_json::to_vec(&packet.answer)
            .map(|bytes| bytes.len())
            .unwrap_or_default();
        let next_answer_cap = current_answer_bytes
            .saturating_sub(over_by.saturating_add(1024))
            .max(1024);

        if truncate_answer_markdown_to_byte_cap(&mut packet.answer, next_answer_cap) {
            push_omitted_section(&mut packet.budget, "markdown_blocks");
            packet.budget.used = packet_budget_usage(&packet.answer);
            packet.benchmark_trace = packet_benchmark_trace(&packet.answer);
            packet.sufficiency = build_packet_sufficiency(
                &packet.question,
                packet
                    .task_class
                    .unwrap_or(PacketTaskClassDto::ArchitectureExplanation),
                &packet.answer,
                &packet.budget,
            );
            continue;
        }
        break;
    }

    let output_bytes = refresh_packet_output_bytes(packet);
    if output_bytes > packet.budget.limits.max_output_bytes as usize {
        packet.budget.truncated = true;
        push_omitted_section(&mut packet.budget, "output_bytes");
        push_omitted_section(&mut packet.budget, "packet_payload");
        packet.sufficiency = build_packet_sufficiency(
            &packet.question,
            packet
                .task_class
                .unwrap_or(PacketTaskClassDto::ArchitectureExplanation),
            &packet.answer,
            &packet.budget,
        );
    }
}

fn refresh_packet_output_bytes(packet: &mut AgentPacketDto) -> usize {
    for _ in 0..4 {
        let output_bytes = serialized_packet_len(packet);
        let output_bytes_u32 = output_bytes.try_into().unwrap_or(u32::MAX);
        if packet.budget.used.output_bytes == output_bytes_u32 {
            return output_bytes;
        }
        packet.budget.used.output_bytes = output_bytes_u32;
    }
    serialized_packet_len(packet)
}

fn serialized_packet_len(packet: &AgentPacketDto) -> usize {
    serde_json::to_vec(packet)
        .map(|bytes| bytes.len())
        .unwrap_or_default()
}

fn push_omitted_section(budget: &mut PacketBudgetDto, section: &str) {
    if !budget
        .omitted_sections
        .iter()
        .any(|existing| existing == section)
    {
        budget.omitted_sections.push(section.to_string());
        budget.omitted_sections.sort();
    }
}

fn cap_citations(answer: &mut AgentAnswerDto, limits: &PacketBudgetLimitsDto) -> bool {
    let original_len = answer.citations.len();
    let mut files = HashSet::new();
    let mut roles = HashSet::new();
    let mut kept = Vec::new();
    let mut deferred = Vec::new();

    for citation in answer.citations.drain(..) {
        let file = citation.file_path.as_deref().map(packet_display_path);
        let role = packet_evidence_role(&citation);
        let file_is_new = file.as_ref().is_some_and(|path| !files.contains(path));
        let role_is_new = role.is_some_and(|role| !roles.contains(role));
        if kept.len() < limits.max_anchors as usize
            && (file_is_new || role_is_new || kept.is_empty())
            && packet_file_fits_limit(file.as_deref(), &files, limits.max_files)
        {
            if let Some(path) = file {
                files.insert(path);
            }
            if let Some(role) = role {
                roles.insert(role);
            }
            kept.push(citation);
        } else {
            deferred.push(citation);
        }
    }

    for citation in deferred {
        if kept.len() >= limits.max_anchors as usize {
            continue;
        }
        let file = citation.file_path.as_deref().map(packet_display_path);
        if !packet_file_fits_limit(file.as_deref(), &files, limits.max_files) {
            continue;
        }
        if let Some(path) = file {
            files.insert(path);
        }
        kept.push(citation);
    }

    let truncated = kept.len() < original_len;
    answer.citations = kept;
    truncated
}

fn packet_file_fits_limit(path: Option<&str>, files: &HashSet<String>, max_files: u32) -> bool {
    path.is_none_or(|path| files.contains(path) || files.len() < max_files as usize)
}

fn cap_graph_edges(answer: &mut AgentAnswerDto, max_edges: u32) -> bool {
    let mut remaining = max_edges as usize;
    let mut truncated = false;
    for artifact in &mut answer.graphs {
        let GraphArtifactDto::Uml { graph, .. } = artifact else {
            continue;
        };
        if graph.edges.len() > remaining {
            let omitted = graph.edges.len() - remaining;
            graph.edges.truncate(remaining);
            graph.truncated = true;
            graph.omitted_edge_count = graph
                .omitted_edge_count
                .saturating_add(omitted.try_into().unwrap_or(u32::MAX));
            truncated = true;
            remaining = 0;
        } else {
            remaining = remaining.saturating_sub(graph.edges.len());
        }
        if prune_graph_to_retained_edges(graph) {
            truncated = true;
        }
    }
    truncated
}

fn prune_graph_to_retained_edges(graph: &mut GraphResponse) -> bool {
    let original_nodes = graph.nodes.len();
    let original_layout_nodes = graph
        .canonical_layout
        .as_ref()
        .map(|layout| layout.nodes.len())
        .unwrap_or_default();
    let original_layout_edges = graph
        .canonical_layout
        .as_ref()
        .map(|layout| layout.edges.len())
        .unwrap_or_default();
    let mut retained_node_ids = HashSet::new();
    retained_node_ids.insert(graph.center_id.clone());
    let retained_edge_ids = graph
        .edges
        .iter()
        .map(|edge| edge.id.clone())
        .collect::<HashSet<_>>();

    for edge in &graph.edges {
        retained_node_ids.insert(edge.source.clone());
        retained_node_ids.insert(edge.target.clone());
    }

    graph
        .nodes
        .retain(|node| retained_node_ids.contains(&node.id));

    if let Some(layout) = graph.canonical_layout.as_mut() {
        layout.edges.retain(|edge| {
            let endpoints_retained = retained_node_ids.contains(&edge.source)
                && retained_node_ids.contains(&edge.target);
            let source_edge_retained = edge.source_edge_ids.is_empty()
                || edge
                    .source_edge_ids
                    .iter()
                    .any(|edge_id| retained_edge_ids.contains(edge_id));
            endpoints_retained && source_edge_retained
        });
        layout
            .nodes
            .retain(|node| retained_node_ids.contains(&node.id));
    }

    let pruned = graph.nodes.len() < original_nodes
        || graph
            .canonical_layout
            .as_ref()
            .map(|layout| layout.nodes.len() < original_layout_nodes)
            .unwrap_or(false)
        || graph
            .canonical_layout
            .as_ref()
            .map(|layout| layout.edges.len() < original_layout_edges)
            .unwrap_or(false);
    if pruned {
        graph.truncated = true;
    }
    pruned
}

fn truncate_answer_markdown_to_byte_cap(answer: &mut AgentAnswerDto, byte_cap: usize) -> bool {
    let mut truncated = false;
    for _ in 0..8 {
        let Ok(bytes) = serde_json::to_vec(answer) else {
            return truncated;
        };
        if bytes.len() <= byte_cap {
            return truncated;
        }
        let Some((section_index, block_index, len)) = largest_markdown_block(answer) else {
            return truncated;
        };
        if len <= 256 {
            return truncated;
        }
        if let AgentResponseBlockDto::Markdown { markdown } =
            &mut answer.sections[section_index].blocks[block_index]
        {
            truncate_markdown_block(markdown);
            truncated = true;
        }
    }
    truncated
}

fn largest_markdown_block(answer: &AgentAnswerDto) -> Option<(usize, usize, usize)> {
    let mut largest = None;
    for (section_index, section) in answer.sections.iter().enumerate() {
        for (block_index, block) in section.blocks.iter().enumerate() {
            if let AgentResponseBlockDto::Markdown { markdown } = block {
                let len = markdown.len();
                if largest.is_none_or(|(_, _, existing)| len > existing) {
                    largest = Some((section_index, block_index, len));
                }
            }
        }
    }
    largest
}

fn truncate_markdown_block(markdown: &mut String) {
    const SUFFIX: &str = "\n\n... packet section truncated by budget ...\n";
    let keep_chars = markdown.chars().count() / 2;
    let mut keep_byte = markdown.len();
    if let Some((index, _)) = markdown.char_indices().nth(keep_chars) {
        keep_byte = index;
    }
    markdown.truncate(keep_byte);
    markdown.push_str(SUFFIX);
}

fn packet_budget_usage(answer: &AgentAnswerDto) -> PacketBudgetUsageDto {
    let files = answer
        .citations
        .iter()
        .filter_map(|citation| citation.file_path.as_deref())
        .collect::<HashSet<_>>()
        .len();
    let trail_edges = answer
        .graphs
        .iter()
        .map(|artifact| match artifact {
            GraphArtifactDto::Uml { graph, .. } => graph.edges.len(),
            GraphArtifactDto::Mermaid { .. } => 0,
        })
        .sum::<usize>();
    let snippets = answer
        .retrieval_trace
        .steps
        .iter()
        .filter(|step| {
            step.kind == AgentRetrievalStepKindDto::SourceRead
                && step.status == AgentRetrievalStepStatusDto::Ok
        })
        .count();
    let output_bytes = serde_json::to_vec(answer)
        .map(|bytes| bytes.len())
        .unwrap_or_default();

    PacketBudgetUsageDto {
        anchors: answer.citations.len().try_into().unwrap_or(u32::MAX),
        files: files.try_into().unwrap_or(u32::MAX),
        snippets: snippets.try_into().unwrap_or(u32::MAX),
        trail_edges: trail_edges.try_into().unwrap_or(u32::MAX),
        output_bytes: output_bytes.try_into().unwrap_or(u32::MAX),
    }
}

fn next_deeper_packet_command(question: &str, requested: PacketBudgetModeDto) -> Option<String> {
    let next = match requested {
        PacketBudgetModeDto::Tiny => "compact",
        PacketBudgetModeDto::Compact => "standard",
        PacketBudgetModeDto::Standard => "deep",
        PacketBudgetModeDto::Deep => return None,
    };
    Some(format!(
        "codestory-cli packet --project <target-workspace> --question {} --budget {next}",
        quote_packet_command_value(question)
    ))
}

fn quote_packet_command_value(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\\\""))
}

fn build_packet_sufficiency(
    question: &str,
    task_class: PacketTaskClassDto,
    answer: &AgentAnswerDto,
    budget: &PacketBudgetDto,
) -> PacketSufficiencyDto {
    let has_errors = answer
        .retrieval_trace
        .steps
        .iter()
        .any(|step| step.status == AgentRetrievalStepStatusDto::Error);
    let min_citations = packet_sufficiency_min_citations(task_class);
    let has_minimum_coverage = answer.citations.len() >= min_citations;
    let status = if answer.citations.is_empty() {
        PacketSufficiencyStatusDto::Insufficient
    } else if has_errors || !has_minimum_coverage || packet_budget_exceeded_hard_output_cap(budget)
    {
        PacketSufficiencyStatusDto::Partial
    } else {
        PacketSufficiencyStatusDto::Sufficient
    };

    let mut gaps = Vec::new();
    if answer.citations.is_empty() {
        gaps.push("No cited anchors were found for the question.".to_string());
    }
    if !answer.citations.is_empty() && !has_minimum_coverage {
        gaps.push(format!(
            "{:?} packet found only {} cited anchor(s); at least {} are required before treating the packet as sufficient.",
            task_class,
            answer.citations.len(),
            min_citations
        ));
    }
    if budget.truncated && status != PacketSufficiencyStatusDto::Sufficient {
        gaps.push(format!(
            "Packet was truncated by {:?} budget: {}.",
            budget.requested,
            budget.omitted_sections.join(", ")
        ));
    }
    for step in answer
        .retrieval_trace
        .steps
        .iter()
        .filter(|step| step.status == AgentRetrievalStepStatusDto::Error)
    {
        gaps.push(format!("{:?} step failed.", step.kind));
    }

    let follow_up_commands = packet_follow_up_commands(question, status, budget);
    let open_next = follow_up_commands.clone();
    let avoid_opening = answer
        .citations
        .iter()
        .filter_map(|citation| citation.file_path.as_ref())
        .map(|path| packet_display_path(path))
        .collect::<HashSet<_>>()
        .into_iter()
        .take(12)
        .map(|path| {
            format!(
                "{} because this packet already includes a citation for the current answer.",
                path
            )
        })
        .collect::<Vec<_>>();

    let mut covered_claims = packet_supported_claims(answer);
    if covered_claims.is_empty() {
        covered_claims.push(PacketClaimDto {
            claim: answer.summary.clone(),
            citations: answer.citations.iter().take(6).cloned().collect(),
        });
    }

    PacketSufficiencyDto {
        status,
        covered_claims,
        open_next,
        avoid_opening,
        gaps,
        follow_up_commands,
    }
}

fn packet_sufficiency_min_citations(task_class: PacketTaskClassDto) -> usize {
    match task_class {
        PacketTaskClassDto::BugLocalization | PacketTaskClassDto::SymbolOwnership => 2,
        PacketTaskClassDto::ArchitectureExplanation
        | PacketTaskClassDto::ChangeImpact
        | PacketTaskClassDto::RouteTracing
        | PacketTaskClassDto::DataFlow
        | PacketTaskClassDto::EditPlanning => 3,
    }
}

fn packet_budget_exceeded_hard_output_cap(budget: &PacketBudgetDto) -> bool {
    budget
        .omitted_sections
        .iter()
        .any(|section| section == "output_bytes")
}

fn packet_follow_up_commands(
    question: &str,
    status: PacketSufficiencyStatusDto,
    budget: &PacketBudgetDto,
) -> Vec<String> {
    match status {
        PacketSufficiencyStatusDto::Sufficient => Vec::new(),
        PacketSufficiencyStatusDto::Partial => budget
            .next_deeper_command
            .clone()
            .into_iter()
            .chain(std::iter::once(format!(
                "codestory-cli search --project <target-workspace> --query {} --why",
                quote_packet_command_value(question)
            )))
            .collect(),
        PacketSufficiencyStatusDto::Insufficient => vec![
            "codestory-cli index --project <target-workspace> --refresh full".to_string(),
            format!(
                "codestory-cli search --project <target-workspace> --query {} --repo-text on --why",
                quote_packet_command_value(question)
            ),
        ],
    }
}

fn packet_benchmark_trace(answer: &AgentAnswerDto) -> PacketBenchmarkTraceDto {
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
            | AgentRetrievalStepKindDto::QueryExpansion
            | AgentRetrievalStepKindDto::RepoTextFallback => search_steps += 1,
            AgentRetrievalStepKindDto::Trail
            | AgentRetrievalStepKindDto::Neighborhood
            | AgentRetrievalStepKindDto::TrailFilterOptions => trail_steps += 1,
            AgentRetrievalStepKindDto::NodeDetails
            | AgentRetrievalStepKindDto::NodeOccurrences
            | AgentRetrievalStepKindDto::EdgeOccurrences
            | AgentRetrievalStepKindDto::MermaidSynthesis
            | AgentRetrievalStepKindDto::AnswerSynthesis => {}
        }
    }

    PacketBenchmarkTraceDto {
        retrieval_trace: answer.retrieval_trace.clone(),
        source_read_steps,
        search_steps,
        trail_steps,
    }
}

fn cap_graph_artifacts(
    graphs: &mut Vec<GraphArtifactDto>,
    byte_cap: usize,
) -> GraphArtifactCapStats {
    let mut retained = Vec::with_capacity(graphs.len());
    let mut retained_bytes = 0usize;
    let mut omitted_count = 0usize;

    for graph in graphs.drain(..) {
        let encoded_bytes = serde_json::to_vec(&graph)
            .map(|bytes| bytes.len())
            .unwrap_or(usize::MAX);
        if retained_bytes.saturating_add(encoded_bytes) <= byte_cap {
            retained_bytes = retained_bytes.saturating_add(encoded_bytes);
            retained.push(graph);
        } else {
            omitted_count = omitted_count.saturating_add(1);
        }
    }

    *graphs = retained;
    GraphArtifactCapStats {
        retained_bytes,
        omitted_count,
        truncated: omitted_count > 0,
    }
}

fn execute_retrieval(
    controller: &AppController,
    req: &AgentAskRequest,
    prompt: &str,
    ask_started_at: Instant,
    resolved_profile: &ResolvedProfile,
    trace: &mut TraceRecorder,
) -> Result<RetrievalBundle, ApiError> {
    let mut bundle = RetrievalBundle::default();
    let semantic_required = hybrid_retrieval_enabled();

    let search_step = trace.start_step(
        AgentRetrievalStepKindDto::Search,
        vec![field("query_chars", prompt.len().to_string())],
    );
    let semantic_query_step = trace.start_step(
        AgentRetrievalStepKindDto::SemanticQueryEmbedding,
        vec![field("required", semantic_required.to_string())],
    );
    let semantic_candidates_step = trace.start_step(
        AgentRetrievalStepKindDto::SemanticCandidateRetrieval,
        vec![field("required", semantic_required.to_string())],
    );
    let hybrid_rerank_step = trace.start_step(
        AgentRetrievalStepKindDto::HybridRerank,
        vec![field("required", semantic_required.to_string())],
    );

    let max_results = req
        .max_results
        .unwrap_or(DEFAULT_MAX_RESULTS)
        .clamp(1, resolved_profile.max_search_results) as usize;
    let mut scored_hits = match controller.search_hybrid_scored(
        SearchRequest {
            query: prompt.to_string(),
            repo_text: SearchRepoTextMode::Off,
            limit_per_source: max_results as u32,
            expand_search_plan: false,
            hybrid_weights: None,
            hybrid_limits: None,
        },
        req.focus_node_id.clone(),
        max_results,
        req.hybrid_weights.clone(),
    ) {
        Ok(value) => value,
        Err(error) => {
            trace.finish_err(search_step, error.message.clone());
            trace.finish_err(semantic_query_step, error.message.clone());
            trace.finish_err(semantic_candidates_step, error.message.clone());
            trace.finish_err(hybrid_rerank_step, error.message.clone());
            return Err(error);
        }
    };
    let hits = scored_hits
        .iter()
        .map(|scored| scored.hit.clone())
        .collect::<Vec<_>>();

    trace.finish_ok(
        search_step,
        vec![
            field("hits", hits.len().to_string()),
            field(
                "accepted_hits",
                if should_investigate(resolved_profile)
                    && weak_initial_hits(prompt, &hits)
                    && !has_literal_fallback_signal(prompt)
                {
                    "0".to_string()
                } else {
                    hits.len().to_string()
                },
            ),
            field("max_results", max_results.to_string()),
            field("repo_text", "off_initial"),
        ],
    );
    if semantic_required {
        trace.finish_ok(
            semantic_query_step,
            vec![
                field("model_required", "local"),
                field("query_embedded", "true"),
            ],
        );
        trace.finish_ok(
            semantic_candidates_step,
            vec![field("candidates", scored_hits.len().to_string())],
        );
        trace.finish_ok(
            hybrid_rerank_step,
            vec![field("ranked", hits.len().to_string())],
        );
    } else {
        trace.finish_skipped(
            semantic_query_step,
            "Hybrid retrieval disabled by CODESTORY_HYBRID_RETRIEVAL_ENABLED=false.",
            Vec::new(),
        );
        trace.finish_skipped(
            semantic_candidates_step,
            "Hybrid retrieval disabled by CODESTORY_HYBRID_RETRIEVAL_ENABLED=false.",
            Vec::new(),
        );
        trace.finish_ok(
            hybrid_rerank_step,
            vec![field("ranked", hits.len().to_string())],
        );
    }

    let initial_hit_count = hits.len();
    let mut hits = hits;
    let literal_fallback_signal = has_literal_fallback_signal(prompt);
    let promotable_focus_available =
        req.focus_node_id.is_some() || investigation_focus_anchor(prompt, &hits).is_some();
    let mut expansion_added_hits = false;
    if should_investigate(resolved_profile)
        && weak_initial_hits(prompt, &hits)
        && !promotable_focus_available
    {
        let expanded = match investigate_query_expansion(
            controller,
            req,
            prompt,
            max_results,
            ask_started_at,
            resolved_profile,
            trace,
        ) {
            Ok(expanded) => expanded,
            Err(error) => {
                trace.annotate(format!(
                    "Investigation query expansion failed; continuing with initial hits: {}",
                    error.message
                ));
                Vec::new()
            }
        };
        if !expanded.is_empty() {
            merge_scored_hits(&mut scored_hits, expanded, max_results);
            hits = scored_hits
                .iter()
                .map(|scored| scored.hit.clone())
                .collect::<Vec<_>>();
            bundle.fallback_used = true;
            expansion_added_hits = true;
        }

        if initial_hit_count == 0 && expansion_added_hits && !literal_fallback_signal {
            hits.clear();
            scored_hits.clear();
            trace.annotate(
                "Investigation discarded expansion-only hits for an unanchored natural-language query.",
            );
        }

        if weak_initial_hits(prompt, &hits) && literal_fallback_signal {
            let text_hits = match investigate_repo_text_fallback(
                controller,
                req,
                prompt,
                max_results,
                ask_started_at,
                resolved_profile,
                trace,
            ) {
                Ok(hits) => hits,
                Err(error) => {
                    trace.annotate(format!(
                        "Investigation repo-text fallback failed; continuing without file fallback: {}",
                        error.message
                    ));
                    Vec::new()
                }
            };
            if !text_hits.is_empty() {
                merge_search_hits(&mut hits, text_hits, max_results);
                bundle.fallback_used = true;
                bundle.repo_text_fallback_used = hits
                    .iter()
                    .any(|hit| hit.origin == SearchHitOrigin::TextMatch);
            }
        } else if weak_initial_hits(prompt, &hits) && !is_repo_explanation_prompt(prompt) {
            if !hits.is_empty() {
                hits.clear();
                scored_hits.clear();
                trace.annotate(
                    "Investigation discarded low-confidence unanchored hits for a natural-language query.",
                );
            }
            trace.annotate(
                "Repo-text fallback skipped because the weak query did not contain a literal file/source token.",
            );
        } else if weak_initial_hits(prompt, &hits) {
            trace.annotate(
                "Investigation deferred a broad repo explanation prompt to grounding snapshot fallback.",
            );
        }

        if weak_initial_hits(prompt, &hits) && !is_repo_explanation_prompt(prompt) {
            trace.annotate(
                "Investigation low confidence gap after query expansion and repo-text fallback.",
            );
        }
    } else if should_investigate(resolved_profile)
        && weak_initial_hits(prompt, &hits)
        && promotable_focus_available
    {
        trace.annotate(
            "Investigation kept an explicit or prompt-anchored focus instead of broad repo-text fallback.",
        );
    }

    if should_investigate(resolved_profile)
        && weak_initial_hits(prompt, &hits)
        && is_repo_explanation_prompt(prompt)
    {
        let overview_hits = repo_explanation_grounding_hits(
            controller,
            req,
            max_results,
            ask_started_at,
            resolved_profile,
            trace,
        )?;
        if !overview_hits.is_empty() {
            hits = overview_hits;
            scored_hits.clear();
            bundle.fallback_used = true;
            bundle.repo_explanation_fallback_used = true;
            trace.annotate(
                "Investigation used grounding snapshot fallback for a broad repo explanation prompt.",
            );
        }
    }

    let focus_node_id = req
        .focus_node_id
        .clone()
        .or_else(|| investigation_focus_anchor(prompt, &hits))
        .or_else(|| {
            hits.iter()
                .find(|hit| hit.resolvable)
                .map(|hit| hit.node_id.clone())
        });

    let filter_step = trace.start_step(
        AgentRetrievalStepKindDto::TrailFilterOptions,
        vec![field("has_focus", focus_node_id.is_some().to_string())],
    );
    let filter_options = match controller.graph_trail_filter_options() {
        Ok(options) => {
            trace.finish_ok(
                filter_step,
                vec![
                    field("edge_kinds", options.edge_kinds.len().to_string()),
                    field("node_kinds", options.node_kinds.len().to_string()),
                ],
            );
            options
        }
        Err(error) => {
            trace.finish_err(filter_step, error.message.clone());
            trace
                .annotate("Trail filter options unavailable; continuing with unsanitized filters.");
            TrailFilterOptionsDto {
                node_kinds: Vec::new(),
                edge_kinds: Vec::new(),
            }
        }
    };

    let mut primary_graph: Option<GraphResponse> = None;

    if let Some(center_id) = focus_node_id.clone() {
        let neighborhood_step = trace.start_step(
            AgentRetrievalStepKindDto::Neighborhood,
            vec![field("center_id", center_id.0.clone())],
        );
        match controller.graph_neighborhood(GraphRequest {
            center_id,
            max_edges: Some(DEFAULT_MAX_EDGES),
        }) {
            Ok(neighborhood) => {
                trace.finish_ok(
                    neighborhood_step,
                    vec![
                        field("nodes", neighborhood.nodes.len().to_string()),
                        field("edges", neighborhood.edges.len().to_string()),
                        field("truncated", neighborhood.truncated.to_string()),
                    ],
                );

                primary_graph = Some(neighborhood.clone());
                bundle.graphs.push(GraphArtifactDto::Uml {
                    id: "uml-neighborhood".to_string(),
                    title: "Primary Neighborhood".to_string(),
                    graph: neighborhood,
                });
            }
            Err(error) => {
                trace.finish_err(neighborhood_step, error.message.clone());
                trace.annotate("Neighborhood retrieval failed; continuing with trail retrieval.");
            }
        }
    } else {
        let neighborhood_step = trace.start_step(
            AgentRetrievalStepKindDto::Neighborhood,
            vec![field("has_focus", "false")],
        );
        trace.finish_skipped(neighborhood_step, "No focus node selected.", Vec::new());
    }

    let sanitized_plans = resolved_profile
        .trail_plans
        .iter()
        .map(|plan| sanitize_plan_filters(plan, &filter_options))
        .collect::<Vec<_>>();

    if focus_node_id.is_none() {
        let trail_step = trace.start_step(
            AgentRetrievalStepKindDto::Trail,
            vec![field("plans", sanitized_plans.len().to_string())],
        );
        trace.finish_skipped(trail_step, "No focus node selected.", Vec::new());
    } else {
        for (idx, plan) in sanitized_plans.iter().enumerate() {
            let trail_step = trace.start_step(
                AgentRetrievalStepKindDto::Trail,
                vec![
                    field("index", idx.to_string()),
                    field("mode", format!("{:?}", plan.mode)),
                    field("depth", plan.depth.to_string()),
                    field("direction", format!("{:?}", plan.direction)),
                    field("max_nodes", plan.max_nodes.to_string()),
                ],
            );

            let root_id = focus_node_id.clone().expect("checked focus node");
            let request = TrailConfigDto {
                root_id,
                mode: plan.mode,
                target_id: None,
                depth: plan.depth,
                direction: plan.direction,
                caller_scope: plan.caller_scope,
                edge_filter: plan.edge_filter.clone(),
                show_utility_calls: true,
                hide_speculative: false,
                story: false,
                node_filter: plan.node_filter.clone(),
                max_nodes: plan.max_nodes,
                layout_direction: codestory_contracts::api::LayoutDirection::Horizontal,
            };

            match controller.graph_trail(request) {
                Ok(trail) => {
                    let trail_output = vec![
                        field("nodes", trail.nodes.len().to_string()),
                        field("edges", trail.edges.len().to_string()),
                        field("max_nodes", plan.max_nodes.to_string()),
                        field("truncated", trail.truncated.to_string()),
                        field("omitted_edges", trail.omitted_edge_count.to_string()),
                    ];
                    if trail.truncated {
                        trace.finish_truncated(
                            trail_step,
                            format!(
                                "Trail output hit max_nodes={}; narrow focus or lower depth.",
                                plan.max_nodes
                            ),
                            trail_output,
                        );
                        trace.annotate(trail_truncated_annotation(idx + 1, plan.max_nodes));
                    } else {
                        trace.finish_ok(trail_step, trail_output);
                    }
                    bundle.graphs.push(GraphArtifactDto::Uml {
                        id: format!("uml-trail-{}", idx + 1),
                        title: format!("Trail {}", idx + 1),
                        graph: trail,
                    });
                }
                Err(error) => {
                    trace.finish_err(trail_step, error.message.clone());
                    trace.annotate(format!("Trail {} failed and was skipped.", idx + 1));
                }
            }
        }
    }

    let details_step = trace.start_step(
        AgentRetrievalStepKindDto::NodeDetails,
        vec![field("has_focus", focus_node_id.is_some().to_string())],
    );
    let focused_node = match focus_node_id.clone() {
        Some(id) => match controller.node_details(NodeDetailsRequest { id }) {
            Ok(details) => {
                trace.finish_ok(
                    details_step,
                    vec![
                        field("display_name", details.display_name.clone()),
                        field("kind", format!("{:?}", details.kind)),
                    ],
                );
                Some(details)
            }
            Err(error) => {
                trace.finish_err(details_step, error.message.clone());
                None
            }
        },
        None => {
            trace.finish_skipped(details_step, "No focus node selected.", Vec::new());
            None
        }
    };

    let occurrences_step = trace.start_step(
        AgentRetrievalStepKindDto::NodeOccurrences,
        vec![field("candidates", hits.len().min(3).to_string())],
    );
    let node_occurrence_deadline = phase_deadline_ms(req, 65, 100);
    if should_truncate_phase(resolved_profile, ask_started_at, node_occurrence_deadline) {
        trace.finish_truncated(
            occurrences_step,
            "Skipped node occurrence lookups because latency budget was exceeded.",
            vec![field(
                "phase_deadline_ms",
                node_occurrence_deadline.to_string(),
            )],
        );
        trace.annotate("Latency-first cutoff skipped node occurrence lookups.");
    } else {
        let mut occurrence_count = 0usize;
        for hit in hits.iter().take(3) {
            match controller.node_occurrences(NodeOccurrencesRequest {
                id: hit.node_id.clone(),
            }) {
                Ok(occurrences) => {
                    occurrence_count += occurrences.len();
                }
                Err(error) => {
                    trace.annotate(format!(
                        "Node occurrence lookup failed for {}: {}",
                        hit.display_name, error.message
                    ));
                }
            }
        }
        trace.finish_ok(
            occurrences_step,
            vec![field("occurrence_count", occurrence_count.to_string())],
        );
    }

    let edge_occurrences_step = trace.start_step(
        AgentRetrievalStepKindDto::EdgeOccurrences,
        vec![field(
            "enabled",
            resolved_profile.include_edge_occurrences.to_string(),
        )],
    );
    let edge_occurrence_deadline = phase_deadline_ms(req, 75, 100);
    if should_truncate_phase(resolved_profile, ask_started_at, edge_occurrence_deadline) {
        trace.finish_truncated(
            edge_occurrences_step,
            "Skipped edge occurrence lookup because latency budget was exceeded.",
            vec![field(
                "phase_deadline_ms",
                edge_occurrence_deadline.to_string(),
            )],
        );
        trace.annotate("Latency-first cutoff skipped edge occurrence lookups.");
    } else if !resolved_profile.include_edge_occurrences {
        trace.finish_skipped(
            edge_occurrences_step,
            "Edge occurrences are disabled for this profile.",
            Vec::new(),
        );
    } else if let Some(edge_id) = first_edge_id_from_graphs(&bundle.graphs) {
        match controller
            .edge_occurrences(codestory_contracts::api::EdgeOccurrencesRequest { id: edge_id })
        {
            Ok(occurrences) => {
                trace.finish_ok(
                    edge_occurrences_step,
                    vec![field("occurrence_count", occurrences.len().to_string())],
                );
            }
            Err(error) => {
                trace.finish_err(edge_occurrences_step, error.message.clone());
            }
        }
    } else {
        trace.finish_skipped(
            edge_occurrences_step,
            "No edges available for lookup.",
            Vec::new(),
        );
    }

    let primary_subgraph_id = bundle.graphs.first().map(|graph| match graph {
        GraphArtifactDto::Uml { id, .. } => id.clone(),
        GraphArtifactDto::Mermaid { id, .. } => id.clone(),
    });
    let include_structured_evidence = req.include_evidence;
    let scored_by_node = scored_hits
        .iter()
        .map(|scored| (scored.hit.node_id.clone(), scored))
        .collect::<HashMap<_, _>>();
    let citations = hits
        .iter()
        .map(|hit| {
            if let Some(scored) = scored_by_node.get(&hit.node_id) {
                to_citation(
                    scored,
                    primary_subgraph_id.as_deref(),
                    primary_graph.as_ref(),
                    include_structured_evidence,
                )
            } else {
                to_citation_from_hit(
                    hit,
                    primary_subgraph_id.as_deref(),
                    primary_graph.as_ref(),
                    include_structured_evidence,
                )
            }
        })
        .collect::<Vec<_>>();

    bundle.hits = hits;
    bundle.citations = citations;
    bundle.focus_node_id = focus_node_id;
    bundle.focused_node = focused_node;
    bundle.primary_graph = primary_graph;

    Ok(bundle)
}

fn to_citation(
    scored: &HybridSearchScoredHit,
    subgraph_id: Option<&str>,
    primary_graph: Option<&GraphResponse>,
    include_evidence: bool,
) -> AgentCitationDto {
    AgentCitationDto {
        node_id: scored.hit.node_id.clone(),
        display_name: scored.hit.display_name.clone(),
        kind: scored.hit.kind,
        file_path: scored.hit.file_path.clone(),
        line: scored.hit.line,
        score: scored.total_score,
        origin: scored.hit.origin,
        resolvable: scored.hit.resolvable,
        subgraph_id: subgraph_id.map(ToOwned::to_owned),
        evidence_edge_ids: if include_evidence {
            evidence_edge_ids_for_node(primary_graph, &scored.hit.node_id)
        } else {
            Vec::new()
        },
        retrieval_score_breakdown: include_evidence.then_some(RetrievalScoreBreakdownDto {
            lexical: scored.lexical_score,
            semantic: scored.semantic_score,
            graph: scored.graph_score,
            total: scored.total_score,
        }),
    }
}

fn weak_initial_hits(prompt: &str, hits: &[SearchHit]) -> bool {
    let Some(top_hit) = hits.first() else {
        return true;
    };
    let prompt_terms = normalized_anchor_terms(prompt);
    if top_hit.score >= WEAK_INITIAL_TOP_SCORE && hit_has_indexed_anchor(top_hit, &prompt_terms) {
        return false;
    }

    hits.len() < WEAK_INITIAL_HIT_COUNT
        || top_hit.score < WEAK_INITIAL_TOP_SCORE
        || !hits
            .iter()
            .take(WEAK_INITIAL_HIT_COUNT)
            .any(|hit| hit_has_indexed_anchor(hit, &prompt_terms))
}

fn hit_has_indexed_anchor(hit: &SearchHit, prompt_terms: &HashSet<String>) -> bool {
    if hit.origin == SearchHitOrigin::TextMatch {
        return true;
    }
    if prompt_mentions_display_name(prompt_terms, &hit.display_name) {
        return true;
    }

    hit.score_breakdown
        .as_ref()
        .map(|breakdown| {
            breakdown.lexical > WEAK_INITIAL_MIN_LEXICAL_ANCHOR
                || breakdown.graph > WEAK_INITIAL_MIN_GRAPH_ANCHOR
        })
        .unwrap_or(hit.resolvable)
}

fn prompt_mentions_display_name(prompt_terms: &HashSet<String>, display_name: &str) -> bool {
    let display_terms = normalized_anchor_terms(display_name);
    !display_terms.is_empty() && display_terms.iter().all(|term| prompt_terms.contains(term))
}

fn investigation_focus_anchor(prompt: &str, hits: &[SearchHit]) -> Option<NodeId> {
    let prompt_terms = normalized_anchor_terms(prompt);
    hits.iter()
        .find(|hit| {
            hit.resolvable && prompt_mentions_display_name(&prompt_terms, &hit.display_name)
        })
        .map(|hit| hit.node_id.clone())
}

fn normalized_anchor_terms(value: &str) -> HashSet<String> {
    value
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter_map(|term| {
            let term = term.trim().to_ascii_lowercase();
            (term.len() >= 3).then_some(term)
        })
        .collect()
}

fn should_investigate(profile: &ResolvedProfile) -> bool {
    profile.preset == codestory_contracts::api::AgentRetrievalPresetDto::Investigate
}

fn has_literal_fallback_signal(prompt: &str) -> bool {
    prompt.contains('`')
        || prompt.contains('/')
        || prompt.contains('\\')
        || prompt.contains("::")
        || prompt.contains(".rs")
        || prompt
            .split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')
            .any(|token| {
                token.contains('_')
                    || (token.len() >= 4
                        && token
                            .chars()
                            .filter(|ch| ch.is_ascii_alphabetic())
                            .all(|ch| ch.is_ascii_uppercase()))
            })
}

fn is_repo_explanation_prompt(prompt: &str) -> bool {
    let lower = prompt.to_ascii_lowercase();
    let subject = lower.contains("repo") || lower.contains("project") || lower.contains("codebase");
    let intent = lower.contains("fit together")
        || lower.contains("how does")
        || lower.contains("explain")
        || lower.contains("overview")
        || lower.contains("architecture");
    subject && intent
}

fn repo_explanation_grounding_hits(
    controller: &AppController,
    req: &AgentAskRequest,
    max_results: usize,
    ask_started_at: Instant,
    resolved_profile: &ResolvedProfile,
    trace: &mut TraceRecorder,
) -> Result<Vec<SearchHit>, ApiError> {
    let step = trace.start_step(
        AgentRetrievalStepKindDto::QueryExpansion,
        vec![
            field("strategy", "grounding_snapshot"),
            field("max_results", max_results.to_string()),
        ],
    );
    let deadline_ms = phase_deadline_ms(req, 55, 100);
    if should_truncate_phase(resolved_profile, ask_started_at, deadline_ms) {
        trace.finish_truncated(
            step,
            "Skipped grounding snapshot fallback because latency budget was exceeded.",
            vec![field("phase_deadline_ms", deadline_ms.to_string())],
        );
        return Ok(Vec::new());
    }

    let snapshot = match controller.grounding_snapshot(GroundingBudgetDto::Strict) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            trace.finish_err(step, error.message.clone());
            return Err(error);
        }
    };

    let hits = crate::grounding::grounding_explanation_search_hits(&snapshot, max_results);

    trace.finish_ok(
        step,
        vec![
            field("grounding_symbols", hits.len().to_string()),
            field("coverage_files", snapshot.coverage.total_files.to_string()),
            field(
                "coverage_symbols",
                snapshot.coverage.total_symbols.to_string(),
            ),
        ],
    );

    Ok(hits)
}

#[cfg(test)]
fn search_hit_from_grounding_symbol(
    symbol: &codestory_contracts::api::GroundingSymbolDigestDto,
) -> SearchHit {
    let (file_path, line) = symbol
        .node_ref
        .as_deref()
        .and_then(split_node_ref_location)
        .unwrap_or((None, symbol.line));
    SearchHit {
        node_id: symbol.id.clone(),
        display_name: symbol
            .label
            .split(" @ ")
            .next()
            .unwrap_or(symbol.label.as_str())
            .to_string(),
        kind: symbol.kind,
        file_path,
        line: symbol.line.or(line),
        score: 0.55,
        origin: SearchHitOrigin::IndexedSymbol,
        match_quality: None,
        resolvable: true,
        score_breakdown: Some(RetrievalScoreBreakdownDto {
            lexical: 0.35,
            semantic: 0.0,
            graph: 0.20,
            total: 0.55,
        }),
    }
}

#[cfg(test)]
fn split_node_ref_location(value: &str) -> Option<(Option<String>, Option<u32>)> {
    let mut parts = value.rsplitn(3, ':');
    let _name = parts.next()?;
    let line = parts.next()?.parse::<u32>().ok();
    let path = parts.next().map(ToOwned::to_owned);
    Some((path, line))
}

fn investigate_query_expansion(
    controller: &AppController,
    req: &AgentAskRequest,
    prompt: &str,
    max_results: usize,
    ask_started_at: Instant,
    resolved_profile: &ResolvedProfile,
    trace: &mut TraceRecorder,
) -> Result<Vec<HybridSearchScoredHit>, ApiError> {
    let terms = prompt_search_terms(prompt)
        .into_iter()
        .take(4)
        .collect::<Vec<_>>();
    let expansion_step = trace.start_step(
        AgentRetrievalStepKindDto::QueryExpansion,
        vec![
            field("term_count", terms.len().to_string()),
            field("max_results", max_results.to_string()),
        ],
    );

    if terms.is_empty() {
        trace.finish_skipped(
            expansion_step,
            "No deterministic expansion terms extracted.",
            Vec::new(),
        );
        return Ok(Vec::new());
    }

    let expansion_deadline = phase_deadline_ms(req, 45, 100);
    if should_truncate_phase(resolved_profile, ask_started_at, expansion_deadline) {
        trace.finish_truncated(
            expansion_step,
            "Skipped query expansion because latency budget was exceeded.",
            vec![field("phase_deadline_ms", expansion_deadline.to_string())],
        );
        trace.annotate("Latency-first cutoff skipped investigation query expansion.");
        return Ok(Vec::new());
    }

    let mut expanded = Vec::new();
    for term in &terms {
        let hits = match controller.search_hybrid_scored(
            SearchRequest {
                query: term.clone(),
                repo_text: SearchRepoTextMode::Off,
                limit_per_source: max_results as u32,
                expand_search_plan: false,
                hybrid_weights: None,
                hybrid_limits: None,
            },
            req.focus_node_id.clone(),
            max_results,
            req.hybrid_weights.clone(),
        ) {
            Ok(hits) => hits,
            Err(error) => {
                trace.finish_err(expansion_step, error.message.clone());
                return Err(error);
            }
        };
        expanded.extend(hits);
    }

    let hit_count = expanded.len();
    trace.finish_ok(
        expansion_step,
        vec![
            field("terms", terms.join(",")),
            field("hits", hit_count.to_string()),
        ],
    );
    Ok(expanded)
}

fn investigate_repo_text_fallback(
    controller: &AppController,
    req: &AgentAskRequest,
    prompt: &str,
    max_results: usize,
    ask_started_at: Instant,
    resolved_profile: &ResolvedProfile,
    trace: &mut TraceRecorder,
) -> Result<Vec<SearchHit>, ApiError> {
    let fallback_step = trace.start_step(
        AgentRetrievalStepKindDto::RepoTextFallback,
        vec![
            field("query_chars", prompt.len().to_string()),
            field("max_results", max_results.to_string()),
        ],
    );

    let fallback_deadline = phase_deadline_ms(req, 55, 100);
    if should_truncate_phase(resolved_profile, ask_started_at, fallback_deadline) {
        trace.finish_truncated(
            fallback_step,
            "Skipped repo-text fallback because latency budget was exceeded.",
            vec![field("phase_deadline_ms", fallback_deadline.to_string())],
        );
        trace.annotate("Latency-first cutoff skipped investigation repo-text fallback.");
        return Ok(Vec::new());
    }

    let results = match controller.search_results(SearchRequest {
        query: prompt.to_string(),
        repo_text: SearchRepoTextMode::On,
        limit_per_source: max_results as u32,
        expand_search_plan: false,
        hybrid_weights: None,
        hybrid_limits: None,
    }) {
        Ok(results) => results,
        Err(error) => {
            trace.finish_err(fallback_step, error.message.clone());
            return Err(error);
        }
    };
    let stats = results.repo_text_stats.clone();
    let mut hits = results.repo_text_hits;
    hits.truncate(max_results);
    let mut output = vec![
        field("repo_text_hits", hits.len().to_string()),
        field("origin", SearchHitOrigin::TextMatch.as_str()),
    ];
    if let Some(stats) = stats.as_ref() {
        output.push(field("scanned_files", stats.scanned_file_count.to_string()));
        output.push(field("scanned_bytes", stats.scanned_byte_count.to_string()));
        output.push(field("file_cap", stats.file_cap.to_string()));
        output.push(field("byte_cap", stats.byte_cap.to_string()));
        output.push(field("time_cap_ms", stats.time_cap_ms.to_string()));
        output.push(field("scan_truncated", stats.truncated.to_string()));
        if let Some(reason) = stats.reason.as_deref() {
            output.push(field("scan_reason", reason.to_string()));
        }
        if let Some(action) = stats.action.as_deref() {
            output.push(field("scan_action", action.to_string()));
        }
    }
    let scan_truncated = stats.as_ref().is_some_and(|stats| stats.truncated);
    if scan_truncated {
        trace.finish_truncated(
            fallback_step,
            "Repo-text fallback stopped at a configured scan cap.",
            output,
        );
    } else {
        trace.finish_ok(fallback_step, output);
    }
    if !hits.is_empty() {
        trace.annotate(
            "Repo-text fallback returned file/line evidence only; unresolved text hits are not treated as symbols.",
        );
    }
    if let Some(stats) = stats.as_ref()
        && stats.truncated
        && let Some(action) = stats.action.as_deref()
    {
        trace.annotate(format!("Repo-text fallback truncated: {action}"));
    }
    Ok(hits)
}

fn to_citation_from_hit(
    hit: &SearchHit,
    subgraph_id: Option<&str>,
    primary_graph: Option<&GraphResponse>,
    include_evidence: bool,
) -> AgentCitationDto {
    AgentCitationDto {
        node_id: hit.node_id.clone(),
        display_name: hit.display_name.clone(),
        kind: hit.kind,
        file_path: hit.file_path.clone(),
        line: hit.line,
        score: hit.score,
        origin: hit.origin,
        resolvable: hit.resolvable,
        subgraph_id: subgraph_id.map(ToOwned::to_owned),
        evidence_edge_ids: if include_evidence && hit.resolvable {
            evidence_edge_ids_for_node(primary_graph, &hit.node_id)
        } else {
            Vec::new()
        },
        retrieval_score_breakdown: include_evidence
            .then(|| hit.score_breakdown.clone())
            .flatten(),
    }
}

fn evidence_edge_ids_for_node(
    primary_graph: Option<&GraphResponse>,
    node_id: &codestory_contracts::api::NodeId,
) -> Vec<EdgeId> {
    let Some(graph) = primary_graph else {
        return Vec::new();
    };

    let mut edge_ids = graph
        .edges
        .iter()
        .filter(|edge| edge.source == *node_id || edge.target == *node_id)
        .map(|edge| edge.id.clone())
        .collect::<Vec<_>>();
    edge_ids.sort_by(|left, right| left.0.cmp(&right.0));
    edge_ids.truncate(12);
    edge_ids
}

fn trail_truncated_annotation(trail_number: usize, max_nodes: u32) -> String {
    format!("Trail {trail_number} was truncated at max_nodes={max_nodes}.")
}

fn sanitize_plan_filters(plan: &TrailPlan, options: &TrailFilterOptionsDto) -> TrailPlan {
    let mut sanitized = plan.clone();

    if !options.edge_kinds.is_empty() && !plan.edge_filter.is_empty() {
        sanitized
            .edge_filter
            .retain(|kind| options.edge_kinds.contains(kind));
    }

    if !options.node_kinds.is_empty() && !plan.node_filter.is_empty() {
        sanitized
            .node_filter
            .retain(|kind| options.node_kinds.contains(kind));
    }

    sanitized
}

struct SourceContextRequest<'a> {
    req: &'a AgentAskRequest,
    prompt: &'a str,
    resolved_profile: &'a ResolvedProfile,
    ask_started_at: Instant,
    focused_node: Option<&'a NodeDetailsDto>,
    fallback_focus: bool,
}

fn maybe_read_source_context(
    controller: &AppController,
    request: SourceContextRequest<'_>,
    trace: &mut TraceRecorder,
) -> Option<FocusedSourceContext> {
    let source_step = trace.start_step(
        AgentRetrievalStepKindDto::SourceRead,
        vec![field(
            "enabled",
            request.resolved_profile.enable_source_reads.to_string(),
        )],
    );

    if !request.resolved_profile.enable_source_reads {
        trace.finish_skipped(
            source_step,
            "Source reads disabled by profile configuration.",
            Vec::new(),
        );
        return None;
    }

    if !needs_source_context(request.prompt) && !request.fallback_focus {
        trace.finish_skipped(
            source_step,
            "Prompt does not request source-level context.",
            Vec::new(),
        );
        return None;
    }

    let source_deadline = phase_deadline_ms(request.req, 50, 100);
    if should_truncate_phase(
        request.resolved_profile,
        request.ask_started_at,
        source_deadline,
    ) {
        trace.finish_truncated(
            source_step,
            "Skipped source read because latency-first phase budget was exceeded.",
            vec![field("phase_deadline_ms", source_deadline.to_string())],
        );
        trace.annotate("Latency-first cutoff skipped source reads.");
        return None;
    }

    let Some(node) = request.focused_node else {
        trace.finish_skipped(source_step, "No focused node available.", Vec::new());
        return None;
    };

    let (Some(path), Some(line)) = (node.file_path.clone(), node.start_line) else {
        trace.finish_skipped(
            source_step,
            "Focused node has no file path and line metadata.",
            Vec::new(),
        );
        return None;
    };

    match controller.bounded_file_snippet(
        &path,
        line,
        6,
        request.resolved_profile.max_source_bytes,
        SOURCE_SNIPPET_TRUNCATION_SUFFIX,
    ) {
        Ok((resolved_path, bounded)) => {
            let context = FocusedSourceContext {
                path: resolved_path,
                line,
                snippet: bounded.markdown,
            };
            trace.finish_ok(
                source_step,
                vec![
                    field("path", context.path.clone()),
                    field("line", context.line.to_string()),
                    field(
                        "max_source_bytes",
                        request.resolved_profile.max_source_bytes.to_string(),
                    ),
                    field("snippet_bytes", context.snippet.len().to_string()),
                    field("truncated", bounded.truncated.to_string()),
                ],
            );
            Some(context)
        }
        Err(error) => {
            trace.finish_err(source_step, error.message.clone());
            None
        }
    }
}

fn needs_source_context(prompt: &str) -> bool {
    let normalized = prompt.to_ascii_lowercase();
    [
        "code",
        "snippet",
        "implementation",
        "source",
        "line",
        "read",
    ]
    .iter()
    .any(|keyword| normalized.contains(keyword))
}

#[cfg(test)]
struct BoundedMarkdownSnippet {
    markdown: String,
    truncated: bool,
}

#[cfg(test)]
fn bounded_markdown_snippet(
    text: &str,
    focus_line: Option<u32>,
    context_lines: usize,
    max_bytes: usize,
) -> BoundedMarkdownSnippet {
    let mut snippet = crate::markdown_snippet(text, focus_line, context_lines);
    if snippet.len() <= max_bytes {
        return BoundedMarkdownSnippet {
            markdown: snippet,
            truncated: false,
        };
    }

    if max_bytes <= SOURCE_SNIPPET_TRUNCATION_SUFFIX.len() {
        snippet = SOURCE_SNIPPET_TRUNCATION_SUFFIX.to_string();
        while snippet.len() > max_bytes {
            snippet.pop();
        }
        return BoundedMarkdownSnippet {
            markdown: snippet,
            truncated: true,
        };
    }

    let content_budget = max_bytes - SOURCE_SNIPPET_TRUNCATION_SUFFIX.len();
    while snippet.len() > content_budget {
        snippet.pop();
    }
    snippet.push_str(SOURCE_SNIPPET_TRUNCATION_SUFFIX);
    debug_assert!(snippet.len() <= max_bytes);
    BoundedMarkdownSnippet {
        markdown: snippet,
        truncated: true,
    }
}
fn build_mermaid_artifacts(
    profile: &ResolvedProfile,
    req: &AgentAskRequest,
    prompt: &str,
    ask_started_at: Instant,
    bundle: &RetrievalBundle,
    trace: &mut TraceRecorder,
) -> Vec<GraphArtifactDto> {
    let mermaid_step = trace.start_step(
        AgentRetrievalStepKindDto::MermaidSynthesis,
        vec![field("existing_graphs", bundle.graphs.len().to_string())],
    );

    let mut artifacts = Vec::new();
    let mermaid_deadline = phase_deadline_ms(req, 85, 100);
    if should_truncate_phase(profile, ask_started_at, mermaid_deadline) {
        trace.finish_truncated(
            mermaid_step,
            "Skipped mermaid synthesis because latency budget was exceeded.",
            vec![field("phase_deadline_ms", mermaid_deadline.to_string())],
        );
        trace.annotate("Latency-first cutoff skipped mermaid synthesis.");
        return artifacts;
    }

    let primary_graph = bundle
        .primary_graph
        .clone()
        .or_else(|| first_uml_graph(&bundle.graphs));

    if let Some(graph) = primary_graph {
        artifacts.push(GraphArtifactDto::Mermaid {
            id: "mermaid-overview".to_string(),
            title: "Graph Overview".to_string(),
            diagram: "flowchart".to_string(),
            mermaid_syntax: mermaid_flowchart(&graph),
        });

        if matches!(
            profile.preset,
            codestory_contracts::api::AgentRetrievalPresetDto::Callflow
        ) {
            artifacts.push(GraphArtifactDto::Mermaid {
                id: "mermaid-sequence".to_string(),
                title: "Sequence Narrative".to_string(),
                diagram: "sequenceDiagram".to_string(),
                mermaid_syntax: mermaid_sequence(&graph),
            });
        }

        if prompt.to_ascii_lowercase().contains("timeline") {
            artifacts.push(GraphArtifactDto::Mermaid {
                id: "mermaid-timeline".to_string(),
                title: "Timeline".to_string(),
                diagram: "gantt".to_string(),
                mermaid_syntax: mermaid_gantt(&bundle.hits),
            });
        }
    }

    if artifacts.is_empty() {
        artifacts.push(GraphArtifactDto::Mermaid {
            id: "mermaid-fallback".to_string(),
            title: "Retrieval Fallback".to_string(),
            diagram: "flowchart".to_string(),
            mermaid_syntax: fallback_mermaid(prompt, bundle.hits.len()),
        });
    }

    trace.finish_ok(
        mermaid_step,
        vec![field("mermaid_count", artifacts.len().to_string())],
    );
    artifacts
}

fn first_uml_graph(graphs: &[GraphArtifactDto]) -> Option<GraphResponse> {
    graphs.iter().find_map(|graph| match graph {
        GraphArtifactDto::Uml { graph, .. } => Some(graph.clone()),
        GraphArtifactDto::Mermaid { .. } => None,
    })
}

fn first_edge_id_from_graphs(
    graphs: &[GraphArtifactDto],
) -> Option<codestory_contracts::api::EdgeId> {
    graphs.iter().find_map(|graph| match graph {
        GraphArtifactDto::Uml { graph, .. } => graph.edges.first().map(|edge| edge.id.clone()),
        GraphArtifactDto::Mermaid { .. } => None,
    })
}

fn build_sections(
    prompt: &str,
    resolved_profile: &ResolvedProfile,
    bundle: &RetrievalBundle,
    source_context: Option<&FocusedSourceContext>,
) -> Vec<AgentResponseSectionDto> {
    let mut sections = Vec::new();

    let mut analysis_blocks = vec![AgentResponseBlockDto::Markdown {
        markdown: "Answer assembled from indexed DB-first retrieval evidence.".to_string(),
    }];

    if let Some(primary_mermaid_id) = first_mermaid_graph_id(&bundle.graphs) {
        analysis_blocks.push(AgentResponseBlockDto::Mermaid {
            graph_id: primary_mermaid_id,
        });
    }

    sections.push(AgentResponseSectionDto {
        id: "analysis".to_string(),
        title: "Analysis".to_string(),
        blocks: analysis_blocks,
    });

    sections.push(AgentResponseSectionDto {
        id: "retrieval-evidence".to_string(),
        title: "Retrieval Evidence".to_string(),
        blocks: vec![AgentResponseBlockDto::Markdown {
            markdown: retrieval_markdown(prompt, resolved_profile, bundle, source_context),
        }],
    });

    let mermaid_ids = bundle
        .graphs
        .iter()
        .filter_map(|graph| match graph {
            GraphArtifactDto::Mermaid { id, .. } => Some(id.clone()),
            GraphArtifactDto::Uml { .. } => None,
        })
        .collect::<Vec<_>>();

    if !mermaid_ids.is_empty() {
        let mut blocks = vec![AgentResponseBlockDto::Markdown {
            markdown: "Mermaid diagrams generated from indexed graph retrieval.".to_string(),
        }];
        for graph_id in mermaid_ids {
            blocks.push(AgentResponseBlockDto::Mermaid { graph_id });
        }

        sections.push(AgentResponseSectionDto {
            id: "diagrams".to_string(),
            title: "Diagrams".to_string(),
            blocks,
        });
    }

    sections
}

fn retrieval_markdown(
    prompt: &str,
    profile: &ResolvedProfile,
    bundle: &RetrievalBundle,
    source_context: Option<&FocusedSourceContext>,
) -> String {
    let mut markdown = String::new();

    let _ = writeln!(markdown, "Prompt: **{}**", prompt.trim().replace('\n', " "));
    let _ = writeln!(
        markdown,
        "Resolved profile: `{:?}` (`{:?}` mode)",
        profile.preset, profile.policy_mode
    );
    let _ = writeln!(
        markdown,
        "Indexed hits: `{}` | Graph artifacts: `{}`",
        bundle.hits.len(),
        bundle.graphs.len()
    );

    if let Some(node) = bundle.focused_node.as_ref() {
        let _ = writeln!(
            markdown,
            "Focused symbol: **{}** (`{:?}`)",
            node.display_name, node.kind
        );
    }

    if let Some(source) = source_context {
        let _ = writeln!(
            markdown,
            "\nSource snippet from `{}`:{}:\n",
            source.path, source.line
        );
        markdown.push_str(&source.snippet);
        markdown.push('\n');
    }

    markdown.push_str("\nWhat I checked:\n");
    markdown.push_str("- Initial indexed-symbol search with current hybrid ranking.\n");
    if bundle.fallback_used {
        markdown.push_str("- Deterministic query expansion because initial hits were weak.\n");
    }
    if bundle.repo_text_fallback_used {
        markdown.push_str("- Repo-text/file fallback for literal file-line evidence.\n");
    }
    if bundle.repo_explanation_fallback_used {
        markdown.push_str("- Grounding snapshot fallback for broad repo overview evidence.\n");
    }
    if !bundle.fallback_used && should_investigate(profile) {
        markdown.push_str("- No fallback was needed because initial hits cleared the investigation confidence gate.\n");
    }

    if bundle.hits.is_empty() {
        markdown.push_str(
            "\nNo indexed symbol matches found. Try: symbol names, module paths, or re-run indexing.\n",
        );
    } else {
        markdown.push_str("\nTop indexed matches:\n");
        for hit in bundle.hits.iter().take(6) {
            write_indexed_match_markdown(&mut markdown, hit);
        }
    }

    if should_investigate(profile) && weak_initial_hits(prompt, &bundle.hits) {
        markdown.push_str("\nGaps:\n");
        markdown.push_str(
            "- Confidence is low: investigation mode could not find enough strong indexed-symbol evidence within its bounded search.\n",
        );
        if bundle.hits.iter().any(SearchHit::is_text_match) {
            markdown.push_str(
                "- Repo-text hits cite file/line locations only and were not treated as resolvable symbols.\n",
            );
        }
    }

    markdown
}

fn write_indexed_match_markdown(markdown: &mut String, hit: &SearchHit) {
    let _ = writeln!(
        markdown,
        "- **{}** [{:?}] origin `{}` resolvable `{}` score `{:.3}`{}",
        hit.display_name,
        hit.kind,
        hit.origin.as_str(),
        hit.resolvable,
        hit.score,
        search_hit_location_suffix(hit)
    );
}

fn search_hit_location_suffix(hit: &SearchHit) -> String {
    match (&hit.file_path, hit.line) {
        (Some(path), Some(line)) => format!(" ({}:{})", path, line),
        (Some(path), None) => format!(" ({})", path),
        _ => String::new(),
    }
}

fn first_mermaid_graph_id(graphs: &[GraphArtifactDto]) -> Option<String> {
    graphs.iter().find_map(|graph| match graph {
        GraphArtifactDto::Mermaid { id, .. } => Some(id.clone()),
        GraphArtifactDto::Uml { .. } => None,
    })
}

fn summarize_response(resolved_profile: &ResolvedProfile, bundle: &RetrievalBundle) -> String {
    format!(
        "DB-first retrieval ({:?}/{:?}) returned {} indexed match(es) and {} graph artifact(s).",
        resolved_profile.preset,
        resolved_profile.policy_mode,
        bundle.hits.len(),
        bundle.graphs.len()
    )
}

fn next_request_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("ask-{}", nanos)
}

#[allow(dead_code)]
fn prompt_search_terms(prompt: &str) -> Vec<String> {
    const STOPWORDS: &[&str] = &[
        "a",
        "an",
        "and",
        "are",
        "as",
        "at",
        "be",
        "by",
        "can",
        "does",
        "for",
        "from",
        "how",
        "in",
        "is",
        "it",
        "of",
        "on",
        "or",
        "repo",
        "repository",
        "the",
        "this",
        "to",
        "what",
        "where",
        "which",
        "why",
        "with",
        "work",
        "works",
    ];

    let mut terms = Vec::new();
    let mut current = String::new();
    let mut seen = HashSet::new();

    for ch in prompt.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            current.push(ch.to_ascii_lowercase());
            continue;
        }

        if current.len() >= 3
            && !STOPWORDS.contains(&current.as_str())
            && seen.insert(current.clone())
        {
            terms.push(current.clone());
        }
        current.clear();
    }

    if current.len() >= 3 && !STOPWORDS.contains(&current.as_str()) && seen.insert(current.clone())
    {
        terms.push(current);
    }

    terms
}

#[allow(dead_code)]
fn merge_search_hits(into: &mut Vec<SearchHit>, additional: Vec<SearchHit>, max_candidates: usize) {
    let mut by_id = HashMap::<codestory_contracts::api::NodeId, SearchHit>::new();

    for hit in into.drain(..) {
        by_id.insert(hit.node_id.clone(), hit);
    }

    for hit in additional {
        by_id
            .entry(hit.node_id.clone())
            .and_modify(|existing| {
                if hit.score > existing.score {
                    *existing = hit.clone();
                }
            })
            .or_insert(hit);
    }

    let mut merged = by_id.into_values().collect::<Vec<_>>();
    merged.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(Ordering::Equal)
    });
    merged.truncate(max_candidates);
    *into = merged;
}

fn merge_scored_hits(
    into: &mut Vec<HybridSearchScoredHit>,
    additional: Vec<HybridSearchScoredHit>,
    max_candidates: usize,
) {
    let mut by_id = HashMap::<codestory_contracts::api::NodeId, HybridSearchScoredHit>::new();

    for hit in into.drain(..) {
        by_id.insert(hit.hit.node_id.clone(), hit);
    }

    for hit in additional {
        by_id
            .entry(hit.hit.node_id.clone())
            .and_modify(|existing| {
                if hit.total_score > existing.total_score {
                    *existing = hit.clone();
                }
            })
            .or_insert(hit);
    }

    let mut merged = by_id.into_values().collect::<Vec<_>>();
    merged.sort_by(|left, right| {
        right
            .total_score
            .partial_cmp(&left.total_score)
            .unwrap_or(Ordering::Equal)
    });
    merged.truncate(max_candidates);
    *into = merged;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::profiles::ResolvedProfile;

    fn latency_profile() -> ResolvedProfile {
        ResolvedProfile {
            preset: codestory_contracts::api::AgentRetrievalPresetDto::Architecture,
            policy_mode: AgentRetrievalPolicyModeDto::LatencyFirst,
            trail_plans: Vec::new(),
            include_edge_occurrences: false,
            enable_source_reads: true,
            max_search_results: 25,
            max_source_bytes: 32 * 1024,
        }
    }

    fn test_search_hit(node_id: &str, score: f32) -> SearchHit {
        SearchHit {
            node_id: codestory_contracts::api::NodeId(node_id.to_string()),
            display_name: node_id.to_string(),
            kind: codestory_contracts::api::NodeKind::FUNCTION,
            file_path: None,
            line: None,
            score,
            origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
            match_quality: None,
            resolvable: true,
            score_breakdown: None,
        }
    }

    fn test_semantic_only_hit(node_id: &str, score: f32) -> SearchHit {
        let mut hit = test_search_hit(node_id, score);
        hit.score_breakdown = Some(RetrievalScoreBreakdownDto {
            lexical: 0.0,
            semantic: score,
            graph: 0.0,
            total: score,
        });
        hit
    }

    fn test_packet_citation(display_name: &str, file_path: &str, score: f32) -> AgentCitationDto {
        AgentCitationDto {
            node_id: NodeId(display_name.to_string()),
            display_name: display_name.to_string(),
            kind: codestory_contracts::api::NodeKind::FUNCTION,
            file_path: Some(file_path.to_string()),
            line: Some(10),
            score,
            origin: SearchHitOrigin::IndexedSymbol,
            resolvable: true,
            subgraph_id: None,
            evidence_edge_ids: Vec::new(),
            retrieval_score_breakdown: Some(RetrievalScoreBreakdownDto {
                lexical: 0.4,
                semantic: 0.2,
                graph: 0.3,
                total: score,
            }),
        }
    }

    fn packet_answer_fixture(question: &str, citations: Vec<AgentCitationDto>) -> AgentAnswerDto {
        AgentAnswerDto {
            answer_id: "packet-fixture".to_string(),
            prompt: question.to_string(),
            summary: "Fixture packet is covered by cited anchors.".to_string(),
            freshness: None,
            sections: vec![AgentResponseSectionDto {
                id: "answer".to_string(),
                title: "Answer".to_string(),
                blocks: vec![AgentResponseBlockDto::Markdown {
                    markdown: "Packet answer assembled from cited anchors.".to_string(),
                }],
            }],
            citations,
            subgraph_ids: Vec::new(),
            retrieval_version: "test".to_string(),
            graphs: Vec::new(),
            retrieval_trace: codestory_contracts::api::AgentRetrievalTraceDto {
                request_id: "packet-fixture".to_string(),
                resolved_profile: AgentRetrievalPresetDto::Architecture,
                policy_mode: AgentRetrievalPolicyModeDto::LatencyFirst,
                total_latency_ms: 1,
                sla_target_ms: None,
                sla_missed: false,
                annotations: Vec::new(),
                steps: Vec::new(),
            },
        }
    }

    fn build_sufficient_packet_fixture(
        question: &str,
        task_class: PacketTaskClassDto,
        citations: Vec<AgentCitationDto>,
    ) -> (AgentAnswerDto, PacketSufficiencyDto) {
        let limits = packet_budget_limits(PacketBudgetModeDto::Compact);
        let mut answer = packet_answer_fixture(question, citations);
        rank_packet_evidence(question, &mut answer);
        append_packet_evidence_sections(&mut answer, task_class, &limits);
        let budget =
            apply_packet_budget(question, PacketBudgetModeDto::Compact, limits, &mut answer);
        let sufficiency = build_packet_sufficiency(question, task_class, &answer, &budget);
        (answer, sufficiency)
    }

    #[test]
    fn investigation_mode_is_explicit_preset_only() {
        let mut profile = latency_profile();
        profile.policy_mode = AgentRetrievalPolicyModeDto::CompletenessFirst;
        assert!(!should_investigate(&profile));

        profile.preset = codestory_contracts::api::AgentRetrievalPresetDto::Investigate;
        assert!(should_investigate(&profile));
    }

    #[test]
    fn weak_initial_hits_use_normalized_search_scores() {
        assert!(!weak_initial_hits(
            "strong",
            &[
                test_search_hit("strong", 0.31),
                test_search_hit("second", 0.20),
                test_search_hit("third", 0.10),
            ]
        ));
        assert!(weak_initial_hits(
            "weak",
            &[
                test_search_hit("weak", 0.29),
                test_search_hit("second", 0.20),
                test_search_hit("third", 0.10),
            ]
        ));
        assert!(weak_initial_hits(
            "too_few",
            &[test_search_hit("too_few", 0.29)]
        ));
    }

    #[test]
    fn weak_initial_hits_treat_semantic_only_matches_as_low_confidence() {
        assert!(weak_initial_hits(
            "unrelated billing conveyor",
            &[
                test_semantic_only_hit("semantic_one", 0.90),
                test_semantic_only_hit("semantic_two", 0.80),
                test_semantic_only_hit("semantic_three", 0.70),
            ]
        ));
    }

    #[test]
    fn weak_initial_hits_accept_prompt_anchored_symbol_names() {
        assert!(!weak_initial_hits(
            "Where is exact_symbol_anchor used?",
            &[test_semantic_only_hit("exact_symbol_anchor", 0.90)]
        ));
    }

    #[test]
    fn investigation_focus_anchor_prefers_prompt_named_symbol() {
        let hit = test_semantic_only_hit("exact_symbol_anchor", 0.05);
        assert_eq!(
            investigation_focus_anchor("Explain exact_symbol_anchor", &[hit])
                .expect("prompt-named hit should become focus")
                .0,
            "exact_symbol_anchor"
        );
        assert!(
            investigation_focus_anchor(
                "Explain unrelated behavior",
                &[test_semantic_only_hit("exact_symbol_anchor", 0.90)]
            )
            .is_none()
        );
    }

    #[test]
    fn repo_explanation_prompt_detection_is_broad_but_not_symbolic() {
        assert!(is_repo_explanation_prompt(
            "How does this repo fit together?"
        ));
        assert!(is_repo_explanation_prompt(
            "Explain the project architecture"
        ));
        assert!(!is_repo_explanation_prompt(
            "Where is build_llm_symbol_doc_text used?"
        ));
    }

    #[test]
    fn packet_plan_infers_task_class_and_code_terms() {
        let plan = build_packet_plan(
            "Trace the /api/users route through AppController and UserStore",
            None,
        );

        assert_eq!(plan.task_class, PacketTaskClassDto::RouteTracing);
        assert!(plan.inferred_task_class);
        assert!(
            plan.queries.iter().any(|query| query.query == "/api/users"),
            "route-like terms should become concrete packet queries: {plan:?}"
        );
        assert!(
            plan.queries
                .iter()
                .any(|query| query.query == "AppController"),
            "CamelCase symbols should become concrete packet queries: {plan:?}"
        );
    }

    #[test]
    fn requested_packet_task_class_overrides_heuristic() {
        let plan = build_packet_plan(
            "What would change if the indexing cache format moved?",
            Some(PacketTaskClassDto::ChangeImpact),
        );

        assert_eq!(plan.task_class, PacketTaskClassDto::ChangeImpact);
        assert!(!plan.inferred_task_class);
        assert!(
            plan.queries
                .iter()
                .any(|query| query.query.contains("affected")),
            "change impact plans should seed affected-symbol queries: {plan:?}"
        );
    }

    #[test]
    fn packet_plan_expands_task_wording_without_fixture_specific_anchors() {
        let plan = build_packet_plan(
            "Explain how a full indexing run moves from the CLI into runtime orchestration, file discovery, symbol extraction, persistence, and search or snapshot refresh.",
            Some(PacketTaskClassDto::ArchitectureExplanation),
        );
        let queries = plan
            .queries
            .iter()
            .map(|query| query.query.as_str())
            .collect::<Vec<_>>();

        for expected in [
            "indexing",
            "runtime",
            "full_indexing",
            "IndexingRun",
            "RuntimeOrchestration",
            "architecture entrypoint",
            "runtime flow",
        ] {
            assert!(
                queries.contains(&expected),
                "expected generic probe {expected} in packet plan: {queries:?}"
            );
        }
        for fixture_anchor in [
            "run_index",
            "IndexService",
            "WorkspaceIndexer",
            "flush_projection_batch",
            "SnapshotStore",
        ] {
            assert!(
                !queries.contains(&fixture_anchor),
                "packet planner should not inject fixture-specific anchor {fixture_anchor}: {queries:?}"
            );
        }
    }

    #[test]
    fn symbol_ownership_packet_plan_seeds_generic_ownership_terms() {
        let question = "Explain which modules own application creation, app-level rendering, response serialization, file sending, and view lookup.";
        let plan = build_packet_plan(question, Some(PacketTaskClassDto::SymbolOwnership));
        let queries = plan
            .queries
            .iter()
            .map(|query| query.query.as_str())
            .collect::<Vec<_>>();

        for expected in [
            "references",
            "callers",
            "definition references",
            "application",
            "view",
            "lookup",
            "application_creation",
            "ApplicationCreation",
        ] {
            assert!(
                queries.contains(&expected),
                "expected {expected} in generic ownership packet plan: {queries:?}"
            );
        }
        for fixture_anchor in ["createApplication", "lib/express.js", "lib/response.js"] {
            assert!(
                !queries.contains(&fixture_anchor),
                "ownership planning should not inject fixture-specific anchor {fixture_anchor}: {queries:?}"
            );
        }
    }

    #[test]
    fn bug_packet_plan_seeds_generic_failure_terms_and_prompt_identifiers() {
        let question =
            "Localize an app.param callback decode bug through router parameter handling.";
        let plan = build_packet_plan(question, Some(PacketTaskClassDto::BugLocalization));
        let queries = plan
            .queries
            .iter()
            .map(|query| query.query.as_str())
            .collect::<Vec<_>>();

        for expected in [
            "app.param",
            "param",
            "callback",
            "error",
            "validate",
            "error path",
            "failure handling",
        ] {
            assert!(
                queries.contains(&expected),
                "expected {expected} in generic bug packet plan: {queries:?}"
            );
        }
        for fixture_anchor in ["proto.param", "Layer.prototype.match", "test/app.param.js"] {
            assert!(
                !queries.contains(&fixture_anchor),
                "bug planning should not inject fixture-specific anchor {fixture_anchor}: {queries:?}"
            );
        }
    }

    #[test]
    fn route_tracing_packet_plan_seeds_generic_route_terms() {
        let question = "Trace how an application registers middleware and routes, then dispatches an incoming request through router layers to a route handler.";
        let plan = build_packet_plan(question, Some(PacketTaskClassDto::RouteTracing));
        let queries = plan
            .queries
            .iter()
            .map(|query| query.query.as_str())
            .collect::<Vec<_>>();

        for expected in [
            "router",
            "handler",
            "route",
            "middleware",
            "dispatch",
            "route handler endpoint",
        ] {
            assert!(
                queries.contains(&expected),
                "expected {expected} in route tracing packet plan: {queries:?}"
            );
        }
        for fixture_anchor in [
            "createApplication",
            "lib/router/layer.js",
            "Router.StrictSlash",
        ] {
            assert!(
                !queries.contains(&fixture_anchor),
                "route tracing should not inject fixture-specific anchor {fixture_anchor}: {queries:?}"
            );
        }
    }

    #[test]
    fn packet_supported_claims_use_generic_evidence_roles() {
        let limits = packet_budget_limits(PacketBudgetModeDto::Compact);
        let mut answer = AgentAnswerDto {
            answer_id: "generic-fixture".to_string(),
            prompt: "Explain the packet evidence roles.".to_string(),
            summary: "Generic evidence roles are covered.".to_string(),
            freshness: None,
            sections: Vec::new(),
            citations: vec![
                test_packet_citation("CliCommand", "crates/tool-cli/src/main.rs", 0.8),
                test_packet_citation("RuntimeCoordinator", "crates/core/src/runtime.rs", 0.8),
                test_packet_citation("WorkspacePlan", "crates/core/src/workspace/plan.rs", 0.8),
                test_packet_citation("GraphIndexer", "crates/indexer/src/lib.rs", 0.8),
                test_packet_citation("ProjectionStore", "crates/store/src/projection.rs", 0.8),
                test_packet_citation("SnapshotRefresh", "crates/store/src/snapshot.rs", 0.8),
                test_packet_citation("RouteHandler", "src/routes/user.rs", 0.8),
                test_packet_citation("PacketRegression", "tests/packet_flow.rs", 0.8),
            ],
            subgraph_ids: Vec::new(),
            retrieval_version: "test".to_string(),
            graphs: Vec::new(),
            retrieval_trace: codestory_contracts::api::AgentRetrievalTraceDto {
                request_id: "generic-fixture".to_string(),
                resolved_profile: AgentRetrievalPresetDto::Architecture,
                policy_mode: AgentRetrievalPolicyModeDto::LatencyFirst,
                total_latency_ms: 1,
                sla_target_ms: None,
                sla_missed: false,
                annotations: Vec::new(),
                steps: Vec::new(),
            },
        };

        append_packet_evidence_sections(
            &mut answer,
            PacketTaskClassDto::ArchitectureExplanation,
            &limits,
        );
        let text = answer
            .sections
            .iter()
            .flat_map(|section| &section.blocks)
            .filter_map(|block| match block {
                AgentResponseBlockDto::Markdown { markdown } => Some(markdown.as_str()),
                AgentResponseBlockDto::Mermaid { .. } => None,
            })
            .collect::<Vec<_>>()
            .join("\n");

        for expected_claim in [
            "The command or public entrypoint for this flow is anchored by `CliCommand`",
            "Runtime orchestration is anchored by `RuntimeCoordinator`",
            "Workspace discovery or planning is anchored by `WorkspacePlan`",
            "Symbol extraction is anchored by `GraphIndexer`",
            "Persistence or search projection is anchored by `ProjectionStore`",
            "Snapshot refresh is anchored by `SnapshotRefresh`",
            "Route handling is anchored by `RouteHandler`",
            "Regression coverage for this flow is anchored by `PacketRegression`",
        ] {
            assert!(
                text.contains(expected_claim),
                "generic packet claims should include {expected_claim}: {text}"
            );
        }
    }

    #[test]
    fn packet_ranking_demotes_test_paths_without_fixture_specific_boosts() {
        let question = "Trace route dispatch through a handler.";
        let mut answer = AgentAnswerDto {
            answer_id: "rank-fixture".to_string(),
            prompt: question.to_string(),
            summary: "Route evidence is covered by cited anchors.".to_string(),
            freshness: None,
            sections: Vec::new(),
            citations: vec![
                test_packet_citation("RouteHandler test", "tests/router_handler.rs", 5.0),
                test_packet_citation("RouteHandler", "src/router/handler.rs", 0.5),
            ],
            subgraph_ids: Vec::new(),
            retrieval_version: "test".to_string(),
            graphs: Vec::new(),
            retrieval_trace: codestory_contracts::api::AgentRetrievalTraceDto {
                request_id: "rank-fixture".to_string(),
                resolved_profile: AgentRetrievalPresetDto::Architecture,
                policy_mode: AgentRetrievalPolicyModeDto::LatencyFirst,
                total_latency_ms: 1,
                sla_target_ms: None,
                sla_missed: false,
                annotations: Vec::new(),
                steps: Vec::new(),
            },
        };

        rank_packet_evidence(question, &mut answer);
        assert_eq!(answer.citations[0].display_name, "RouteHandler");
    }

    #[test]
    fn sufficient_packets_stop_broad_exploration_across_task_classes() {
        let fixtures = [
            (
                PacketTaskClassDto::ArchitectureExplanation,
                "Explain how the command runtime loads a workspace plan and refreshes snapshots.",
                vec![
                    test_packet_citation("CliCommand", "crates/app-cli/src/main.rs", 0.9),
                    test_packet_citation(
                        "RuntimeCoordinator",
                        "crates/app-runtime/src/runtime.rs",
                        0.9,
                    ),
                    test_packet_citation("WorkspacePlan", "crates/workspace/src/plan.rs", 0.8),
                ],
                "Runtime orchestration is anchored by `RuntimeCoordinator`",
                "crates/app-runtime/src/runtime.rs",
            ),
            (
                PacketTaskClassDto::BugLocalization,
                "Find the failure handling path for decode validation.",
                vec![
                    test_packet_citation("RuntimeErrorHandler", "src/runtime/errors.rs", 0.9),
                    test_packet_citation("DecodeValidator", "src/validation/decode.rs", 0.8),
                    test_packet_citation("DecodeRegression", "tests/decode_regression.rs", 0.7),
                ],
                "Runtime orchestration is anchored by `RuntimeErrorHandler`",
                "src/runtime/errors.rs",
            ),
            (
                PacketTaskClassDto::ChangeImpact,
                "What changes if reference resolution behavior changes?",
                vec![
                    test_packet_citation(
                        "AffectedReferenceIndex",
                        "crates/indexer/src/references.rs",
                        0.9,
                    ),
                    test_packet_citation("ReferenceStore", "crates/store/src/references.rs", 0.8),
                    test_packet_citation(
                        "ReferenceRegression",
                        "tests/reference_regression.rs",
                        0.7,
                    ),
                ],
                "Symbol extraction is anchored by `AffectedReferenceIndex`",
                "crates/indexer/src/references.rs",
            ),
            (
                PacketTaskClassDto::RouteTracing,
                "Trace how a request reaches the selected handler.",
                vec![
                    test_packet_citation("RouteDispatcher", "src/router/dispatch.rs", 0.9),
                    test_packet_citation("RouteHandler", "src/router/handler.rs", 0.8),
                    test_packet_citation("RouteRegression", "tests/route_regression.rs", 0.7),
                ],
                "Route handling is anchored by `RouteDispatcher`",
                "src/router/dispatch.rs",
            ),
            (
                PacketTaskClassDto::SymbolOwnership,
                "Who owns workspace planning and graph state?",
                vec![
                    test_packet_citation(
                        "WorkspaceOwnerPlan",
                        "crates/workspace/src/ownership.rs",
                        0.9,
                    ),
                    test_packet_citation("GraphStateStore", "crates/store/src/graph.rs", 0.8),
                    test_packet_citation(
                        "OwnershipRegression",
                        "tests/ownership_regression.rs",
                        0.7,
                    ),
                ],
                "Workspace discovery or planning is anchored by `WorkspaceOwnerPlan`",
                "crates/workspace/src/ownership.rs",
            ),
            (
                PacketTaskClassDto::EditPlanning,
                "Plan the focused edit for configuration validation behavior.",
                vec![
                    test_packet_citation("ConfigValidator", "src/config/validator.rs", 0.9),
                    test_packet_citation("ConfigEditPlan", "src/config/edit_plan.rs", 0.8),
                    test_packet_citation("ConfigRegression", "tests/config_regression.rs", 0.7),
                ],
                "Regression coverage for this flow is anchored by `ConfigRegression`",
                "tests/config_regression.rs",
            ),
        ];

        for (task_class, question, citations, expected_claim, avoid_path) in fixtures {
            let (_answer, sufficiency) =
                build_sufficient_packet_fixture(question, task_class, citations);

            assert_eq!(
                sufficiency.status,
                PacketSufficiencyStatusDto::Sufficient,
                "task class {task_class:?} should be sufficient: {sufficiency:?}"
            );
            assert!(
                sufficiency.follow_up_commands.is_empty(),
                "sufficient {task_class:?} packets should not recommend broad follow-up commands: {sufficiency:?}"
            );
            assert!(
                sufficiency.open_next.is_empty(),
                "sufficient {task_class:?} packets should not name generic open-next work: {sufficiency:?}"
            );
            assert!(
                sufficiency
                    .covered_claims
                    .iter()
                    .any(|claim| claim.claim.contains(expected_claim)),
                "sufficient {task_class:?} packet should name the covered task claim `{expected_claim}`: {sufficiency:?}"
            );
            assert!(
                sufficiency
                    .avoid_opening
                    .iter()
                    .any(|entry| entry.contains(avoid_path)),
                "sufficient {task_class:?} packet should discourage reopening cited path `{avoid_path}`: {sufficiency:?}"
            );
        }
    }

    #[test]
    fn partial_and_insufficient_packets_recommend_targeted_followups() {
        let question = "Explain route dispatch with enough evidence to stop.";
        let mut partial_answer = packet_answer_fixture(
            question,
            vec![test_packet_citation(
                "RouteDispatcher",
                "src/router/dispatch.rs",
                0.8,
            )],
        );
        let mut budget = apply_packet_budget(
            question,
            PacketBudgetModeDto::Tiny,
            packet_budget_limits(PacketBudgetModeDto::Tiny),
            &mut partial_answer,
        );
        budget.truncated = true;
        budget.omitted_sections = vec!["output_bytes".to_string()];
        let partial = build_packet_sufficiency(
            question,
            PacketTaskClassDto::RouteTracing,
            &partial_answer,
            &budget,
        );

        assert_eq!(partial.status, PacketSufficiencyStatusDto::Partial);
        assert!(
            partial
                .follow_up_commands
                .iter()
                .any(|command| command.contains("--budget compact")),
            "partial packets should recommend the next deeper packet command: {partial:?}"
        );
        assert!(
            partial
                .follow_up_commands
                .iter()
                .any(|command| command.contains("codestory-cli search")),
            "partial packets should recommend targeted CodeStory search, not broad source reads: {partial:?}"
        );

        let mut weak_answer = packet_answer_fixture(
            question,
            vec![test_packet_citation(
                "RouteDispatcher",
                "src/router/dispatch.rs",
                0.8,
            )],
        );
        let weak_budget = apply_packet_budget(
            question,
            PacketBudgetModeDto::Compact,
            packet_budget_limits(PacketBudgetModeDto::Compact),
            &mut weak_answer,
        );
        let weak = build_packet_sufficiency(
            question,
            PacketTaskClassDto::RouteTracing,
            &weak_answer,
            &weak_budget,
        );
        assert_eq!(weak.status, PacketSufficiencyStatusDto::Partial);
        assert!(
            weak.gaps
                .iter()
                .any(|gap| gap.contains("at least 3 are required")),
            "single-citation route packets should name the coverage gap: {weak:?}"
        );

        let mut empty_answer = packet_answer_fixture(question, Vec::new());
        let empty_budget = apply_packet_budget(
            question,
            PacketBudgetModeDto::Compact,
            packet_budget_limits(PacketBudgetModeDto::Compact),
            &mut empty_answer,
        );
        let insufficient = build_packet_sufficiency(
            question,
            PacketTaskClassDto::RouteTracing,
            &empty_answer,
            &empty_budget,
        );

        assert_eq!(
            insufficient.status,
            PacketSufficiencyStatusDto::Insufficient
        );
        assert!(
            insufficient
                .follow_up_commands
                .iter()
                .any(|command| command.contains("codestory-cli index")),
            "insufficient packets should recommend indexing before broad exploration: {insufficient:?}"
        );
        assert!(
            insufficient
                .follow_up_commands
                .iter()
                .any(|command| command.contains("--repo-text on")),
            "insufficient packets should keep fallback exploration inside CodeStory: {insufficient:?}"
        );
    }

    #[test]
    fn merged_packet_latency_recomputes_sla_against_packet_budget() {
        let mut answer = packet_answer_fixture(
            "Explain the packet latency budget.",
            vec![
                test_packet_citation("A", "src/a.rs", 0.8),
                test_packet_citation("B", "src/b.rs", 0.8),
                test_packet_citation("C", "src/c.rs", 0.8),
            ],
        );
        answer.retrieval_trace.total_latency_ms = 900;
        answer.retrieval_trace.sla_missed = false;
        let mut subanswer =
            packet_answer_fixture("subquery", vec![test_packet_citation("D", "src/d.rs", 0.8)]);
        subanswer.retrieval_trace.total_latency_ms = 250;
        merge_packet_subanswer(
            &mut answer,
            subanswer,
            &PacketPlanQueryDto {
                query: "subquery".to_string(),
                purpose: "fixture".to_string(),
            },
        );

        PacketLatencyBudget {
            started_at: Instant::now(),
            target_ms: 1_000,
        }
        .apply_to_trace(&mut answer);

        assert_eq!(answer.retrieval_trace.total_latency_ms, 1_150);
        assert!(answer.retrieval_trace.sla_missed);
        assert_eq!(answer.retrieval_trace.sla_target_ms, Some(1_000));
    }

    #[test]
    fn citation_budget_truncation_keeps_sufficient_stop_signal() {
        let question = "Explain the compact packet stop rule.";
        let mut answer = packet_answer_fixture(
            question,
            (0..14)
                .map(|index| {
                    test_packet_citation(
                        &format!("symbol_{index}"),
                        &format!("src/file_{index}.rs"),
                        0.8,
                    )
                })
                .collect(),
        );
        let budget = apply_packet_budget(
            question,
            PacketBudgetModeDto::Compact,
            packet_budget_limits(PacketBudgetModeDto::Compact),
            &mut answer,
        );
        let sufficiency = build_packet_sufficiency(
            question,
            PacketTaskClassDto::ArchitectureExplanation,
            &answer,
            &budget,
        );

        assert!(
            budget.truncated && budget.omitted_sections.contains(&"citations".to_string()),
            "fixture should exercise normal citation budget truncation: {budget:?}"
        );
        assert_eq!(
            sufficiency.status,
            PacketSufficiencyStatusDto::Sufficient,
            "budgeted citation clipping should not force broad follow-up when the compact packet still has cited anchors: {sufficiency:?}"
        );
        assert!(sufficiency.follow_up_commands.is_empty());
        assert_eq!(answer.citations.len(), 10);
        assert!(
            sufficiency.gaps.is_empty(),
            "normal compact-budget truncation should stay in budget metadata, not sufficiency gaps: {sufficiency:?}"
        );
        assert!(budget.used.files <= budget.limits.max_files);
        assert!(budget.used.output_bytes <= budget.limits.max_output_bytes);
    }

    #[test]
    fn packet_output_budget_measures_serialized_packet_payload() {
        let question = "Explain the final packet payload budget.";
        let limits = PacketBudgetLimitsDto {
            max_anchors: 4,
            max_files: 4,
            max_snippets: 4,
            max_trail_edges: 4,
            max_output_bytes: 6 * 1024,
        };
        let max_output_bytes = limits.max_output_bytes;
        let mut answer = packet_answer_fixture(
            question,
            vec![test_packet_citation(
                "PacketBudget",
                "crates/codestory-runtime/src/agent/orchestrator.rs",
                0.8,
            )],
        );
        if let AgentResponseBlockDto::Markdown { markdown } = &mut answer.sections[0].blocks[0] {
            *markdown = "payload budget evidence ".repeat(6000);
        }
        let budget = apply_packet_budget(question, PacketBudgetModeDto::Tiny, limits, &mut answer);
        let sufficiency = build_packet_sufficiency(
            question,
            PacketTaskClassDto::ArchitectureExplanation,
            &answer,
            &budget,
        );
        let benchmark_trace = packet_benchmark_trace(&answer);
        let mut packet = AgentPacketDto {
            packet_id: answer.answer_id.clone(),
            question: question.to_string(),
            task_class: Some(PacketTaskClassDto::ArchitectureExplanation),
            plan: PacketPlanDto {
                task_class: PacketTaskClassDto::ArchitectureExplanation,
                inferred_task_class: false,
                queries: vec![PacketPlanQueryDto {
                    query: question.to_string(),
                    purpose: "fixture".to_string(),
                }],
                trace: Vec::new(),
            },
            answer,
            budget,
            sufficiency,
            benchmark_trace,
        };

        enforce_packet_output_budget(&mut packet);

        let serialized_len = serde_json::to_vec(&packet).expect("serialize packet").len();
        assert!(
            serialized_len <= max_output_bytes as usize,
            "serialized packet should honor max_output_bytes: {serialized_len} > {}",
            max_output_bytes
        );
        assert_eq!(packet.budget.used.output_bytes as usize, serialized_len);
        assert!(packet.budget.truncated);
        assert!(
            packet
                .budget
                .omitted_sections
                .contains(&"packet_payload".to_string())
        );
    }

    #[test]
    fn graph_budget_prunes_nodes_not_referenced_by_retained_edges() {
        fn node(id: &str) -> codestory_contracts::api::GraphNodeDto {
            codestory_contracts::api::GraphNodeDto {
                id: NodeId(id.to_string()),
                label: id.to_string(),
                kind: codestory_contracts::api::NodeKind::FUNCTION,
                depth: 1,
                label_policy: None,
                badge_visible_members: None,
                badge_total_members: None,
                merged_symbol_examples: Vec::new(),
                file_path: None,
                qualified_name: None,
                member_access: None,
            }
        }

        fn edge(id: &str, source: &str, target: &str) -> codestory_contracts::api::GraphEdgeDto {
            codestory_contracts::api::GraphEdgeDto {
                id: EdgeId(id.to_string()),
                source: NodeId(source.to_string()),
                target: NodeId(target.to_string()),
                kind: codestory_contracts::api::EdgeKind::CALL,
                confidence: None,
                certainty: None,
                callsite_identity: None,
                candidate_targets: Vec::new(),
            }
        }

        let mut answer = packet_answer_fixture(
            "Explain graph budget trimming.",
            vec![test_packet_citation("center", "src/center.rs", 0.9)],
        );
        answer.graphs.push(GraphArtifactDto::Uml {
            id: "graph".to_string(),
            title: "Graph".to_string(),
            graph: GraphResponse {
                center_id: NodeId("center".to_string()),
                nodes: vec![
                    node("center"),
                    node("kept"),
                    node("dropped_a"),
                    node("dropped_b"),
                ],
                edges: vec![
                    edge("edge_1", "center", "kept"),
                    edge("edge_2", "kept", "dropped_a"),
                    edge("edge_3", "dropped_a", "dropped_b"),
                ],
                truncated: false,
                omitted_edge_count: 0,
                canonical_layout: None,
            },
        });

        let budget = apply_packet_budget(
            "Explain graph budget trimming.",
            PacketBudgetModeDto::Tiny,
            PacketBudgetLimitsDto {
                max_trail_edges: 1,
                ..packet_budget_limits(PacketBudgetModeDto::Tiny)
            },
            &mut answer,
        );

        let GraphArtifactDto::Uml { graph, .. } = &answer.graphs[0] else {
            panic!("expected UML graph");
        };
        let node_ids = graph
            .nodes
            .iter()
            .map(|node| node.id.0.as_str())
            .collect::<Vec<_>>();
        assert_eq!(graph.edges.len(), 1);
        assert_eq!(node_ids, vec!["center", "kept"]);
        assert!(graph.truncated);
        assert!(budget.omitted_sections.contains(&"trail_edges".to_string()));
    }

    #[test]
    fn generic_packet_sections_and_sufficiency_cover_agent_stop_contract() {
        let question = "Explain how a command enters runtime orchestration, workspace planning, symbol extraction, persistence, and snapshot refresh.";
        let limits = packet_budget_limits(PacketBudgetModeDto::Compact);
        let mut answer = AgentAnswerDto {
            answer_id: "packet-fixture".to_string(),
            prompt: question.to_string(),
            summary: "Runtime flow is covered by cited anchors.".to_string(),
            freshness: None,
            sections: vec![AgentResponseSectionDto {
                id: "answer".to_string(),
                title: "Answer".to_string(),
                blocks: vec![AgentResponseBlockDto::Markdown {
                    markdown: "The flow starts at the command surface and proceeds through runtime, workspace, indexer, store, and snapshot layers.".to_string(),
                }],
            }],
            citations: vec![
                test_packet_citation(
                    "FlowRegression",
                    "tests/flow_regression.rs",
                    0.5,
                ),
                test_packet_citation("CliCommand", "crates/app-cli/src/main.rs", 0.2),
                test_packet_citation(
                    "RuntimeCoordinator",
                    "crates/app-runtime/src/services.rs",
                    0.3,
                ),
                test_packet_citation(
                    "WorkspacePlan",
                    "crates/workspace/src/plan.rs",
                    0.2,
                ),
                test_packet_citation(
                    "GraphIndexer",
                    "crates/indexer/src/lib.rs",
                    0.2,
                ),
                test_packet_citation(
                    "ProjectionStore",
                    "crates/store/src/projection.rs",
                    0.2,
                ),
            ],
            subgraph_ids: Vec::new(),
            retrieval_version: "test".to_string(),
            graphs: Vec::new(),
            retrieval_trace: codestory_contracts::api::AgentRetrievalTraceDto {
                request_id: "packet-fixture".to_string(),
                resolved_profile: AgentRetrievalPresetDto::Architecture,
                policy_mode: AgentRetrievalPolicyModeDto::LatencyFirst,
                total_latency_ms: 1,
                sla_target_ms: None,
                sla_missed: false,
                annotations: Vec::new(),
                steps: Vec::new(),
            },
        };

        rank_packet_evidence(question, &mut answer);
        append_packet_evidence_sections(
            &mut answer,
            PacketTaskClassDto::ArchitectureExplanation,
            &limits,
        );
        let budget =
            apply_packet_budget(question, PacketBudgetModeDto::Compact, limits, &mut answer);
        let sufficiency = build_packet_sufficiency(
            question,
            PacketTaskClassDto::ArchitectureExplanation,
            &answer,
            &budget,
        );

        assert_eq!(answer.sections[0].id, "packet-evidence-ledger");
        assert_eq!(answer.sections[1].id, "packet-flow-claims");
        let top_anchor_names = answer
            .citations
            .iter()
            .take(4)
            .map(|citation| citation.display_name.as_str())
            .collect::<Vec<_>>();
        assert!(
            top_anchor_names.contains(&"CliCommand"),
            "command entrypoint should stay in the high-priority flow anchors: {top_anchor_names:?}"
        );
        assert!(
            top_anchor_names.contains(&"RuntimeCoordinator"),
            "runtime coordination should stay in the high-priority flow anchors: {top_anchor_names:?}"
        );
        assert_eq!(sufficiency.status, PacketSufficiencyStatusDto::Sufficient);
        assert!(sufficiency.follow_up_commands.is_empty());
        assert!(sufficiency.open_next.is_empty());
        assert!(
            sufficiency.covered_claims.iter().any(|claim| claim
                .claim
                .contains("Runtime orchestration is anchored by `RuntimeCoordinator`")),
            "generic packet should include claim-led runtime flow notes: {sufficiency:?}"
        );
        assert!(
            sufficiency
                .avoid_opening
                .iter()
                .any(|path| path.contains("crates/app-cli/src/main.rs")),
            "sufficient packets should tell agents cited files do not need broad re-opening: {sufficiency:?}"
        );
    }

    #[test]
    fn packet_claims_use_normalized_evidence_paths() {
        let citation = AgentCitationDto {
            node_id: NodeId("CliCommand".to_string()),
            display_name: "CliCommand".to_string(),
            kind: codestory_contracts::api::NodeKind::FUNCTION,
            file_path: Some(
                "\\\\?\\C:\\workspaces\\sample\\crates\\tool-cli\\src\\main.rs".to_string(),
            ),
            line: Some(193),
            score: 0.85,
            origin: SearchHitOrigin::IndexedSymbol,
            resolvable: true,
            subgraph_id: None,
            evidence_edge_ids: Vec::new(),
            retrieval_score_breakdown: None,
        };

        assert_eq!(packet_evidence_role(&citation), Some("command entrypoint"));
        assert_eq!(
            packet_display_path(citation.file_path.as_deref().unwrap()),
            "crates/tool-cli/src/main.rs"
        );
        assert!(
            packet_claim_for_role("command entrypoint", &citation).contains("`CliCommand`"),
            "claim should name the evidence anchor"
        );
    }

    #[test]
    fn grounding_symbol_fallback_hit_is_anchor_ranked() {
        let hit =
            search_hit_from_grounding_symbol(&codestory_contracts::api::GroundingSymbolDigestDto {
                id: NodeId("abc".to_string()),
                node_ref: Some("src/main.rs:42:AppController".to_string()),
                label: "AppController @ src/main.rs".to_string(),
                kind: codestory_contracts::api::NodeKind::STRUCT,
                line: None,
                member_count: None,
                summary: None,
                edge_digest: Vec::new(),
            });

        assert_eq!(hit.display_name, "AppController");
        assert_eq!(hit.file_path.as_deref(), Some("src/main.rs"));
        assert_eq!(hit.line, Some(42));
        assert!(!weak_initial_hits(
            "How does this repo fit together?",
            &[hit]
        ));
    }

    #[test]
    fn bounded_markdown_snippet_keeps_suffix_inside_byte_cap() {
        let source = (0..200)
            .map(|line| format!("let value_{line} = \"large source context\";\n"))
            .collect::<String>();

        let snippet = bounded_markdown_snippet(&source, Some(90), 90, 96);

        assert!(snippet.truncated);
        assert!(snippet.markdown.len() <= 96);
        assert!(snippet.markdown.contains("truncated"));
    }

    #[test]
    fn mermaid_builder_guarantees_fallback_diagram() {
        let mut trace = TraceRecorder::new(Some(DEFAULT_SLA_TARGET_MS));
        let bundle = RetrievalBundle::default();
        let artifacts = build_mermaid_artifacts(
            &latency_profile(),
            &AgentAskRequest {
                prompt: "inspect this".to_string(),
                retrieval_profile:
                    codestory_contracts::api::AgentRetrievalProfileSelectionDto::Auto,
                focus_node_id: None,
                max_results: None,
                response_mode: AgentResponseModeDto::Markdown,
                latency_budget_ms: None,
                include_evidence: true,
                hybrid_weights: None,
            },
            "inspect this",
            Instant::now(),
            &bundle,
            &mut trace,
        );

        assert_eq!(artifacts.len(), 1);
        assert!(matches!(artifacts[0], GraphArtifactDto::Mermaid { .. }));
    }

    #[test]
    fn source_context_keyword_gate_detects_code_requests() {
        assert!(needs_source_context(
            "show me the implementation and snippet"
        ));
        assert!(!needs_source_context(
            "summarize architecture at a high level"
        ));
    }

    #[test]
    fn prompt_search_terms_extracts_core_keywords() {
        let terms = prompt_search_terms("How does the language parsing work in this repo?");
        assert_eq!(terms, vec!["language".to_string(), "parsing".to_string()]);
    }

    #[test]
    fn merge_search_hits_deduplicates_and_keeps_best_score() {
        let mut into = vec![SearchHit {
            node_id: codestory_contracts::api::NodeId("1".to_string()),
            display_name: "Parser".to_string(),
            kind: codestory_contracts::api::NodeKind::FUNCTION,
            file_path: None,
            line: None,
            score: 10.0,
            origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
            match_quality: None,
            resolvable: true,
            score_breakdown: None,
        }];

        merge_search_hits(
            &mut into,
            vec![
                SearchHit {
                    node_id: codestory_contracts::api::NodeId("1".to_string()),
                    display_name: "Parser".to_string(),
                    kind: codestory_contracts::api::NodeKind::FUNCTION,
                    file_path: None,
                    line: None,
                    score: 42.0,
                    origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
                    match_quality: None,
                    resolvable: true,
                    score_breakdown: None,
                },
                SearchHit {
                    node_id: codestory_contracts::api::NodeId("2".to_string()),
                    display_name: "LanguageParser".to_string(),
                    kind: codestory_contracts::api::NodeKind::MODULE,
                    file_path: None,
                    line: None,
                    score: 18.0,
                    origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
                    match_quality: None,
                    resolvable: true,
                    score_breakdown: None,
                },
            ],
            10,
        );

        assert_eq!(into.len(), 2);
        assert_eq!(into[0].node_id.0, "1");
        assert_eq!(into[0].score, 42.0);
    }

    #[test]
    fn evidence_edge_ids_are_sorted_and_filtered() {
        let graph = GraphResponse {
            center_id: codestory_contracts::api::NodeId("1".to_string()),
            nodes: Vec::new(),
            edges: vec![
                codestory_contracts::api::GraphEdgeDto {
                    id: EdgeId("8".to_string()),
                    source: codestory_contracts::api::NodeId("2".to_string()),
                    target: codestory_contracts::api::NodeId("3".to_string()),
                    kind: codestory_contracts::api::EdgeKind::CALL,
                    confidence: None,
                    certainty: None,
                    callsite_identity: None,
                    candidate_targets: Vec::new(),
                },
                codestory_contracts::api::GraphEdgeDto {
                    id: EdgeId("3".to_string()),
                    source: codestory_contracts::api::NodeId("4".to_string()),
                    target: codestory_contracts::api::NodeId("2".to_string()),
                    kind: codestory_contracts::api::EdgeKind::CALL,
                    confidence: None,
                    certainty: None,
                    callsite_identity: None,
                    candidate_targets: Vec::new(),
                },
                codestory_contracts::api::GraphEdgeDto {
                    id: EdgeId("9".to_string()),
                    source: codestory_contracts::api::NodeId("7".to_string()),
                    target: codestory_contracts::api::NodeId("8".to_string()),
                    kind: codestory_contracts::api::EdgeKind::CALL,
                    confidence: None,
                    certainty: None,
                    callsite_identity: None,
                    candidate_targets: Vec::new(),
                },
            ],
            truncated: false,
            canonical_layout: None,
            omitted_edge_count: 0,
        };

        let evidence = evidence_edge_ids_for_node(
            Some(&graph),
            &codestory_contracts::api::NodeId("2".to_string()),
        );
        let ids = evidence.into_iter().map(|id| id.0).collect::<Vec<_>>();
        assert_eq!(ids, vec!["3".to_string(), "8".to_string()]);
    }
}
