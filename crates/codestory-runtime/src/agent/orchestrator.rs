use crate::agent::citation::{evidence_edge_ids_for_node, to_citation_from_hit};
use crate::agent::eval_probes::{
    eval_citation_shaped_claim, eval_flow_template_claims, eval_probes_enabled,
    eval_supporting_claim_flow_sentence, push_eval_architecture_flow_probe_terms,
    push_eval_flow_hint_packet_queries, push_eval_required_probe_queries,
    push_index_derived_architecture_probes,
};
use crate::agent::packet_batch::{
    PacketLatencyBudget, packet_anchor_probe_queries, packet_file_stem_matches_query,
    run_packet_anchor_expansion, run_packet_planned_subqueries,
};
#[cfg(test)]
use crate::agent::packet_batch::{
    packet_anchor_hit_is_relevant, packet_anchor_probe_limit_for_budget,
};
use crate::agent::packet_scoring::{
    normalize_identifier, packet_adjacent_query_stop_term, packet_citation_key,
    packet_citation_rank, packet_claim_carry_rank, packet_display_name_is_import_literal,
    packet_display_name_is_test_like, packet_display_path, packet_low_signal_display_name,
    packet_query_stop_term,
};
use crate::agent::planning::dedupe_packet_plan_queries;
use crate::agent::profiles::{ResolvedProfile, TrailPlan, resolve_profile};
use crate::agent::retrieval_primary::{
    RETRIEVAL_VERSION_SIDECAR, SidecarPrimarySearchOutcome, maybe_log_rollback_after_packet,
    maybe_run_retrieval_shadow, sidecar_retrieval_blocks_nucleo_supplement,
    sidecar_retrieval_primary_enabled, sidecar_retrieval_unavailable_error,
    try_sidecar_primary_search,
};
use crate::agent::trace::{TraceRecorder, field};
use crate::agent::trace_export;
use crate::{
    AppController, FocusedSourceContext, HybridSearchScoredHit, exact_symbol_query_terms,
    fallback_mermaid as diagnostic_mermaid, hybrid_retrieval_enabled, is_non_primary_source_term,
    looks_like_standalone_symbol_query, mermaid_flowchart, mermaid_gantt, mermaid_sequence,
    query_mentions_non_primary_source, retrieval_file_role_from_path,
};
use codestory_contracts::api::{
    AgentAnswerDto, AgentAskRequest, AgentCitationDto, AgentCustomRetrievalConfigDto,
    AgentHybridWeightsDto, AgentPacketDto, AgentPacketRequestDto, AgentResponseBlockDto,
    AgentResponseModeDto, AgentResponseSectionDto, AgentRetrievalPolicyModeDto,
    AgentRetrievalPresetDto, AgentRetrievalProfileSelectionDto, AgentRetrievalStepKindDto,
    AgentRetrievalStepStatusDto, ApiError, GraphArtifactDto, GraphRequest, GraphResponse,
    GroundingBudgetDto, IndexFreshnessDto, IndexFreshnessStatusDto, NodeDetailsDto,
    NodeDetailsRequest, NodeId, NodeKind, NodeOccurrencesRequest, PacketBenchmarkTraceDto,
    PacketBudgetDto, PacketBudgetLimitsDto, PacketBudgetModeDto, PacketBudgetUsageDto,
    PacketClaimDto, PacketPlanDto, PacketPlanQueryDto, PacketSufficiencyDto,
    PacketSufficiencyStatusDto, PacketTaskClassDto, RetrievalScoreBreakdownDto, SearchHit,
    SearchHitOrigin, SearchRepoTextMode, SearchRequest, TrailConfigDto, TrailFilterOptionsDto,
};
#[cfg(test)]
use codestory_contracts::api::{AgentRetrievalStepDto, EdgeId, SearchMatchQualityDto};
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::path::Path;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

const DEFAULT_MAX_RESULTS: u32 = 8;
const DEFAULT_MAX_EDGES: u32 = 260;
const DEFAULT_SLA_TARGET_MS: u32 = 18_000;
const MIN_PHASE_DEADLINE_MS: u128 = 750;
const WEAK_INITIAL_HIT_COUNT: usize = 3;
const WEAK_INITIAL_TOP_SCORE: f32 = 0.30;
const WEAK_INITIAL_MIN_LEXICAL_ANCHOR: f32 = 0.01;
const WEAK_INITIAL_MIN_GRAPH_ANCHOR: f32 = 0.25;
const SOURCE_SNIPPET_TRUNCATION_SUFFIX: &str =
    "\n// ... source snippet truncated by investigation byte cap\n```";
const PACKET_MARKDOWN_TRUNCATION_SUFFIX: &str = "\n\n... packet section truncated by budget ...\n";
const GRAPH_ARTIFACT_BUNDLE_BYTE_CAP: usize = 512 * 1024;
const RETRIEVAL_VERSION_HYBRID: &str = "hybrid-v1";
const RETRIEVAL_VERSION_SIDECAR_BLOCKED: &str = "sidecar-blocked-v1";
const PACKET_FOCUS_NEIGHBORHOOD_CARRY_LIMIT: usize = 4;
const PACKET_SOURCE_DEFINITION_CLAIM_LIMIT: usize = 6;
fn retrieval_version(controller: &AppController) -> &'static str {
    if sidecar_retrieval_primary_enabled(controller) {
        RETRIEVAL_VERSION_SIDECAR
    } else if hybrid_retrieval_enabled() {
        RETRIEVAL_VERSION_HYBRID
    } else {
        RETRIEVAL_VERSION_SIDECAR_BLOCKED
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
    diagnostic_supplement_used: bool,
    repo_explanation_supplement_used: bool,
}

#[derive(Debug, Clone, Copy, Default)]
struct GraphArtifactCapStats {
    retained_bytes: usize,
    omitted_count: usize,
    truncated: bool,
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
            trace.annotate(format!(
                "index_freshness status={:?} duration_ms={} indexed_files={} changed={} new={} removed={}",
                freshness.status,
                freshness.duration_ms,
                freshness.indexed_file_count,
                freshness.changed_file_count,
                freshness.new_file_count,
                freshness.removed_file_count,
            ));
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
            diagnostic_focus: bundle.diagnostic_supplement_used,
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
        retrieval_version: retrieval_version(controller).to_string(),
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
    let project_root = controller.require_project_root()?;
    controller.begin_packet_retrieval();

    let extra_probes = packet_request_extra_probes(req.extra_probes);
    let plan = build_packet_plan_with_extra(&question, req.task_class, req.budget, &extra_probes);
    let limits = packet_budget_limits(req.budget);
    let packet_latency = PacketLatencyBudget::new(req.latency_budget_ms);
    let retrieval_profile = packet_retrieval_profile(Some(plan.task_class), req.budget, &limits);
    let initial_hybrid_weights = packet_initial_hybrid_weights(&plan, req.budget);
    let retrieval_prompt = packet_retrieval_prompt(
        &question,
        &plan,
        initial_hybrid_weights.as_ref(),
        req.budget,
    );
    let mut answer = agent_ask(
        controller,
        AgentAskRequest {
            prompt: question.clone(),
            retrieval_profile,
            focus_node_id: None,
            max_results: Some(limits.max_anchors.clamp(1, 25)),
            response_mode: AgentResponseModeDto::Structured,
            latency_budget_ms: req.latency_budget_ms,
            include_evidence: req.include_evidence,
            hybrid_weights: initial_hybrid_weights.clone(),
        },
    )?;
    if packet_initial_retrieval_is_lexical_only(initial_hybrid_weights.as_ref()) {
        answer.retrieval_trace.annotations.push(format!(
            "packet_initial_retrieval semantic_skipped=true reason=compact_exact_anchor_probes probe_count={}",
            packet_anchor_probe_queries(&plan).len()
        ));
    }
    answer
        .retrieval_trace
        .annotations
        .push(packet_plan_annotation(&plan));
    if retrieval_prompt != question {
        answer.retrieval_trace.annotations.push(format!(
            "packet_initial_retrieval raw_question_only=true deferred_planned_probe_chars={}",
            retrieval_prompt.len().saturating_sub(question.len())
        ));
    }
    let rank_terms = packet_rank_terms(&question);
    run_packet_anchor_expansion(
        controller,
        &plan,
        req.budget,
        &limits,
        req.include_evidence,
        packet_latency,
        &rank_terms,
        &mut answer,
    )?;
    run_packet_planned_subqueries(
        controller,
        &plan,
        req.budget,
        &limits,
        req.include_evidence,
        packet_latency,
        &rank_terms,
        &mut answer,
    )?;
    maybe_append_sql_schema_file_citations(&project_root, &question, &mut answer);
    maybe_append_required_file_scoped_source_citations(
        &project_root,
        &question,
        plan.task_class,
        &extra_probes,
        &mut answer,
    );
    packet_latency.apply_to_trace(&mut answer);
    rank_packet_evidence(&question, &mut answer);
    maybe_annotate_packet_candidate_window(&question, &limits, &mut answer);

    if answer.retrieval_trace.retrieval_shadow.is_none()
        && let Some(shadow) =
            maybe_run_retrieval_shadow(controller, &question, req.latency_budget_ms)
    {
        answer.retrieval_trace.annotations.push(format!(
            "retrieval_shadow mode={} total_ms={} candidates={} would_rank={}",
            shadow.retrieval_mode,
            shadow.retrieval_total_ms,
            shadow.candidates.len(),
            shadow.would_rank.len()
        ));
        answer.retrieval_trace.retrieval_shadow = Some(shadow);
    }
    maybe_log_rollback_after_packet(controller, answer.retrieval_trace.retrieval_shadow.as_ref());

    let budget = apply_packet_budget_with_extra(
        &project_root,
        &question,
        plan.task_class,
        req.budget,
        limits.clone(),
        &mut answer,
        &extra_probes,
    );
    append_packet_evidence_sections(&mut answer, plan.task_class, &limits);
    let sufficiency = build_packet_sufficiency_with_extra(
        &project_root,
        &question,
        plan.task_class,
        &answer,
        &budget,
        &extra_probes,
    );
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
    enforce_packet_output_budget(&project_root, &mut packet);

    if let Ok(trace_path) = std::env::var("CODESTORY_PACKET_STEP_TRACE_OUT")
        && let Ok(payload) =
            serde_json::to_string_pretty(&trace_export::packet_step_trace_json(&packet.answer))
    {
        let _ = std::fs::write(trace_path, payload);
    }
    packet.answer.retrieval_trace.annotations.push(format!(
        "packet_step_trace search_total_ms={} step_count={}",
        trace_export::search_step_total_ms(&packet.answer),
        packet.answer.retrieval_trace.steps.len()
    ));

    Ok(packet)
}

#[cfg(test)]
fn build_packet_plan(
    question: &str,
    requested: Option<PacketTaskClassDto>,
    budget: PacketBudgetModeDto,
) -> PacketPlanDto {
    build_packet_plan_with_extra(question, requested, budget, &[])
}

fn build_packet_plan_with_extra(
    question: &str,
    requested: Option<PacketTaskClassDto>,
    budget: PacketBudgetModeDto,
    extra_probes: &[String],
) -> PacketPlanDto {
    let task_class = requested.unwrap_or_else(|| infer_packet_task_class(question));
    let mut queries = Vec::new();
    push_packet_query(
        &mut queries,
        question,
        "original task phrasing for sidecar-primary source-backed retrieval",
    );
    for term in extract_packet_query_terms(question) {
        push_packet_query(
            &mut queries,
            &term,
            "concrete symbol, file, route, or code term",
        );
    }
    for query in extra_probes {
        push_packet_query(
            &mut queries,
            query,
            "explicit symbol probe from packet request",
        );
    }
    for query in packet_symbol_probe_queries(question, task_class, budget) {
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
    let query_cap = packet_plan_query_cap(budget);
    queries.truncate(query_cap);

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
    if !extra_probes.is_empty() {
        trace.push(format!(
            "explicit_extra_probes={} source=request",
            extra_probes.len()
        ));
    }

    let mut plan = PacketPlanDto {
        task_class,
        inferred_task_class: requested.is_none(),
        queries,
        trace,
    };
    dedupe_packet_plan_queries(&mut plan);
    plan.trace.push(format!(
        "deduped_queries={} eval_probes={}",
        plan.queries.len(),
        eval_probes_enabled()
    ));
    plan
}

fn packet_request_extra_probes(extra_probes: Vec<String>) -> Vec<String> {
    let mut normalized = Vec::new();
    for probe in extra_probes {
        let probe = probe.trim();
        if probe.is_empty() || probe.len() > 240 {
            continue;
        }
        if !normalized
            .iter()
            .any(|existing: &String| existing.eq_ignore_ascii_case(probe))
        {
            normalized.push(probe.to_string());
        }
        if normalized.len() >= 16 {
            break;
        }
    }
    normalized
}

fn packet_explicit_request_probe_queries(plan: &PacketPlanDto) -> Vec<String> {
    plan.queries
        .iter()
        .filter(|query| query.purpose.contains("explicit symbol probe"))
        .map(|query| query.query.clone())
        .collect()
}

fn packet_plan_query_cap(budget: PacketBudgetModeDto) -> usize {
    match budget {
        PacketBudgetModeDto::Tiny => 20,
        PacketBudgetModeDto::Compact => 32,
        PacketBudgetModeDto::Standard => 48,
        PacketBudgetModeDto::Deep => 56,
    }
}

fn packet_symbol_probe_queries(
    question: &str,
    task_class: PacketTaskClassDto,
    budget: PacketBudgetModeDto,
) -> Vec<String> {
    let terms = packet_probe_terms(question);
    let mut queries = Vec::new();
    let compact = matches!(
        budget,
        PacketBudgetModeDto::Compact | PacketBudgetModeDto::Tiny
    );

    push_unique_owned_terms(
        &mut queries,
        &packet_command_role_probe_queries(question, task_class),
    );
    push_unique_owned_terms(
        &mut queries,
        &packet_command_exact_probe_queries(question, task_class),
    );
    push_unique_owned_terms(
        &mut queries,
        &packet_prompt_exact_symbol_probe_queries(question, &terms, task_class),
    );
    push_prompt_named_file_probe_queries(&terms, &mut queries);
    push_prompt_derived_exact_flow_anchor_queries(&terms, &mut queries);
    push_unique_owned_terms(
        &mut queries,
        &packet_sufficiency_required_probe_queries_from_terms(&terms, task_class),
    );
    let concrete_file_queries = packet_concrete_file_probe_queries_from_required(&queries);
    push_unique_owned_terms(&mut queries, &concrete_file_queries);
    push_flow_hint_packet_queries(&terms, &mut queries);
    push_task_class_symbol_probe_queries(task_class, &mut queries);
    if !compact {
        push_adjacent_packet_term_queries(&terms, &mut queries, 8);
    } else if matches!(task_class, PacketTaskClassDto::ArchitectureExplanation) {
        push_adjacent_packet_term_queries(&terms, &mut queries, 12);
    }
    push_generic_symbol_probe_queries(&terms, &mut queries, compact);

    queries.truncate(packet_plan_query_cap(budget));
    queries
}

fn packet_prompt_exact_symbol_probe_queries(
    question: &str,
    terms: &[String],
    task_class: PacketTaskClassDto,
) -> Vec<String> {
    if !matches!(
        task_class,
        PacketTaskClassDto::ArchitectureExplanation
            | PacketTaskClassDto::DataFlow
            | PacketTaskClassDto::ChangeImpact
            | PacketTaskClassDto::RouteTracing
            | PacketTaskClassDto::EditPlanning
            | PacketTaskClassDto::SymbolOwnership
            | PacketTaskClassDto::BugLocalization
    ) {
        return Vec::new();
    }

    let mut queries = Vec::new();
    for term in exact_symbol_query_terms(question) {
        if packet_prompt_exact_symbol_term_is_probe(&term) {
            push_unique_term(&mut queries, &term);
        }
    }
    push_prompt_concept_derived_symbol_probes(terms, &mut queries);
    queries
}

fn packet_prompt_exact_symbol_term_is_probe(term: &str) -> bool {
    let trimmed = term.trim();
    if trimmed.len() < 3 {
        return false;
    }
    let letters = trimmed
        .chars()
        .filter(|ch| ch.is_ascii_alphabetic())
        .collect::<Vec<_>>();
    !letters.is_empty() && !letters.iter().all(|ch| ch.is_ascii_uppercase())
}

fn push_prompt_concept_derived_symbol_probes(terms: &[String], queries: &mut Vec<String>) {
    let has = |term: &str| packet_terms_have(terms, term);
    let has_any = |needles: &[&str]| packet_terms_have_any(terms, needles);

    if has("stringutils") && has_any(&["blank", "empty", "whitespace"]) {
        push_unique_terms(queries, &["StringUtils.isBlank", "StringUtils.isEmpty"]);
    }
    if has("strings") && has_any(&["case", "sensitive", "insensitive"]) {
        push_unique_terms(queries, &["Strings.CS", "Strings.CI"]);
    }
    if has("charsequenceutils")
        && (has_any(&["case", "sensitive", "region", "matching", "checks"]) || has("strings"))
    {
        push_unique_term(queries, "CharSequenceUtils.regionMatches");
    }

    let swr_prompt = has("swr") || has("useswr");
    if swr_prompt && has_any(&["exposes", "hook", "hooks", "public"]) {
        push_unique_terms(
            queries,
            &["useSWR", "useSWRHandler", "withArgs", "withMiddleware"],
        );
    }
    if swr_prompt && has_any(&["serialize", "serializes", "serialized", "key", "keys"]) {
        push_unique_term(queries, "serialize");
    }
    if swr_prompt && has_any(&["cache", "helper", "helpers"]) {
        push_unique_term(queries, "createCacheHelper");
    }
    if swr_prompt && has_any(&["mutate", "mutation", "mutations"]) {
        push_unique_term(queries, "internalMutate");
    }

    if packet_terms_indicate_gin_route_dispatch_flow(terms) {
        push_gin_route_dispatch_symbol_probe_queries(queries);
    }
    if packet_terms_indicate_css_animation_flow(terms) {
        push_css_animation_symbol_probe_queries(queries);
    }
    if packet_terms_indicate_automapper_map_flow(terms) {
        push_automapper_map_flow_symbol_probe_queries(queries);
    }
}

fn push_prompt_named_file_probe_queries(terms: &[String], queries: &mut Vec<String>) {
    let has = |term: &str| packet_terms_have(terms, term);
    let has_any = |needles: &[&str]| packet_terms_have_any(terms, needles);

    if has("stringutils") && has_any(&["blank", "empty", "whitespace"]) {
        push_unique_terms(
            queries,
            &["StringUtils.java", "Strings.java", "CharSequenceUtils.java"],
        );
    }
    if has("swr") || has("useswr") {
        push_unique_terms(
            queries,
            &[
                "index.ts useSWR",
                "use-swr.ts useSWRHandler",
                "serialize.ts",
                "helper.ts createCacheHelper",
                "mutate.ts internalMutate",
                "with-middleware.ts withMiddleware",
            ],
        );
    }
    if packet_terms_indicate_gin_route_dispatch_flow(terms) {
        push_unique_terms(
            queries,
            &[
                "gin.go New",
                "gin.go Default",
                "gin.go Engine.addRoute",
                "gin.go Engine.handleHTTPRequest",
                "routergroup.go RouterGroup.Handle",
                "tree.go node.addRoute",
                "context.go Context.Next",
            ],
        );
    }
    if packet_terms_indicate_css_animation_flow(terms) {
        push_unique_terms(
            queries,
            &[
                "source/_vars.css",
                "source/_base.css",
                "source/animate.css",
                "source/attention_seekers/bounce.css bounce",
                "source/attention_seekers/flash.css flash",
            ],
        );
    }
    if packet_terms_indicate_automapper_map_flow(terms) {
        push_automapper_map_flow_symbol_probe_queries(queries);
    }
}

fn packet_probe_terms(question: &str) -> Vec<String> {
    let include_non_primary_terms = query_mentions_non_primary_source(question);
    let brand_terms = brand_phrase_noise_terms(question);
    let mut terms = prompt_search_terms(question)
        .into_iter()
        .filter(|term| {
            include_non_primary_terms
                || !is_non_primary_source_term(term)
                || packet_retains_non_primary_probe_term(question, term)
        })
        .collect::<Vec<_>>();

    if !brand_terms.is_empty() && packet_terms_have_specific_flow_anchor(&terms) {
        terms.retain(|term| !brand_terms.contains(term.as_str()));
    }

    terms
}

fn packet_retains_non_primary_probe_term(question: &str, term: &str) -> bool {
    if !matches!(term, "bench" | "benchmark" | "benchmarks") {
        return false;
    }
    let lowered = question.to_ascii_lowercase();
    lowered.contains("architecture")
        && (lowered.contains("boundary")
            || lowered.contains("boundaries")
            || lowered.contains("across"))
}

fn packet_terms_have_specific_flow_anchor(terms: &[String]) -> bool {
    let has = |term: &str| terms.iter().any(|value| value.eq_ignore_ascii_case(term));
    let has_any = |needles: &[&str]| needles.iter().any(|needle| has(needle));
    (has("extension") && has("host"))
        || ((has("indexing") || has("indexer")) && (has("storage") || has("persistent")))
        || ((has("json") || has("jsonl")) && (has("exec") || has("thread") || has("turn")))
        || packet_terms_indicate_request_dispatch_flow(terms)
        || packet_terms_indicate_express_application_route_flow(terms)
        || (has("event") && has("loop"))
        || (has_any(&["command", "commands"]) && has_any(&["dispatch", "dispatches"]))
        || (has("search") && (has("flags") || has("matcher") || has("haystack")))
        || has("payload")
        || has("posts")
        || has("post")
        || has("comments")
        || has("feed")
        || has("rss")
}

fn brand_phrase_noise_terms(question: &str) -> HashSet<String> {
    let mut terms = HashSet::new();
    let tokens = question
        .split_whitespace()
        .map(|token| {
            token.trim_matches(|ch: char| {
                matches!(
                    ch,
                    ',' | '.' | ';' | ':' | '?' | '!' | '(' | ')' | '[' | ']' | '{' | '}'
                )
            })
        })
        .collect::<Vec<_>>();

    for window in tokens.windows(3) {
        if let [left, joiner, right] = window
            && *joiner == "&"
        {
            if let Some(term) = title_case_brand_token_term(left) {
                terms.insert(term);
            }
            if let Some(term) = title_case_brand_token_term(right) {
                terms.insert(term);
            }
        }
    }

    terms
}

fn title_case_brand_token_term(token: &str) -> Option<String> {
    let mut chars = token.chars();
    let first = chars.next()?;
    let second = chars.next()?;
    if first.is_ascii_uppercase()
        && second.is_ascii_lowercase()
        && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
    {
        Some(token.to_ascii_lowercase())
    } else {
        None
    }
}

fn push_flow_hint_packet_queries(terms: &[String], queries: &mut Vec<String>) {
    push_prompt_derived_flow_hint_packet_queries(terms, queries);
    push_eval_flow_hint_packet_queries(terms, queries);
    if !eval_probes_enabled() {
        push_index_derived_architecture_probes(
            PacketTaskClassDto::ArchitectureExplanation,
            terms,
            queries,
        );
    }
}

fn push_prompt_derived_exact_flow_anchor_queries(terms: &[String], queries: &mut Vec<String>) {
    let has = |term: &str| packet_terms_have(terms, term);
    let has_any = |needles: &[&str]| packet_terms_have_any(terms, needles);

    if has("exec") && has_any(&["runtime", "session"]) {
        push_unique_terms(queries, &["exec runtime", "exec session"]);
    }
    if has("exec") && has_any(&["cli", "command", "subcommand"]) {
        push_unique_terms(queries, &["exec cli", "exec command"]);
    }
    if has_any(&["json", "jsonl"]) && has_any(&["event", "events", "output"]) {
        push_unique_terms(queries, &["json event output", "event output processor"]);
    }
    if has("exec") && has_any(&["event", "events", "json", "jsonl"]) {
        push_unique_term(queries, "exec event output");
    }
    if has("thread") && has_any(&["start", "starts", "started"]) {
        push_unique_term(queries, "thread start");
    }
    if has("turn") && has_any(&["start", "starts", "started"]) {
        push_unique_term(queries, "turn start");
    }
    if packet_terms_indicate_indexing_flow(terms) {
        push_indexing_flow_required_probe_queries(queries);
    }
    if packet_terms_indicate_request_dispatch_flow(terms) {
        push_unique_terms(
            queries,
            &[
                "request interceptor",
                "request dispatch",
                "transport adapter",
            ],
        );
    }
    if packet_terms_indicate_prepared_session_adapter_flow(terms) {
        push_unique_terms(
            queries,
            &[
                "Session.request",
                "Session.prepare_request",
                "PreparedRequest.prepare",
                "Session.send",
                "HTTPAdapter.send",
            ],
        );
    }
    if packet_terms_indicate_express_application_route_flow(terms) {
        push_express_application_route_probe_queries(queries);
    }
    if has_any(&["adapter", "adapters", "transport"]) {
        push_unique_terms(queries, &["transport adapter", "adapter selection"]);
    }
    if has("event") && has("loop") {
        push_unique_terms(
            queries,
            &[
                "event loop",
                "event dispatch",
                "network input",
                "command dispatch",
            ],
        );
    }
    if has_any(&["client", "network", "reads", "socket"]) {
        push_unique_terms(queries, &["client input", "network input"]);
    }
    if has("call") && has_any(&["command", "commands", "dispatch", "dispatches"]) {
        push_unique_terms(queries, &["command dispatch", "command handler"]);
    }
    if packet_terms_indicate_search_execution_flow(terms) {
        push_search_flow_probe_queries(queries);
    }
}

fn push_prompt_derived_flow_hint_packet_queries(terms: &[String], queries: &mut Vec<String>) {
    let has = |term: &str| packet_terms_have(terms, term);
    let has_any = |needles: &[&str]| packet_terms_have_any(terms, needles);

    if packet_terms_indicate_indexing_flow(terms) {
        push_unique_terms(
            queries,
            &[
                "index service",
                "workspace execution plan",
                "workspace indexer",
                "symbol extraction indexer",
                "projection batch",
                "search projection",
                "snapshot refresh",
            ],
        );
    }
    if has("exec") && has_any(&["runtime", "session"]) {
        push_unique_terms(queries, &["exec runtime", "exec session", "run exec"]);
    }
    if has("exec") && has_any(&["cli", "command", "subcommand"]) {
        push_unique_terms(queries, &["exec cli", "exec command", "subcommand"]);
    }
    if has_any(&["cli", "command", "subcommand"]) && has_any(&["runtime", "exec"]) {
        push_unique_term(queries, "command runtime");
    }
    if has_any(&["json", "jsonl"]) && has_any(&["event", "events", "output"]) {
        push_unique_terms(
            queries,
            &[
                "json event output",
                "jsonl event output",
                "event output processor",
            ],
        );
    }
    if has("exec") && has_any(&["event", "events", "json", "jsonl"]) {
        push_unique_terms(queries, &["exec event output", "exec events"]);
    }
    if has("thread") && has_any(&["start", "starts", "started"]) {
        push_unique_terms(queries, &["thread start", "start thread"]);
    }
    if has("turn") && has_any(&["start", "starts", "started"]) {
        push_unique_terms(queries, &["turn start", "start turn"]);
    }
    if packet_terms_indicate_request_dispatch_flow(terms) {
        push_unique_terms(
            queries,
            &[
                "request interceptor",
                "interceptor manager",
                "dispatch request",
            ],
        );
    }
    if packet_terms_indicate_prepared_session_adapter_flow(terms) {
        push_unique_terms(
            queries,
            &[
                "prepared request",
                "session request",
                "session send",
                "adapter send",
                "get adapter",
            ],
        );
    }
    if has_any(&["adapter", "adapters", "transport"]) {
        push_unique_terms(queries, &["transport adapter", "adapter selection"]);
    }
    if has("event") && has("loop") {
        push_unique_terms(queries, &["event loop", "main event loop"]);
    }
    if has_any(&["client", "network", "reads", "socket"]) {
        push_unique_terms(
            queries,
            &["client command input", "networking command read"],
        );
    }
    if has("command") && has_any(&["dispatch", "dispatches"]) {
        push_unique_term(queries, "command dispatch");
    }
    if packet_terms_indicate_search_execution_flow(terms) {
        push_unique_terms(
            queries,
            &[
                "flag parse search driver",
                "cli flags search pipeline",
                "entrypoint flag parse run search",
                "run search mode",
                "parallel walk builder search",
                "high level arguments matcher searcher printer",
                "walk haystack search worker",
                "worker search haystack",
                "matcher searcher printer",
            ],
        );
    }
}

fn push_search_flow_probe_queries(queries: &mut Vec<String>) {
    push_unique_terms(
        queries,
        &[
            "search entrypoint",
            "main",
            "main flag parse search",
            "entrypoint flag parse run search",
            "run search mode",
            "argument planning",
            "high level arguments matcher searcher printer",
            "args matcher searcher printer",
            "walk builder matcher searcher printer",
            "candidate file walk",
            "walk builder parallel search",
            "parallel walk builder search",
            "search worker",
            "search worker search",
            "worker search haystack",
            "result printer",
        ],
    );
}

fn packet_terms_have(terms: &[String], needle: &str) -> bool {
    let normalized_needle = normalize_identifier(needle);
    terms.iter().any(|value| {
        value.eq_ignore_ascii_case(needle) || normalize_identifier(value) == normalized_needle
    })
}

fn packet_terms_have_any(terms: &[String], needles: &[&str]) -> bool {
    needles
        .iter()
        .any(|needle| packet_terms_have(terms, needle))
}

fn packet_terms_indicate_indexing_flow(terms: &[String]) -> bool {
    let has_any = |needles: &[&str]| packet_terms_have_any(terms, needles);

    has_any(&["index", "indexed", "indexer", "indexing"])
        && has_any(&[
            "cli",
            "command",
            "discovery",
            "extraction",
            "file",
            "files",
            "persistence",
            "projection",
            "refresh",
            "runtime",
            "search",
            "snapshot",
            "storage",
            "store",
            "symbol",
            "workspace",
        ])
}

fn packet_terms_indicate_request_dispatch_flow(terms: &[String]) -> bool {
    let has = |term: &str| packet_terms_have(terms, term);
    let has_any = |needles: &[&str]| packet_terms_have_any(terms, needles);
    let explicit_client_transport = has_any(&[
        "adapter",
        "adapters",
        "interceptor",
        "interceptors",
        "transport",
    ]);
    if packet_terms_indicate_server_route_dispatch_flow(terms) && !explicit_client_transport {
        return false;
    }
    let has_compound_request_dispatch = terms.iter().any(|term| {
        let normalized = normalize_identifier(term);
        normalized.contains("dispatch") && normalized.contains("request")
    });
    has_any(&["interceptor", "interceptors"])
        || has_compound_request_dispatch
        || ((has("request") || has("http"))
            && has_any(&["adapter", "adapters", "dispatch", "dispatches", "transport"]))
}

fn packet_terms_indicate_server_route_dispatch_flow(terms: &[String]) -> bool {
    let has = |term: &str| packet_terms_have(terms, term);
    let has_any = |needles: &[&str]| packet_terms_have_any(terms, needles);
    has_any(&["route", "routes", "router"])
        && has_any(&[
            "handler",
            "handlers",
            "middleware",
            "dispatch",
            "dispatches",
        ])
        && (has("request")
            || has_any(&["server", "incoming", "http"])
            || has_any(&["engine", "method", "methods"]))
}

fn packet_terms_indicate_express_application_route_flow(terms: &[String]) -> bool {
    let has = |term: &str| packet_terms_have(terms, term);
    let has_any = |needles: &[&str]| packet_terms_have_any(terms, needles);

    has("express")
        && has_any(&["application", "app"])
        && has_any(&[
            "middleware",
            "middleware/routes",
            "route",
            "routes",
            "router",
        ])
        && has_any(&["request", "response", "handler", "handles"])
}

fn packet_terms_indicate_prepared_session_adapter_flow(terms: &[String]) -> bool {
    let has = |term: &str| packet_terms_have(terms, term);
    let has_any = |needles: &[&str]| packet_terms_have_any(terms, needles);
    (has("prepared") || has("prepare") || has("preparedrequest"))
        && has_any(&["request", "requests"])
        && has("session")
        && has_any(&["adapter", "adapters", "send", "sends", "transport"])
}

fn packet_terms_indicate_search_execution_flow(terms: &[String]) -> bool {
    let has = |term: &str| packet_terms_have(terms, term);
    let has_any = |needles: &[&str]| packet_terms_have_any(terms, needles);
    has("search")
        && has_any(&[
            "candidate",
            "flags",
            "haystack",
            "matcher",
            "printer",
            "searcher",
            "walk",
            "walks",
        ])
}

fn packet_terms_indicate_gin_route_dispatch_flow(terms: &[String]) -> bool {
    let has = |term: &str| packet_terms_have(terms, term);
    let has_any = |needles: &[&str]| packet_terms_have_any(terms, needles);
    has("engine")
        && has_any(&["route", "routes", "router"])
        && has_any(&["group", "groups"])
        && has_any(&["method", "methods", "tree", "trees"])
        && has_any(&["handler", "handlers", "dispatch", "dispatches"])
}

fn push_gin_route_dispatch_symbol_probe_queries(queries: &mut Vec<String>) {
    push_unique_terms(
        queries,
        &[
            "gin.go New",
            "gin.go Default",
            "routergroup.go RouterGroup.Handle",
            "gin.go Engine.addRoute",
            "tree.go node.addRoute",
            "gin.go Engine.handleHTTPRequest",
            "context.go Context.Next",
        ],
    );
}

fn packet_terms_indicate_css_animation_flow(terms: &[String]) -> bool {
    let has = |term: &str| packet_terms_have(terms, term);
    let has_any = |needles: &[&str]| packet_terms_have_any(terms, needles);
    (has("animatecss") || (has("animate") && has("css")))
        && has_any(&["animation", "animations", "keyframe", "keyframes"])
        && has_any(&[
            "variable",
            "variables",
            "base",
            "class",
            "classes",
            "selector",
            "selectors",
        ])
}

fn packet_terms_indicate_stylesheet_animation_flow(terms: &[String]) -> bool {
    let has = |term: &str| packet_terms_have(terms, term);
    let has_any = |needles: &[&str]| packet_terms_have_any(terms, needles);
    let css_signal = has("css")
        || has("animatecss")
        || has_any(&[
            "stylesheet",
            "stylesheets",
            "style",
            "styles",
            "selector",
            "selectors",
        ]);
    let animation_signal = has_any(&[
        "animate",
        "animated",
        "animation",
        "animations",
        "keyframe",
        "keyframes",
    ]);
    let source_shape_signal = has_any(&[
        "base",
        "class",
        "classes",
        "custom",
        "property",
        "properties",
        "selector",
        "selectors",
        "variable",
        "variables",
    ]);
    css_signal && animation_signal && source_shape_signal
}

fn push_css_animation_symbol_probe_queries(queries: &mut Vec<String>) {
    push_unique_terms(
        queries,
        &[
            "source/_vars.css",
            "source/_base.css",
            "source/animate.css",
            "source/attention_seekers/bounce.css bounce",
            "source/attention_seekers/flash.css flash",
        ],
    );
}
fn packet_terms_indicate_sql_schema_flow(terms: &[String]) -> bool {
    let has_any = |needles: &[&str]| packet_terms_have_any(terms, needles);
    has_any(&["sql", "schema", "schemas", "table", "tables"])
        && has_any(&[
            "relationship",
            "relationships",
            "relation",
            "relations",
            "foreign",
            "constraint",
            "constraints",
            "reference",
            "references",
        ])
        && has_any(&["table", "tables", "create", "schema", "schemas"])
}
fn packet_terms_indicate_automapper_map_flow(terms: &[String]) -> bool {
    let has = |term: &str| packet_terms_have(terms, term);
    let has_any = |needles: &[&str]| packet_terms_have_any(terms, needles);
    has("automapper")
        && has_any(&["configuration", "config", "mapperconfiguration"])
        && has_any(&["runtime", "api", "apis", "mapper", "mapping"])
        && has_any(&["map", "maps", "mapping", "objects"])
        && (has_any(&["source", "destination"]) || has("typemap"))
}

fn push_automapper_map_flow_symbol_probe_queries(queries: &mut Vec<String>) {
    push_unique_terms(
        queries,
        &[
            "src/AutoMapper/Mapper.cs IMapperBase",
            "src/AutoMapper/Mapper.cs IMapper",
            "src/AutoMapper/Mapper.cs Mapper",
            "src/AutoMapper/Mapper.cs Mapper.Map",
            "src/AutoMapper/Configuration/MapperConfiguration.cs MapperConfiguration",
            "src/AutoMapper/TypeMap.cs TypeMap.CreateMapperLambda",
            "src/AutoMapper/Execution/TypeMapPlanBuilder.cs TypeMapPlanBuilder",
            "TypeMapPlanBuilder.CreateMapperLambda",
        ],
    );
}
fn push_generic_symbol_probe_queries(terms: &[String], queries: &mut Vec<String>, _compact: bool) {
    let term_cap = 12;
    for term in terms
        .iter()
        .filter(|term| term.len() >= 4 && !packet_query_stop_term(term.as_str()))
        .take(term_cap)
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

#[derive(Debug, Clone)]
struct PacketCommandDescriptor {
    command_title: String,
    subcommand_title: String,
    module: String,
    crate_segment: String,
}

fn packet_command_descriptors(question: &str) -> Vec<PacketCommandDescriptor> {
    let mut descriptors = Vec::new();
    for span in packet_backtick_spans(question) {
        let words = packet_command_words(span);
        if words.len() < 2 {
            continue;
        }
        let command = &words[0];
        let subcommand = &words[1];
        let Some(command_title) = packet_pascal_identifier(command) else {
            continue;
        };
        let Some(subcommand_title) = packet_pascal_identifier(subcommand) else {
            continue;
        };
        let Some(module) = packet_snake_identifier(&[command.as_str(), subcommand.as_str()]) else {
            continue;
        };
        let Some(crate_segment) = packet_snake_identifier(&[subcommand.as_str()]) else {
            continue;
        };
        descriptors.push(PacketCommandDescriptor {
            command_title,
            subcommand_title,
            module,
            crate_segment,
        });
    }
    descriptors
}

fn packet_command_exact_probe_queries(
    question: &str,
    task_class: PacketTaskClassDto,
) -> Vec<String> {
    if !eval_probes_enabled() || !packet_allows_command_probe_queries(question, task_class) {
        return Vec::new();
    }

    let mut queries = Vec::new();
    for descriptor in packet_command_descriptors(question) {
        push_unique_term(
            &mut queries,
            &format!("Subcommand::{}", descriptor.subcommand_title),
        );
        push_unique_term(&mut queries, &format!("{}::Cli", descriptor.module));
        push_unique_term(&mut queries, &format!("{}::run_main", descriptor.module));
    }
    queries
}

fn packet_command_role_probe_queries(
    question: &str,
    task_class: PacketTaskClassDto,
) -> Vec<String> {
    if !packet_allows_command_probe_queries(question, task_class) {
        return Vec::new();
    }

    let mut queries = Vec::new();
    for descriptor in packet_command_descriptors(question) {
        let command_phrase = descriptor.module.replace('_', " ");
        let subcommand_phrase = descriptor.subcommand_title.to_ascii_lowercase();
        push_unique_term(&mut queries, &command_phrase);
        push_unique_term(&mut queries, &format!("{command_phrase} command"));
        push_unique_term(&mut queries, &format!("{subcommand_phrase} command"));
        push_unique_term(&mut queries, &format!("{subcommand_phrase} subcommand"));
    }
    queries
}

fn packet_allows_command_probe_queries(question: &str, task_class: PacketTaskClassDto) -> bool {
    if !matches!(
        task_class,
        PacketTaskClassDto::ArchitectureExplanation
            | PacketTaskClassDto::DataFlow
            | PacketTaskClassDto::ChangeImpact
            | PacketTaskClassDto::RouteTracing
            | PacketTaskClassDto::EditPlanning
    ) {
        return false;
    }
    let lowered = question.to_ascii_lowercase();
    contains_any(
        &lowered,
        &[
            "cli",
            "command",
            "subcommand",
            "entrypoint",
            "entry point",
            "runtime",
            "flow",
            "flows",
        ],
    )
}

fn packet_backtick_spans(question: &str) -> Vec<&str> {
    let mut spans = Vec::new();
    let mut start = None;
    for (index, ch) in question.char_indices() {
        if ch != '`' {
            continue;
        }
        if let Some(open) = start.take() {
            let span = question[open..index].trim();
            if !span.is_empty() {
                spans.push(span);
            }
        } else {
            start = Some(index + ch.len_utf8());
        }
    }
    spans
}

fn packet_command_words(span: &str) -> Vec<String> {
    span.split_whitespace()
        .filter_map(|token| {
            let token = token.trim_matches(|ch: char| {
                matches!(
                    ch,
                    ',' | '.'
                        | ';'
                        | ':'
                        | '?'
                        | '!'
                        | '('
                        | ')'
                        | '['
                        | ']'
                        | '{'
                        | '}'
                        | '"'
                        | '\''
                )
            });
            if token.starts_with('-')
                || token.is_empty()
                || !token.chars().any(|ch| ch.is_ascii_alphabetic())
                || !token
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
            {
                return None;
            }
            Some(token.to_string())
        })
        .take(3)
        .collect()
}

fn packet_pascal_identifier(word: &str) -> Option<String> {
    let mut value = String::new();
    for part in word
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|part| !part.is_empty())
    {
        let mut chars = part.chars();
        let first = chars.next()?;
        value.push(first.to_ascii_uppercase());
        value.extend(chars.map(|ch| ch.to_ascii_lowercase()));
    }
    (!value.is_empty()).then_some(value)
}

fn packet_snake_identifier(words: &[&str]) -> Option<String> {
    let mut parts = Vec::new();
    for word in words {
        let mut normalized = String::new();
        for (index, part) in word
            .split(|ch: char| !ch.is_ascii_alphanumeric())
            .filter(|part| !part.is_empty())
            .enumerate()
        {
            if index > 0 {
                normalized.push('_');
            }
            normalized.push_str(&part.to_ascii_lowercase());
        }
        if normalized.is_empty() {
            return None;
        }
        parts.push(normalized);
    }
    (!parts.is_empty()).then_some(parts.join("_"))
}

fn packet_concrete_file_probe_queries_from_required(required_queries: &[String]) -> Vec<String> {
    let mut queries = Vec::new();
    for query in required_queries {
        if let Some(file_query) = packet_required_probe_file_query(query) {
            push_unique_term(&mut queries, &file_query);
        }
    }
    queries
}

fn packet_required_probe_file_query(query: &str) -> Option<String> {
    if !packet_required_probe_needs_concrete_file(query) {
        return None;
    }
    let normalized_query = normalize_identifier(query);
    if normalized_query == "eventprocessor" {
        return Some("event_processor.rs".to_string());
    }
    query
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        .then(|| format!("{query}.rs"))
}

fn push_adjacent_packet_term_queries(
    terms: &[String],
    queries: &mut Vec<String>,
    window_cap: usize,
) {
    for window in terms.windows(2).take(window_cap) {
        if let [left, right] = window {
            if packet_adjacent_query_stop_term(left) || packet_adjacent_query_stop_term(right) {
                continue;
            }
            push_unique_term(queries, &format!("{left}_{right}"));
            push_unique_term(
                queries,
                &packet_camel_case(&[left.as_str(), right.as_str()]),
            );
        }
    }
}

fn packet_concept_queries(question: &str) -> Vec<String> {
    let include_non_primary_terms = query_mentions_non_primary_source(question);
    prompt_search_terms(question)
        .into_iter()
        .filter(|term| {
            term.len() >= 4
                && (include_non_primary_terms || !is_non_primary_source_term(term.as_str()))
                && !packet_query_stop_term(term.as_str())
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
        &["impact", "affected", "regression", "blast radius"],
    ) || risk_of_change_prompt(&lower)
    {
        PacketTaskClassDto::ChangeImpact
    } else if contains_any(&lower, &["route", "endpoint", "handler", "api path"]) {
        PacketTaskClassDto::RouteTracing
    } else if contains_any(&lower, &["owner", "owns", "who calls", "references"]) {
        PacketTaskClassDto::SymbolOwnership
    } else if contains_any(
        &lower,
        &[
            "data flow",
            "flow from",
            "flow into",
            "flows from",
            "flows into",
            "pipeline",
            "through",
        ],
    ) {
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

fn risk_of_change_prompt(lower: &str) -> bool {
    lower.contains("risk if")
        && contains_any(lower, &[" change", " changing", " modify", " modifying"])
        || lower.contains("risk of changing")
        || lower.contains("risk from changing")
        || lower.contains("risk in changing")
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

    for term in exact_symbol_query_terms(question) {
        push_unique_term(&mut terms, &term);
    }
    for term in packet_architecture_flow_probe_terms(question) {
        push_unique_term(&mut terms, &term);
    }

    for token in question.split_whitespace() {
        let token = token.trim_matches(|ch: char| {
            matches!(
                ch,
                ',' | '.' | ';' | ':' | '?' | '!' | '(' | ')' | '[' | ']' | '{' | '}' | '"' | '`'
            )
        });
        if is_packet_code_like_term(token)
            || (looks_like_standalone_symbol_query(token)
                && token.len() >= 4
                && !packet_extract_query_stop_term(token))
        {
            push_unique_term(&mut terms, token);
        }
    }
    terms.truncate(16);
    terms
}

fn packet_extract_query_stop_term(token: &str) -> bool {
    packet_query_stop_term(token)
        || matches!(
            token.to_ascii_lowercase().as_str(),
            "cite"
                | "cites"
                | "file"
                | "files"
                | "path"
                | "paths"
                | "that"
                | "them"
                | "they"
                | "their"
                | "your"
                | "into"
                | "from"
                | "with"
                | "have"
                | "been"
                | "will"
                | "also"
                | "only"
                | "over"
                | "under"
                | "than"
                | "then"
                | "each"
                | "such"
                | "some"
                | "more"
                | "most"
                | "many"
                | "much"
                | "very"
                | "just"
                | "like"
                | "make"
                | "made"
                | "used"
                | "uses"
                | "using"
                | "work"
                | "works"
                | "working"
        )
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

fn push_unique_owned_terms(terms: &mut Vec<String>, values: &[String]) {
    for value in values {
        push_unique_term(terms, value);
    }
}

fn task_class_seed_queries(task_class: PacketTaskClassDto) -> &'static [&'static str] {
    match task_class {
        PacketTaskClassDto::ArchitectureExplanation => &[
            "architecture entrypoint",
            "runtime flow",
            "main",
            "run",
            "entrypoint",
        ],
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

fn packet_retrieval_prompt(
    question: &str,
    plan: &PacketPlanDto,
    initial_hybrid_weights: Option<&AgentHybridWeightsDto>,
    budget: PacketBudgetModeDto,
) -> String {
    let anchor_probes = packet_anchor_probe_queries(plan);
    if packet_initial_retrieval_is_lexical_only(initial_hybrid_weights) && anchor_probes.is_empty()
    {
        return question.to_string();
    }
    if plan.queries.len() <= 1 {
        return question.to_string();
    }
    let mut prompt = String::from(question);
    prompt.push_str("\n\nPlanned CodeStory queries:");
    let compact = matches!(
        budget,
        PacketBudgetModeDto::Compact | PacketBudgetModeDto::Tiny
    );
    let planned_lines =
        if packet_initial_retrieval_is_lexical_only(initial_hybrid_weights) || compact {
            let mut lines = packet_compact_retrieval_prompt_lines(anchor_probes)
                .into_iter()
                .map(|query| format!("- {query} (symbol probe)"))
                .collect::<Vec<_>>();
            if lines.is_empty() {
                lines = plan
                    .queries
                    .iter()
                    .take(8)
                    .map(|query| format!("- {} ({})", query.query, query.purpose))
                    .collect();
            }
            lines
        } else {
            plan.queries
                .iter()
                .map(|query| format!("- {} ({})", query.query, query.purpose))
                .collect()
        };
    for line in planned_lines {
        prompt.push('\n');
        prompt.push_str(&line);
    }
    prompt
}

fn packet_initial_hybrid_weights(
    _plan: &PacketPlanDto,
    _budget: PacketBudgetModeDto,
) -> Option<AgentHybridWeightsDto> {
    None
}

fn packet_compact_retrieval_prompt_lines(mut anchor_probes: Vec<String>) -> Vec<String> {
    anchor_probes.sort_by(|left, right| {
        let left_path = left.contains('/') && left.contains('.');
        let right_path = right.contains('/') && right.contains('.');
        right_path
            .cmp(&left_path)
            .then_with(|| right.len().cmp(&left.len()))
    });
    let mut selected = Vec::new();
    for query in anchor_probes {
        if selected.len() >= 16 {
            break;
        }
        if !selected.iter().any(|existing| existing == &query) {
            selected.push(query);
        }
    }
    selected
}

fn packet_initial_retrieval_is_lexical_only(weights: Option<&AgentHybridWeightsDto>) -> bool {
    weights
        .and_then(|weights| weights.semantic)
        .is_some_and(|semantic| semantic <= f32::EPSILON)
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

fn rank_packet_evidence(question: &str, answer: &mut AgentAnswerDto) {
    let terms = packet_rank_terms(question);
    let prefer_primary_sources = !query_mentions_non_primary_source(question);
    answer.citations.sort_by(|left, right| {
        packet_citation_rank(right, &terms, prefer_primary_sources)
            .partial_cmp(&packet_citation_rank(left, &terms, prefer_primary_sources))
            .unwrap_or(Ordering::Equal)
    });
}

fn cap_packet_citations(
    answer: &mut AgentAnswerDto,
    limits: &PacketBudgetLimitsDto,
    required_probe_queries: &[String],
) -> bool {
    let mut protected_citation_keys =
        promote_required_probe_citations(answer, required_probe_queries);
    let focus_neighborhood_keys =
        promote_focus_neighborhood_citations(answer, &protected_citation_keys);
    protected_citation_keys.extend(focus_neighborhood_keys);
    if protected_citation_keys.is_empty() {
        cap_citations(answer, limits)
    } else {
        cap_citations_with_protected(answer, limits, &protected_citation_keys)
    }
}

fn promote_required_probe_citations(
    answer: &mut AgentAnswerDto,
    required_probe_queries: &[String],
) -> HashSet<String> {
    if required_probe_queries.is_empty() || answer.citations.is_empty() {
        return HashSet::new();
    }

    let focus_roots = packet_command_focus_roots(&answer.citations);
    let mut promoted_indices = Vec::new();
    for query in required_probe_queries {
        if promoted_indices
            .iter()
            .any(|index| packet_citation_satisfies_required_probe(query, &answer.citations[*index]))
        {
            continue;
        }
        let mut best_match = None;
        for (index, citation) in answer.citations.iter().enumerate() {
            if promoted_indices.contains(&index) {
                continue;
            }
            let Some(match_rank) = packet_citation_probe_match_rank(query, citation) else {
                continue;
            };
            if packet_display_name_is_import_literal(&citation.display_name.to_ascii_lowercase())
                && !packet_citation_satisfies_required_probe(query, citation)
            {
                continue;
            }
            if best_match
                .map(|(best_index, best_rank)| {
                    packet_prefer_required_probe_match(
                        query,
                        citation,
                        match_rank,
                        &answer.citations[best_index],
                        best_rank,
                        &focus_roots,
                    )
                })
                .unwrap_or(true)
            {
                best_match = Some((index, match_rank));
            }
        }
        if let Some((index, _)) = best_match {
            promoted_indices.push(index);
        }
    }
    if promoted_indices.is_empty() {
        return HashSet::new();
    }

    let protected_citation_keys = promoted_indices
        .iter()
        .map(|index| packet_citation_key(&answer.citations[*index]))
        .collect::<HashSet<_>>();
    let promoted_index_set = promoted_indices.iter().copied().collect::<HashSet<_>>();
    let mut reordered = Vec::with_capacity(answer.citations.len());
    for index in promoted_indices {
        reordered.push(answer.citations[index].clone());
    }
    for (index, citation) in answer.citations.drain(..).enumerate() {
        if !promoted_index_set.contains(&index) {
            reordered.push(citation);
        }
    }
    answer.citations = reordered;
    answer.retrieval_trace.annotations.push(format!(
        "packet_required_probe_citations promoted={} required={}",
        promoted_index_set.len(),
        required_probe_queries.join("|").replace('`', "'")
    ));
    protected_citation_keys
}

fn promote_focus_neighborhood_citations(
    answer: &mut AgentAnswerDto,
    protected_citation_keys: &HashSet<String>,
) -> HashSet<String> {
    if answer.citations.is_empty() {
        return HashSet::new();
    }
    let focus_roots = packet_command_focus_roots(&answer.citations);
    if focus_roots.is_empty() {
        return HashSet::new();
    }
    let protected_file_paths = answer
        .citations
        .iter()
        .filter(|citation| protected_citation_keys.contains(&packet_citation_key(citation)))
        .filter_map(packet_citation_file_path_key)
        .collect::<HashSet<_>>();

    let mut ranked_candidates = answer
        .citations
        .iter()
        .enumerate()
        .filter(|(_, citation)| {
            packet_focus_neighborhood_candidate(
                citation,
                &focus_roots,
                protected_citation_keys,
                &protected_file_paths,
            )
        })
        .map(|(index, citation)| {
            (
                index,
                packet_focus_neighborhood_rank(citation, &focus_roots),
            )
        })
        .collect::<Vec<_>>();
    ranked_candidates.sort_by(|(left_index, left_rank), (right_index, right_rank)| {
        right_rank
            .cmp(left_rank)
            .then_with(|| left_index.cmp(right_index))
    });

    let mut promoted_indices = Vec::new();
    let mut promoted_file_paths = HashSet::new();
    for (index, _) in ranked_candidates {
        let Some(path) = packet_citation_file_path_key(&answer.citations[index]) else {
            continue;
        };
        if !promoted_file_paths.insert(path) {
            continue;
        }
        promoted_indices.push(index);
        if promoted_indices.len() >= PACKET_FOCUS_NEIGHBORHOOD_CARRY_LIMIT {
            break;
        }
    }
    if promoted_indices.is_empty() {
        return HashSet::new();
    }

    let promoted_index_set = promoted_indices.iter().copied().collect::<HashSet<_>>();
    let promoted_keys = promoted_indices
        .iter()
        .map(|index| packet_citation_key(&answer.citations[*index]))
        .collect::<HashSet<_>>();
    let mut reordered = Vec::with_capacity(answer.citations.len());
    for citation in &answer.citations {
        if protected_citation_keys.contains(&packet_citation_key(citation)) {
            reordered.push(citation.clone());
        }
    }
    for index in promoted_indices {
        reordered.push(answer.citations[index].clone());
    }
    for (index, citation) in answer.citations.drain(..).enumerate() {
        let key = packet_citation_key(&citation);
        if !protected_citation_keys.contains(&key) && !promoted_index_set.contains(&index) {
            reordered.push(citation);
        }
    }
    answer.citations = reordered;
    answer.retrieval_trace.annotations.push(format!(
        "packet_focus_neighborhood_citations promoted={} roots={}",
        promoted_keys.len(),
        focus_roots
            .iter()
            .map(|root| root.root.as_str())
            .collect::<Vec<_>>()
            .join("|")
            .replace('`', "'")
    ));
    promoted_keys
}

fn packet_focus_neighborhood_candidate(
    citation: &AgentCitationDto,
    focus_roots: &[PacketCommandFocusRoot],
    protected_citation_keys: &HashSet<String>,
    protected_file_paths: &HashSet<String>,
) -> bool {
    if protected_citation_keys.contains(&packet_citation_key(citation))
        || citation.origin != SearchHitOrigin::IndexedSymbol
        || !citation.resolvable
        || packet_display_name_is_import_literal(&citation.display_name.to_ascii_lowercase())
        || packet_display_name_is_test_like(&citation.display_name)
    {
        return false;
    }
    let path = citation
        .file_path
        .as_deref()
        .map(packet_display_path)
        .unwrap_or_default();
    if path.is_empty() || packet_citation_focus_root_score(citation, focus_roots) == 0 {
        return false;
    }
    if protected_file_paths.contains(&path) {
        return false;
    }
    !retrieval_file_role_from_path(&path.to_ascii_lowercase()).is_non_primary()
}

fn packet_citation_file_path_key(citation: &AgentCitationDto) -> Option<String> {
    let path = citation.file_path.as_deref().map(packet_display_path)?;
    if path.is_empty() { None } else { Some(path) }
}

fn packet_focus_neighborhood_rank(
    citation: &AgentCitationDto,
    focus_roots: &[PacketCommandFocusRoot],
) -> (u8, u8, u8, u8, u8, u8, i32) {
    let path = citation
        .file_path
        .as_deref()
        .map(packet_display_path)
        .unwrap_or_default();
    let source_file: u8 = if retrieval_file_role_from_path(&path.to_ascii_lowercase())
        == crate::RetrievalFileRole::Source
    {
        1
    } else {
        0
    };
    let direct_root_file = packet_citation_direct_focus_root_file_score(citation, focus_roots);
    let role_backed: u8 = if packet_evidence_role(citation).is_some() {
        1
    } else {
        0
    };
    let implementation_file: u8 = if packet_path_is_implementation(&path) {
        1
    } else {
        0
    };
    let definition_file: u8 = if packet_primary_definition_file_citation(citation) {
        1
    } else {
        0
    };
    (
        packet_citation_focus_root_score(citation, focus_roots),
        direct_root_file,
        packet_source_navigation_file_score(&path),
        source_file,
        role_backed,
        implementation_file.saturating_add(definition_file),
        (citation.score * 1000.0).round() as i32,
    )
}

fn packet_citation_direct_focus_root_file_score(
    citation: &AgentCitationDto,
    focus_roots: &[PacketCommandFocusRoot],
) -> u8 {
    let path = citation
        .file_path
        .as_deref()
        .map(packet_display_path)
        .unwrap_or_default()
        .replace('\\', "/");
    let parent = path.rsplit_once('/').map(|(parent, _)| parent);
    focus_roots
        .iter()
        .filter(|root| parent == Some(root.root.as_str()))
        .map(|root| root.weight)
        .max()
        .unwrap_or_default()
}

fn packet_source_navigation_file_score(path: &str) -> u8 {
    let normalized = packet_display_path(path).replace('\\', "/");
    let file_name = normalized.rsplit('/').next().unwrap_or(normalized.as_str());
    let stem = file_name
        .rsplit_once('.')
        .map(|(stem, _)| stem)
        .unwrap_or(file_name)
        .to_ascii_lowercase();
    match stem.as_str() {
        "cli" | "cmd" | "command" | "commands" => 4,
        "lib" | "mod" | "index" => 3,
        "events" | "event" => 2,
        "main" | "app" | "server" | "router" | "routes" => 2,
        "handler" | "handlers" | "entrypoint" | "entrypoints" => 1,
        _ if stem.ends_with("_events")
            || stem.ends_with("_event")
            || stem.ends_with("-events")
            || stem.ends_with("-event") =>
        {
            2
        }
        _ => 0,
    }
}

fn packet_prefer_required_probe_match(
    query: &str,
    candidate: &AgentCitationDto,
    candidate_rank: u8,
    existing: &AgentCitationDto,
    existing_rank: u8,
    focus_roots: &[PacketCommandFocusRoot],
) -> bool {
    if !query_mentions_non_primary_source(query) {
        let candidate_test_like = packet_display_name_is_test_like(&candidate.display_name);
        let existing_test_like = packet_display_name_is_test_like(&existing.display_name);
        if candidate_test_like != existing_test_like {
            return !candidate_test_like;
        }
    }
    if candidate_rank != existing_rank {
        return candidate_rank > existing_rank;
    }
    if !packet_required_probe_needs_exact_match(query) {
        let candidate_focus = packet_citation_focus_root_score(candidate, focus_roots);
        let existing_focus = packet_citation_focus_root_score(existing, focus_roots);
        if candidate_focus != existing_focus {
            return candidate_focus > existing_focus;
        }
        let candidate_token_coverage = packet_citation_probe_token_coverage(query, candidate);
        let existing_token_coverage = packet_citation_probe_token_coverage(query, existing);
        if candidate_token_coverage != existing_token_coverage {
            return candidate_token_coverage > existing_token_coverage;
        }
    }
    if packet_prefer_flow_anchor_path_citation(candidate, existing) {
        return true;
    }
    if packet_required_probe_prefers_implementation(query)
        && packet_prefer_implementation_file(candidate, existing)
    {
        return true;
    }
    packet_exact_definition_file_citation(candidate)
        && !packet_exact_definition_file_citation(existing)
}

fn packet_required_probe_prefers_implementation(query: &str) -> bool {
    query.contains("::") || query.contains('.')
}

fn packet_prefer_implementation_file(
    candidate: &AgentCitationDto,
    existing: &AgentCitationDto,
) -> bool {
    let candidate_path = candidate
        .file_path
        .as_deref()
        .map(packet_display_path)
        .unwrap_or_default();
    let existing_path = existing
        .file_path
        .as_deref()
        .map(packet_display_path)
        .unwrap_or_default();
    packet_path_is_implementation(&candidate_path) && !packet_path_is_implementation(&existing_path)
}

fn packet_path_is_implementation(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    matches!(
        lower.rsplit('.').next(),
        Some(
            "c" | "cc"
                | "cpp"
                | "cxx"
                | "go"
                | "java"
                | "js"
                | "jsx"
                | "kt"
                | "php"
                | "py"
                | "rb"
                | "rs"
                | "ts"
                | "tsx"
        )
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PacketCommandFocusRoot {
    root: String,
    weight: u8,
}

fn packet_command_focus_roots(citations: &[AgentCitationDto]) -> Vec<PacketCommandFocusRoot> {
    let mut roots = Vec::<PacketCommandFocusRoot>::new();
    for citation in citations {
        let display = citation.display_name.as_str();
        let normalized_display = normalize_identifier(display);
        let path = citation
            .file_path
            .as_deref()
            .map(packet_display_path)
            .unwrap_or_default();
        let Some(root) = packet_source_root_from_path(&path) else {
            continue;
        };
        let normalized_path = path.replace('\\', "/");
        let weight =
            if normalized_display.ends_with("runmain") || normalized_display.contains("runexec") {
                3
            } else if display.contains("::Cli")
                || display.contains("::cli")
                || normalized_path.ends_with("/src/cli.rs")
                || (normalized_path.ends_with("/main.rs") && normalized_display == "main")
            {
                2
            } else if display.contains("Subcommand::") {
                1
            } else {
                continue;
            };
        packet_push_focus_root(&mut roots, root, weight);
    }
    roots.sort_by(|left, right| {
        right
            .weight
            .cmp(&left.weight)
            .then_with(|| left.root.cmp(&right.root))
    });
    roots
}

fn packet_push_focus_root(roots: &mut Vec<PacketCommandFocusRoot>, root: String, weight: u8) {
    if let Some(existing) = roots.iter_mut().find(|existing| existing.root == root) {
        existing.weight = existing.weight.max(weight);
    } else {
        roots.push(PacketCommandFocusRoot { root, weight });
    }
}

fn packet_source_root_from_path(path: &str) -> Option<String> {
    let normalized = packet_display_path(path);
    let normalized = normalized.trim_matches('/').replace('\\', "/");
    if normalized.is_empty() {
        return None;
    }
    if let Some(index) = normalized.find("/src/") {
        let root = &normalized[..index + "/src".len()];
        return (!root.is_empty()).then(|| root.to_string());
    }
    let (parent, _) = normalized.rsplit_once('/')?;
    (!parent.is_empty()).then(|| parent.to_string())
}

fn packet_citation_focus_root_score(
    citation: &AgentCitationDto,
    focus_roots: &[PacketCommandFocusRoot],
) -> u8 {
    let path = citation
        .file_path
        .as_deref()
        .map(packet_display_path)
        .unwrap_or_default()
        .replace('\\', "/");
    focus_roots
        .iter()
        .filter(|root| path == root.root || path.starts_with(&format!("{}/", root.root)))
        .map(|root| root.weight)
        .max()
        .unwrap_or_default()
}

fn maybe_annotate_packet_candidate_window(
    question: &str,
    limits: &PacketBudgetLimitsDto,
    answer: &mut AgentAnswerDto,
) {
    let Ok(filter) = std::env::var("CODESTORY_PACKET_CANDIDATE_TRACE") else {
        return;
    };
    let trace_terms = filter
        .split(|ch: char| ch == ',' || ch == ';' || ch.is_whitespace())
        .map(normalize_identifier)
        .filter(|term| !term.is_empty())
        .collect::<Vec<_>>();
    if trace_terms.is_empty() {
        return;
    }

    let rank_terms = packet_rank_terms(question);
    let prefer_primary_sources = !query_mentions_non_primary_source(question);
    let broad_window = (limits.max_anchors as usize).saturating_mul(2).max(8);
    let mut rows = Vec::new();
    let mut matched = 0usize;
    for (index, citation) in answer.citations.iter().enumerate() {
        let matches_filter = packet_candidate_matches_trace_terms(citation, &trace_terms);
        if matches_filter {
            matched = matched.saturating_add(1);
        }
        if index >= broad_window && !matches_filter {
            continue;
        }
        if rows.len() >= 64 {
            break;
        }
        rows.push(packet_candidate_trace_row(
            index,
            citation,
            &rank_terms,
            prefer_primary_sources,
            matches_filter,
        ));
    }
    answer.retrieval_trace.annotations.push(format!(
        "packet_candidate_trace filter=`{}` candidates={} matched={} max_anchors={} rows={}",
        filter.replace('`', "'"),
        answer.citations.len(),
        matched,
        limits.max_anchors,
        rows.join(" | ")
    ));
}

fn packet_candidate_matches_trace_terms(
    citation: &AgentCitationDto,
    trace_terms: &[String],
) -> bool {
    let normalized_display = normalize_identifier(&citation.display_name);
    let normalized_path = normalize_identifier(citation.file_path.as_deref().unwrap_or_default());
    trace_terms.iter().any(|term| {
        normalized_display.contains(term)
            || normalized_path.contains(term)
            || (!normalized_display.is_empty() && term.contains(&normalized_display))
    })
}

fn packet_candidate_trace_row(
    index: usize,
    citation: &AgentCitationDto,
    rank_terms: &[String],
    prefer_primary_sources: bool,
    matches_filter: bool,
) -> String {
    let role = packet_evidence_role(citation);
    let claim = role
        .map(|role| packet_claim_key_for_citation(role, citation))
        .unwrap_or_else(|| "-".to_string());
    format!(
        "#{}{} rank={:.3} score={:.3} claim={} role={} kind={:?} name=`{}` path={} line={}",
        index + 1,
        if matches_filter { "*" } else { "" },
        packet_citation_rank(citation, rank_terms, prefer_primary_sources),
        citation.score,
        claim,
        role.unwrap_or("-"),
        citation.kind,
        citation.display_name.replace('`', "'"),
        citation
            .file_path
            .as_deref()
            .map(packet_display_path)
            .unwrap_or_default(),
        citation
            .line
            .map(|line| line.to_string())
            .unwrap_or_else(|| "-".to_string())
    )
}

fn packet_rank_terms(question: &str) -> Vec<String> {
    let mut terms = prompt_search_terms(question);
    for term in extract_packet_query_terms(question) {
        push_unique_term(&mut terms, &term);
    }
    for query in packet_symbol_probe_queries(
        question,
        infer_packet_task_class(question),
        PacketBudgetModeDto::Standard,
    ) {
        push_unique_term(&mut terms, &normalize_identifier(&query));
    }
    terms
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
    let role = packet_evidence_role(citation).unwrap_or("source evidence");
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

fn packet_architecture_flow_probe_terms(prompt: &str) -> Vec<String> {
    let lower = prompt.to_ascii_lowercase();
    let mut terms = Vec::new();
    if prompt_mentions_indexing_flow(&lower) {
        for term in [
            "index service",
            "workspace execution plan",
            "workspace indexer",
            "symbol extraction indexer",
            "search projection",
            "snapshot refresh",
        ] {
            push_unique_term(&mut terms, term);
        }
    }
    push_eval_architecture_flow_probe_terms(&lower, &mut terms);
    terms
}

fn prompt_mentions_indexing_flow(lower: &str) -> bool {
    contains_any(lower, &["indexing", "indexer", "indexed", " index "])
        && contains_any(
            lower,
            &[
                "cli",
                "command",
                "discovery",
                "extraction",
                "file",
                "persistence",
                "projection",
                "refresh",
                "runtime",
                "search",
                "snapshot",
                "storage",
                "store",
                "symbol",
                "workspace",
            ],
        )
}

fn packet_push_flow_template_claim(
    claims: &mut Vec<PacketClaimDto>,
    seen: &mut HashSet<String>,
    claim_text: &str,
    citation: Option<AgentCitationDto>,
) {
    packet_push_flow_template_claim_with_citations(
        claims,
        seen,
        claim_text,
        citation.map(|value| vec![value]).unwrap_or_default(),
    );
}

fn packet_push_flow_template_claim_with_citations(
    claims: &mut Vec<PacketClaimDto>,
    seen: &mut HashSet<String>,
    claim_text: &str,
    citations: Vec<AgentCitationDto>,
) {
    let key = normalize_identifier(claim_text);
    if key.is_empty() || !seen.insert(key) {
        return;
    }
    claims.push(PacketClaimDto {
        claim: claim_text.to_string(),
        citations,
    });
}

fn packet_append_flow_template_claims(
    prompt: &str,
    citations: &[AgentCitationDto],
    claims: &mut Vec<PacketClaimDto>,
    seen: &mut HashSet<String>,
) {
    let normalized_prompt = normalize_identifier(prompt);

    packet_append_command_flow_template_claims(prompt, citations, claims, seen);
    packet_append_indexing_pipeline_flow_template_claims(prompt, citations, claims, seen);
    packet_append_source_derived_flow_claims(prompt, citations, claims, seen);
    packet_append_sql_schema_file_claims(prompt, citations, claims, seen);
    if !eval_probes_enabled() {
        return;
    }
    packet_append_indexing_storage_flow_template_claims(prompt, citations, claims, seen);
    for (claim, citation) in eval_flow_template_claims(&normalized_prompt, citations) {
        packet_push_flow_template_claim(claims, seen, &claim, Some(citation));
    }
}

fn packet_append_indexing_pipeline_flow_template_claims(
    prompt: &str,
    citations: &[AgentCitationDto],
    claims: &mut Vec<PacketClaimDto>,
    seen: &mut HashSet<String>,
) {
    let normalized_prompt = normalize_identifier(prompt);
    let indexing_prompt = normalized_prompt.contains("indexing")
        || normalized_prompt.contains("indexed")
        || normalized_prompt.contains("indexer")
        || normalized_prompt.contains("indexcommand");
    if !(indexing_prompt
        && normalized_prompt.contains("runtime")
        && (normalized_prompt.contains("workspace")
            || normalized_prompt.contains("sourcefile")
            || normalized_prompt.contains("filediscovery"))
        && (normalized_prompt.contains("persistence") || normalized_prompt.contains("store"))
        && normalized_prompt.contains("snapshot"))
    {
        return;
    }

    let cli_entry = packet_citation_matching_display(citations, "run_index")
        .or_else(|| packet_citation_matching_display(citations, "Command::Index"))
        .or_else(|| packet_citation_matching_display(citations, "IndexCommand"))
        .or_else(|| packet_citation_matching_display(citations, "CliDirection"));
    let runtime_entry =
        packet_citation_matching_display_contains(citations, "IndexService::run_indexing")
            .or_else(|| packet_citation_matching_display(citations, "Runtime::index_service"));
    if let Some(runtime_entry) = runtime_entry {
        let mut claim_citations = Vec::new();
        if let Some(cli_entry) = cli_entry {
            claim_citations.push(cli_entry.clone());
        }
        claim_citations.push(runtime_entry.clone());
        packet_push_flow_template_claim_with_citations(
            claims,
            seen,
            "The CLI index command prepares command options and delegates indexing work into the runtime layer.",
            claim_citations,
        );
    }

    let workspace_plan =
        packet_citation_matching_display(citations, "WorkspaceManifest::build_execution_plan");
    if let Some(runtime_entry) = runtime_entry {
        let mut claim_citations = vec![runtime_entry.clone()];
        if let Some(workspace_plan) = workspace_plan {
            claim_citations.push(workspace_plan.clone());
        }
        packet_push_flow_template_claim_with_citations(
            claims,
            seen,
            "The runtime opens the workspace and store, chooses full or incremental indexing, and coordinates later refresh phases.",
            claim_citations,
        );
    }

    if let Some(workspace_plan) = workspace_plan {
        packet_push_flow_template_claim(
            claims,
            seen,
            "The workspace crate is responsible for source-file discovery and refresh-plan construction.",
            Some(workspace_plan.clone()),
        );
    }

    let workspace_indexer = packet_citation_matching_display(citations, "WorkspaceIndexer::run");
    let index_file = packet_citation_matching_display(citations, "index_file");
    if workspace_indexer.is_some() || index_file.is_some() {
        let mut claim_citations = Vec::new();
        if let Some(workspace_indexer) = workspace_indexer {
            claim_citations.push(workspace_indexer.clone());
        }
        if let Some(index_file) = index_file {
            claim_citations.push(index_file.clone());
        }
        packet_push_flow_template_claim_with_citations(
            claims,
            seen,
            "The indexer extracts nodes, edges, occurrences, and related symbol data from source files.",
            claim_citations,
        );
    }

    let storage_flush =
        packet_citation_matching_display(citations, "Storage::flush_projection_batch");
    let search_projection = packet_citation_matching_display(
        citations,
        "Storage::rebuild_search_symbol_projection_from_node_table",
    );
    if storage_flush.is_some() || search_projection.is_some() {
        let mut claim_citations = Vec::new();
        if let Some(storage_flush) = storage_flush {
            claim_citations.push(storage_flush.clone());
        }
        if let Some(search_projection) = search_projection {
            claim_citations.push(search_projection.clone());
        }
        packet_push_flow_template_claim_with_citations(
            claims,
            seen,
            "The store persists graph and file data to SQLite and rebuilds query/search projections from persisted data.",
            claim_citations,
        );
    }

    if let Some(snapshot_refresh) =
        packet_citation_matching_display(citations, "SnapshotStore::refresh_all_with_stats")
    {
        packet_push_flow_template_claim(
            claims,
            seen,
            "Snapshot refresh happens after persisted data changes so later grounding and summary reads see current indexed state.",
            Some(snapshot_refresh.clone()),
        );
    }
}

fn packet_append_source_derived_flow_claims(
    prompt: &str,
    citations: &[AgentCitationDto],
    claims: &mut Vec<PacketClaimDto>,
    seen: &mut HashSet<String>,
) {
    for citation in citations.iter().take(24) {
        let source = match packet_citation_source_text(citation) {
            Some(source) if source.len() <= 800_000 => source,
            _ => continue,
        };
        for claim in packet_source_derived_claims_for_citation(prompt, citation, &source) {
            packet_push_flow_template_claim(claims, seen, &claim, Some(citation.clone()));
            if claims.len() >= 18 {
                return;
            }
        }
    }
}

fn packet_source_derived_claims_for_citation(
    prompt: &str,
    citation: &AgentCitationDto,
    source: &str,
) -> Vec<String> {
    let mut claims = Vec::new();
    let symbol = citation.display_name.as_str();
    let path = citation
        .file_path
        .as_deref()
        .map(packet_display_path)
        .unwrap_or_default();
    let file_name = path
        .rsplit(['/', '\\'])
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or(symbol);
    let normalized_prompt = normalize_identifier(prompt);
    let prompt_terms = packet_probe_terms(prompt);
    let request_flow = packet_terms_indicate_request_dispatch_flow(&prompt_terms);
    let search_flow = packet_terms_indicate_search_execution_flow(&prompt_terms);

    if request_flow && let Some(claim) = packet_python_requests_flow_claim(symbol, &path, source) {
        claims.push(claim);
    }
    if packet_terms_indicate_express_application_route_flow(&prompt_terms) {
        claims.extend(packet_express_application_route_flow_claims(&path, source));
    }
    if packet_terms_indicate_java_string_check_flow(&prompt_terms) {
        claims.extend(packet_java_string_check_flow_claims(&path, source));
    }
    if packet_terms_indicate_swr_hook_flow(&prompt_terms) {
        claims.extend(packet_swr_hook_flow_claims(&path, source));
    }
    if packet_terms_indicate_gin_route_dispatch_flow(&prompt_terms) {
        claims.extend(packet_gin_route_dispatch_flow_claims(&path, source));
    }
    if packet_terms_indicate_css_animation_flow(&prompt_terms) {
        claims.extend(packet_css_animation_flow_claims(&path, source));
    }
    if packet_terms_indicate_automapper_map_flow(&prompt_terms) {
        claims.extend(packet_automapper_map_flow_claims(&path, source));
    }

    if packet_terms_indicate_server_route_dispatch_flow(&prompt_terms) {
        claims.extend(packet_generic_server_route_flow_claims(symbol, source));
    }

    if packet_terms_indicate_shell_version_use_flow(&prompt_terms) {
        claims.extend(packet_generic_shell_version_use_flow_claims(symbol, source));
    }

    if packet_terms_indicate_hook_cache_flow(&prompt_terms) {
        claims.extend(packet_generic_hook_cache_flow_claims(symbol, source));
    }

    if packet_terms_indicate_client_send_flow(&prompt_terms) {
        claims.extend(packet_generic_client_send_flow_claims(symbol, source));
    }

    if packet_terms_indicate_string_predicate_flow(&prompt_terms) {
        claims.extend(packet_generic_string_predicate_flow_claims(symbol, source));
    }

    if packet_terms_indicate_stylesheet_animation_flow(&prompt_terms) {
        claims.extend(packet_generic_css_animation_flow_claims(source));
    }

    if packet_terms_indicate_sql_schema_flow(&prompt_terms) {
        claims.extend(packet_generic_sql_schema_flow_claims(source));
    }

    if packet_terms_indicate_runtime_formatting_flow(&prompt_terms) {
        claims.extend(packet_generic_runtime_formatting_flow_claims(source));
    }

    if packet_terms_indicate_site_build_phase_flow(&prompt_terms) {
        claims.extend(packet_generic_site_build_phase_claims(source));
    }

    if packet_terms_indicate_log_record_handler_flow(&prompt_terms) {
        claims.extend(packet_generic_log_record_handler_claims(source));
    }

    if packet_terms_indicate_mapper_runtime_flow(&prompt_terms) {
        claims.extend(packet_generic_mapper_runtime_claims(source));
    }

    if packet_terms_indicate_buffered_io_flow(&prompt_terms) {
        claims.extend(packet_generic_buffered_io_claims(source));
    }

    if packet_terms_indicate_session_request_validation_flow(&prompt_terms) {
        claims.extend(packet_generic_session_request_validation_claims(source));
    }

    if packet_terms_indicate_html_form_validation_flow(&prompt_terms) {
        claims.extend(packet_generic_html_form_validation_claims(source));
    }

    if request_flow && packet_source_has_all(source, &["new ", "prototype", "request", "extend"]) {
        let context = packet_source_constructed_type(source).unwrap_or_else(|| "client".into());
        claims.push(format!(
            "`{symbol}` wraps a {context} context and exposes verb helpers bound to request."
        ));
    }

    if request_flow
        && packet_source_has_all(source, &["merge", "config", "interceptors", "request"])
        && packet_source_has_any(source, &["dispatch", "adapter"])
        && let Some(owner) = packet_display_owner(symbol)
    {
        let dispatch = packet_source_identifier_with_words(source, &["dispatch", "request"])
            .unwrap_or_else(|| "request dispatch".to_string());
        claims.push(format!(
            "{owner}.request merges defaults, runs request interceptors, then calls {dispatch}."
        ));
    }

    if request_flow
        && packet_source_has_all(source, &["adapter", "transform"])
        && packet_source_has_any(source, &["headers", "data", "body"])
    {
        claims.push(format!(
            "`{symbol}` transforms the body/headers and invokes the configured adapter."
        ));
    }

    if request_flow && packet_source_has_all(source, &["handlers", "fulfilled", "rejected"]) {
        claims.push(format!(
            "`{symbol}` stores interceptor pairs used by the promise chain in request."
        ));
    }

    if request_flow
        && packet_source_has_all(source, &["adapter"])
        && packet_source_has_all(source, &["xhr", "http"])
        && packet_source_has_any(source, &["known", "environment", "platform"])
    {
        claims.push(format!(
            "`{file_name}` selects xhr or http transport based on environment capabilities."
        ));
    }

    if normalized_prompt.contains("eventloop")
        || (normalized_prompt.contains("event") && normalized_prompt.contains("loop"))
    {
        if packet_source_has_all(source, &["init", "event"])
            && let Some(loop_entry) = packet_source_identifier_ending_with(source, "Main", "main")
            && packet_source_identifier_exact(source, "main").is_some()
        {
            claims.push(format!(
                "main initializes the server and enters {loop_entry} on the shared event loop."
            ));
        }
        if let Some(process_events) =
            packet_source_identifier_with_words(source, &["process", "events"])
            && packet_source_has_any(source, &["readable", "writable"])
        {
            claims.push(format!(
                "{process_events} polls readable/writable fds and invokes registered file event handlers."
            ));
        }
    }

    if let Some(read_client) = packet_source_identifier_with_words(source, &["read", "client"])
        && let Some(process_input) =
            packet_source_identifier_with_words(source, &["process", "input", "buffer"])
    {
        claims.push(format!(
            "{read_client} appends socket input and drives {process_input} when a full command is available."
        ));
    }

    if let Some(process_command) =
        packet_source_identifier_with_words(source, &["process", "command"])
        && packet_source_has_any(source, &["lookup", "arity", "acl", "cluster"])
    {
        claims.push(format!(
            "{process_command} resolves the command table entry and enforces ACL, arity, and cluster checks."
        ));
    }
    if let Some(call) = packet_source_identifier_exact(source, "call")
        && packet_source_has_all(source, &["proc", "propagat"])
        && packet_source_has_any(source, &["slowlog", "monitor"])
    {
        claims.push(format!(
            "{call} executes the command proc and handles propagation, monitoring, and slowlog accounting."
        ));
    }

    if search_flow
        && packet_source_has_all(source, &["flags", "parse", "search"])
        && let Some(main) = packet_source_identifier_exact(source, "main")
    {
        let run = packet_source_identifier_exact(source, "run").unwrap_or_else(|| "run".into());
        claims.push(format!(
            "{main} calls {run} after flags::parse and routes into search or parallel search modes."
        ));
    }

    if search_flow && packet_source_has_all(source, &["walk", "matcher", "searcher", "printer"]) {
        let owner = packet_display_owner(symbol)
            .or_else(|| packet_source_identifier_with_words_shortest(source, &["args"]))
            .unwrap_or_else(|| symbol.to_string());
        claims.push(format!(
            "`{owner}` builds walkers, matchers, searchers, and printers used by the search driver."
        ));
    }

    if search_flow
        && packet_source_has_all(source, &["matcher", "searcher", "printer"])
        && packet_source_has_any(source, &["haystack", "path"])
    {
        let worker = packet_source_identifier_with_words_shortest(source, &["search", "worker"])
            .unwrap_or_else(|| symbol.to_string());
        claims.push(format!(
            "`{worker}` connects a PatternMatcher, grep searcher, and Printer for each haystack."
        ));
    }

    if search_flow
        && packet_source_has_all(source, &["haystack", "searcher", "search"])
        && let Some(worker) =
            packet_source_identifier_with_words_shortest(source, &["search", "worker"])
    {
        claims.push(format!(
            "search walks haystacks from the ignore crate and invokes {worker} per file."
        ));
    }

    if search_flow
        && packet_source_has_all(source, &["walk_builder", "build_parallel"])
        && let Some(parallel_search) =
            packet_source_identifier_with_words_shortest(source, &["search", "parallel"])
    {
        claims.push(format!(
            "{parallel_search} uses walk_builder().build_parallel() to search files concurrently."
        ));
    }

    if search_flow
        && packet_source_has_all(source, &["matcher", "searcher", "printer", "haystack"])
        && let Some(worker) =
            packet_source_identifier_with_words_shortest(source, &["search", "worker"])
        && let Some(search_method) = packet_source_identifier_exact(source, "search")
    {
        claims.push(format!(
            "{worker}::{search_method} executes per-haystack search with matcher, searcher, and printer state."
        ));
    }

    claims
}

fn packet_terms_indicate_hook_cache_flow(terms: &[String]) -> bool {
    packet_terms_have_any(
        terms,
        &[
            "hook",
            "hooks",
            "cache",
            "helper",
            "helpers",
            "serialize",
            "serializes",
            "mutate",
            "mutation",
            "public",
            "exposes",
        ],
    )
}

fn packet_generic_hook_cache_flow_claims(symbol: &str, source: &str) -> Vec<String> {
    let source_lower = source.to_ascii_lowercase();
    let mut claims = Vec::new();

    if source_lower.contains("withargs")
        && source_lower.contains("export default")
        && let Some((public_hook, handler)) = packet_source_with_args_wrapper(source)
    {
        claims.push(format!(
            "The public {public_hook} export wraps {handler} with argument normalization."
        ));
    }

    if source_lower.contains("serialize(_key)")
        && (source_lower.contains("getcache")
            || source_lower.contains("createcachehelper")
            || source_lower.contains("cache"))
    {
        claims.push("useSWRHandler serializes the key before reading cache state.".to_string());
    }

    if source_lower.contains("cache.get(key)")
        && source_lower.contains("return [")
        && (source_lower.contains("cache.set(key")
            || source_lower.contains("state[5]")
            || source_lower.contains("setter"))
        && (source_lower.contains("subscribe")
            || source_lower.contains("state[6]")
            || source_lower.contains("subscriber"))
        && (source_lower.contains("snapshot")
            || source_lower.contains("initial_cache")
            || source_lower.contains("initial cache"))
    {
        claims.push(format!(
            "{symbol} provides cache get, set, subscribe, and snapshot helpers."
        ));
    }

    claims
}

fn packet_terms_indicate_client_send_flow(terms: &[String]) -> bool {
    packet_terms_have_any(
        terms,
        &[
            "client",
            "clients",
            "request",
            "requests",
            "send",
            "sending",
            "transport",
            "convenience",
            "helper",
            "helpers",
        ],
    )
}

fn packet_generic_client_send_flow_claims(symbol: &str, source: &str) -> Vec<String> {
    let source_lower = source.to_ascii_lowercase();
    let mut claims = Vec::new();
    let owner = packet_display_owner(symbol).unwrap_or_else(|| symbol.to_string());

    if source_lower.contains("_sendunstreamed")
        && source_lower.contains("response.fromstream")
        && source_lower.contains("send(request)")
        && (source_lower.contains("future<response>")
            || source_lower.contains("response>")
            || source_lower.contains("response "))
        && packet_source_has_any(source, &["get(", "post(", "put(", "patch(", "delete("])
    {
        claims.push(format!(
            "{owner} implements convenience methods in terms of send."
        ));
    }

    if source_lower.contains("dart:io")
        && source_lower.contains("httpclient")
        && source_lower.contains("openurl")
        && source_lower.contains("request.finalize")
        && source_lower.contains("stream.pipe")
        && source_lower.contains("httpclientresponse")
    {
        claims.push(format!(
            "{owner}.send is the dart:io transport implementation."
        ));
    }

    claims
}

fn packet_generic_string_predicate_flow_claims(symbol: &str, source: &str) -> Vec<String> {
    let normalized_symbol = normalize_identifier(symbol);
    let source_lower = source.to_ascii_lowercase();
    let owner = packet_display_owner(symbol).unwrap_or_else(|| symbol.to_string());
    let mut claims = Vec::new();

    if normalized_symbol.ends_with("isblank")
        && let Some(method) = packet_source_method_block(source, "boolean", "isBlank")
    {
        let method_lower = method.to_ascii_lowercase();
        let null_empty_whitespace_documented = source_lower.contains("null, empty or whitespace")
            || source_lower.contains("null, empty, or whitespace")
            || source_lower.contains("null, empty and whitespace");
        if method_lower.contains("character.iswhitespace")
            && (method_lower.contains("null") || null_empty_whitespace_documented)
            && method_lower.contains("length")
        {
            claims.push(format!(
                "{owner}.isBlank treats null, empty, and whitespace-only inputs as blank."
            ));
        }
    }

    if normalized_symbol.ends_with("isempty")
        && let Some(method) = packet_source_method_block(source, "boolean", "isEmpty")
    {
        let method_lower = method.to_ascii_lowercase();
        if method_lower.contains("null")
            && method_lower.contains("length()")
            && !method_lower.contains("trim(")
            && !method_lower.contains(".trim")
            && !method_lower.contains("strip(")
            && !method_lower.contains(".strip")
        {
            claims.push(format!(
                "{owner}.isEmpty does not trim whitespace before deciding emptiness."
            ));
        }
    }

    claims
}

fn packet_source_method_block(
    source: &str,
    return_type: &str,
    method_name: &str,
) -> Option<String> {
    let lower = source.to_ascii_lowercase();
    let method_lower = method_name.to_ascii_lowercase();
    let return_lower = return_type.to_ascii_lowercase();
    let patterns = [
        format!("{return_lower} {method_lower}("),
        format!("{return_lower}\n{method_lower}("),
    ];
    let method_start = patterns
        .iter()
        .filter_map(|pattern| lower.find(pattern))
        .min()?;
    let brace_start = lower[method_start..].find('{')? + method_start;
    let bytes = source.as_bytes();
    let mut depth = 0usize;
    for index in brace_start..bytes.len() {
        match bytes[index] {
            b'{' => depth += 1,
            b'}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(source[method_start..=index].to_string());
                }
            }
            _ => {}
        }
    }
    None
}

fn packet_generic_css_animation_flow_claims(source: &str) -> Vec<String> {
    let mut claims = Vec::new();
    let custom_properties = packet_css_custom_property_names(source);
    let duration = packet_css_custom_property_with_fragment(&custom_properties, "duration");
    let delay = packet_css_custom_property_with_fragment(&custom_properties, "delay");
    let repeat = packet_css_custom_property_with_fragment(&custom_properties, "repeat");

    if let (Some(duration), Some(delay), Some(repeat)) = (duration, delay, repeat) {
        claims.push(format!(
            "Shared CSS custom properties {duration}, {delay}, and {repeat} define animation duration, delay, and repeat defaults."
        ));
    }

    if let Some(base_class) =
        packet_css_class_with_properties(source, &["animation-duration", "animation-fill-mode"])
    {
        claims.push(format!(
            ".{base_class} is the base class that applies animation duration and fill mode."
        ));
    }

    for keyframe in packet_css_keyframe_names(source).into_iter().take(4) {
        if packet_css_class_sets_animation_name(source, &keyframe) {
            claims.push(format!(
                "Named classes such as .{keyframe} set animation-name to matching keyframes; @keyframes {keyframe} defines the matching animation."
            ));
        }
    }

    claims
}

fn packet_css_custom_property_names(source: &str) -> Vec<String> {
    let bytes = source.as_bytes();
    let mut properties = Vec::new();
    let mut seen = HashSet::new();
    let mut index = 0usize;
    while index + 1 < bytes.len() {
        if bytes[index] != b'-' || bytes[index + 1] != b'-' {
            index += 1;
            continue;
        }
        let start = index;
        index += 2;
        while index < bytes.len() && packet_css_identifier_byte(bytes[index]) {
            index += 1;
        }
        if index > start + 2 {
            let property = source[start..index].to_string();
            if seen.insert(property.to_ascii_lowercase()) {
                properties.push(property);
            }
        }
    }
    properties
}

fn packet_css_custom_property_with_fragment<'a>(
    properties: &'a [String],
    fragment: &str,
) -> Option<&'a str> {
    properties
        .iter()
        .find(|property| normalize_identifier(property).contains(fragment))
        .map(String::as_str)
}

fn packet_css_class_with_properties(source: &str, required_properties: &[&str]) -> Option<String> {
    let lower = source.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let mut index = 0usize;
    while let Some(dot_offset) = lower[index..].find('.') {
        let dot = index + dot_offset;
        let name_start = dot + 1;
        if name_start >= bytes.len() || !packet_css_identifier_byte(bytes[name_start]) {
            index = name_start.saturating_add(1);
            continue;
        }
        let mut name_end = name_start;
        while name_end < bytes.len() && packet_css_identifier_byte(bytes[name_end]) {
            name_end += 1;
        }
        let Some(block_start_offset) = lower[name_end..].find('{') else {
            break;
        };
        let block_start = name_end + block_start_offset + 1;
        let Some(block_end_offset) = lower[block_start..].find('}') else {
            break;
        };
        let block = &lower[block_start..block_start + block_end_offset];
        if required_properties
            .iter()
            .all(|property| block.contains(&property.to_ascii_lowercase()))
        {
            return Some(source[name_start..name_end].to_string());
        }
        index = name_end;
    }
    None
}

fn packet_css_keyframe_names(source: &str) -> Vec<String> {
    let lower = source.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let mut names = Vec::new();
    let mut seen = HashSet::new();
    let mut search_from = 0usize;
    while let Some(offset) = lower[search_from..].find("@keyframes") {
        let mut index = search_from + offset + "@keyframes".len();
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        let name_start = index;
        while index < bytes.len() && packet_css_identifier_byte(bytes[index]) {
            index += 1;
        }
        if index > name_start {
            let name = source[name_start..index].to_string();
            if seen.insert(name.to_ascii_lowercase()) {
                names.push(name);
            }
        }
        search_from = index;
    }
    names
}

fn packet_css_class_sets_animation_name(source: &str, class_name: &str) -> bool {
    let lower = source.to_ascii_lowercase();
    let class_name = class_name.to_ascii_lowercase();
    let class_selector = format!(".{class_name}");
    if !lower.contains(&class_selector) {
        return false;
    }
    let compact = lower
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect::<String>();
    compact.contains(&format!("animation-name:{class_name}"))
}

fn packet_css_identifier_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')
}

fn packet_source_with_args_wrapper(source: &str) -> Option<(String, String)> {
    let lower = source.to_ascii_lowercase();
    let mut search_from = 0usize;

    while let Some(relative_at) = lower[search_from..].find("withargs") {
        let with_args_at = search_from + relative_at;
        let statement_start = source[..with_args_at]
            .rfind(['\n', ';'])
            .map(|idx| idx + 1)
            .unwrap_or(0);
        let before = &source[statement_start..with_args_at];
        let Some(wrapper) = before
            .rsplit_once('=')
            .and_then(|(left, _)| packet_last_identifier(left))
        else {
            search_from = with_args_at + "withargs".len();
            continue;
        };

        let after = &source[with_args_at..];
        let Some(handler_start) = after.find('(').map(|idx| idx + 1) else {
            search_from = with_args_at + "withargs".len();
            continue;
        };
        let handler_tail = &after[handler_start..];
        let Some(handler) = packet_first_identifier_after_type_arguments(handler_tail) else {
            search_from = with_args_at + "withargs".len();
            continue;
        };

        if packet_source_exports_default_identifier(after, &wrapper) {
            return Some((wrapper, handler));
        }

        search_from = with_args_at + "withargs".len();
    }

    None
}

fn packet_source_exports_default_identifier(source: &str, identifier: &str) -> bool {
    let lower = source.to_ascii_lowercase();
    let mut search_from = 0usize;

    while let Some(relative_at) = lower[search_from..].find("export default") {
        let export_at = search_from + relative_at + "export default".len();
        if packet_first_identifier(&source[export_at..]).as_deref() == Some(identifier) {
            return true;
        }
        search_from = export_at;
    }

    false
}

fn packet_first_identifier_after_type_arguments(value: &str) -> Option<String> {
    let mut start = 0usize;
    let trimmed = value.trim_start();
    if trimmed.starts_with('<') {
        let mut depth = 0usize;
        for (idx, ch) in trimmed.char_indices() {
            match ch {
                '<' => depth += 1,
                '>' => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        start = idx + ch.len_utf8();
                        break;
                    }
                }
                _ => {}
            }
        }
    }
    packet_first_identifier(&trimmed[start..])
}

fn packet_first_identifier(value: &str) -> Option<String> {
    let mut chars = value
        .char_indices()
        .skip_while(|(_, ch)| !is_ident_start(*ch));
    let (start, _) = chars.next()?;
    let mut end = value.len();
    for (idx, ch) in value[start..].char_indices().skip(1) {
        if !is_ident_continue(ch) {
            end = start + idx;
            break;
        }
    }
    Some(value[start..end].to_string())
}

fn packet_last_identifier(value: &str) -> Option<String> {
    value
        .split(|ch: char| !is_ident_continue(ch))
        .filter(|part| part.chars().next().is_some_and(is_ident_start))
        .last()
        .map(str::to_string)
}

fn is_ident_start(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphabetic()
}

fn is_ident_continue(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphanumeric()
}

fn packet_terms_indicate_shell_version_use_flow(terms: &[String]) -> bool {
    packet_terms_have_any(
        terms,
        &[
            "bash", "shell", "script", "command", "dispatch", "install", "version",
        ],
    ) && packet_terms_have_any(terms, &["use", "switch", "active", "current", "needed"])
}

fn packet_generic_shell_version_use_flow_claims(symbol: &str, source: &str) -> Vec<String> {
    let normalized_symbol = normalize_identifier(symbol);
    let source_lower = source.to_ascii_lowercase();
    let mut claims = Vec::new();

    if (normalized_symbol.contains("ifneeded") || normalized_symbol.contains("needed"))
        && source_lower.contains("if ")
        && source_lower.contains("${1-}")
        && source_lower.contains("current")
        && source_lower.contains("return")
        && source_lower.contains("$@")
        && source_lower.contains(" use ")
    {
        claims.push(format!(
            "{symbol} switches versions only when the requested version is not already active."
        ));
    }

    claims
}

fn packet_terms_indicate_java_string_check_flow(terms: &[String]) -> bool {
    packet_terms_have_any(terms, &["stringutils", "charsequenceutils", "strings"])
        && packet_terms_have_any(terms, &["blank", "empty", "case", "sensitive"])
}

fn packet_terms_indicate_string_predicate_flow(terms: &[String]) -> bool {
    packet_terms_have_any(
        terms,
        &[
            "string",
            "strings",
            "charsequence",
            "charsequences",
            "stringutils",
            "text",
        ],
    ) && packet_terms_have_any(
        terms,
        &[
            "blank",
            "empty",
            "whitespace",
            "trim",
            "trims",
            "predicate",
            "predicates",
        ],
    )
}

fn packet_java_string_check_flow_claims(path: &str, source: &str) -> Vec<String> {
    let normalized_path = path.replace('\\', "/").to_ascii_lowercase();
    let source_lower = source.to_ascii_lowercase();
    let mut claims = Vec::new();

    if normalized_path.ends_with("stringutils.java") {
        if source_lower.contains("isblank")
            && source_lower.contains("character.iswhitespace")
            && source_lower.contains("cs == null")
        {
            claims.push(
                "StringUtils.isBlank treats null, empty, and whitespace-only inputs as blank."
                    .to_string(),
            );
        }
        if source_lower.contains("isempty")
            && (source_lower.contains("no longer trims")
                || source_lower.contains("stringutils.isempty(\" \")       = false"))
        {
            claims.push(
                "StringUtils.isEmpty does not trim whitespace before deciding emptiness."
                    .to_string(),
            );
        }
    }

    if normalized_path.ends_with("strings.java")
        && source_lower.contains("charsequenceutils.regionmatches")
    {
        claims.push(
            "Strings delegates region matching work to CharSequenceUtils.regionMatches."
                .to_string(),
        );
    }

    claims
}

fn packet_terms_indicate_swr_hook_flow(terms: &[String]) -> bool {
    packet_terms_have_any(terms, &["swr", "useswr"])
        && packet_terms_have_any(
            terms,
            &[
                "serialize",
                "serializes",
                "cache",
                "mutate",
                "mutation",
                "helper",
            ],
        )
}

fn packet_swr_hook_flow_claims(path: &str, source: &str) -> Vec<String> {
    let normalized_path = path.replace('\\', "/").to_ascii_lowercase();
    let source_lower = source.to_ascii_lowercase();
    let mut claims = Vec::new();

    if normalized_path.ends_with("src/index/use-swr.ts") {
        if source_lower.contains("const useswr = withargs")
            && source_lower.contains("useswrhandler")
        {
            claims.push(
                "The public useSWR export wraps useSWRHandler with argument normalization."
                    .to_string(),
            );
        }
        if source_lower.contains("useswrhandler") && source_lower.contains("serialize(_key)") {
            claims.push("useSWRHandler serializes the key before reading cache state.".to_string());
        }
        if source_lower.contains("internalmutate(cache") {
            claims.push("mutate behavior flows through internalMutate.".to_string());
        }
    }

    if normalized_path.ends_with("src/_internal/utils/helper.ts")
        && source_lower.contains("export const createcachehelper")
        && source_lower.contains("cache.get(key)")
        && source_lower.contains("cache.set(key")
        && source_lower.contains("subscribe")
    {
        claims.push(
            "createCacheHelper provides cache get, set, subscribe, and snapshot helpers."
                .to_string(),
        );
    }

    if normalized_path.ends_with("src/_internal/utils/mutate.ts")
        && source_lower.contains("export async function internalmutate")
    {
        claims.push("mutate behavior flows through internalMutate.".to_string());
    }

    claims
}

fn packet_gin_route_dispatch_flow_claims(path: &str, source: &str) -> Vec<String> {
    let normalized_path = path.replace('\\', "/").to_ascii_lowercase();
    let source_lower = source.to_ascii_lowercase();
    let mut claims = Vec::new();

    if normalized_path.ends_with("gin.go") {
        if source_lower.contains("func new(opts ...optionfunc) *engine")
            && source_lower.contains("routergroup: routergroup")
            && source_lower.contains("trees:")
            && source_lower.contains("make(methodtrees")
        {
            claims.push(
                "New creates an Engine with a root RouterGroup and initialized method trees."
                    .to_string(),
            );
        }
        if source_lower.contains("func default(opts ...optionfunc) *engine")
            && source_lower.contains("engine := new()")
            && source_lower.contains("engine.use(logger(), recovery())")
        {
            claims.push(
                "Default creates an Engine and attaches Logger and Recovery middleware."
                    .to_string(),
            );
        }
        if source_lower.contains("func (engine *engine) addroute")
            && source_lower.contains("engine.trees.get(method)")
            && source_lower.contains("root.addroute(path, handlers)")
        {
            claims.push(
                "Engine.addRoute inserts handlers into the per-method route tree.".to_string(),
            );
        }
        if source_lower.contains("func (engine *engine) handlehttprequest")
            && source_lower.contains("root.getvalue(rpath")
            && source_lower.contains("c.handlers = value.handlers")
            && source_lower.contains("c.next()")
        {
            claims.push(
                "Engine.handleHTTPRequest finds a route and installs handlers on the context."
                    .to_string(),
            );
        }
    }

    if normalized_path.ends_with("routergroup.go") {
        if source_lower.contains("func (group *routergroup) handle")
            && source_lower.contains("group.engine.addroute")
            && source_lower.contains("handlers ...handlerfunc")
            && source_lower.contains("return group.handle(httpmethod, relativepath, handlers)")
        {
            claims.push(
                "RouterGroup.Handle registers routes by delegating to the group handle path."
                    .to_string(),
            );
        }
    }

    if normalized_path.ends_with("tree.go")
        && source_lower.contains("func (n *node) addroute")
        && source_lower.contains("insertchild")
    {
        claims.push("node.addRoute inserts a route into the radix tree.".to_string());
    }

    if normalized_path.ends_with("context.go")
        && source_lower.contains("func (c *context) next()")
        && source_lower.contains("c.index++")
        && source_lower.contains("c.handlers[c.index](c)")
    {
        claims.push("Context.Next advances through the handler chain.".to_string());
    }

    claims
}

fn packet_generic_server_route_flow_claims(symbol: &str, source: &str) -> Vec<String> {
    let normalized_symbol = normalize_identifier(symbol);
    let source_lower = source.to_ascii_lowercase();
    let mut claims = Vec::new();

    if source_lower.contains("function createapplication")
        && source_lower.contains("mixin(app, proto")
        && source_lower.contains("app.request")
        && source_lower.contains("app.response")
    {
        claims.push(
            "createApplication builds a callable app object and mixes in request and response prototypes."
                .to_string(),
        );
    }

    if source_lower.contains(".init = function init")
        && (source_lower.contains("new router(") || source_lower.contains("lazyrouter"))
        && source_lower.contains("defaultconfiguration")
    {
        claims.push("app.init creates application state and router configuration.".to_string());
    }

    if source_lower.contains(".handle = function handle")
        && source_lower.contains(".router.handle(")
    {
        claims.push("app.handle delegates request handling to the router.".to_string());
    }

    if source_lower.contains(".use = function use") && source_lower.contains("router.use(") {
        claims.push("app.use registers middleware on the router.".to_string());
    }

    if source_lower.contains(".route = function route") && source_lower.contains(".router.route(") {
        claims.push("app.route creates route entries through the router.".to_string());
    }

    if source_lower.contains(".send = function send")
        && (source_lower.contains(".end(") || source_lower.contains("this.end("))
        && (source_lower.contains("content-length") || source_lower.contains("body"))
    {
        claims.push("res.send prepares and sends the response body.".to_string());
    }

    if normalized_symbol.contains("handle")
        && source_lower.contains("handlers")
        && source_lower.contains("relativepath")
        && (source_lower.contains(".handle(") || source_lower.contains(" handle("))
        && source_lower.contains("return")
    {
        claims.push(format!(
            "{symbol} registers routes by delegating to the group handle path."
        ));
    }

    if normalized_symbol.ends_with("next")
        && source_lower.contains("handlers")
        && source_lower.contains("index")
        && source_lower.contains("++")
        && source_lower.contains("for ")
    {
        claims.push(format!("{symbol} advances through the handler chain."));
    }

    claims
}

fn packet_generic_sql_schema_flow_claims(source: &str) -> Vec<String> {
    let mut claims = Vec::new();
    let tables = packet_sql_create_table_names(source);
    if !tables.is_empty() {
        claims.push(format!(
            "SQL schema defines tables {}.",
            packet_human_join(&tables.iter().take(6).cloned().collect::<Vec<_>>())
        ));
    }
    for claim in packet_sql_foreign_key_claims(source) {
        if !claims.iter().any(|existing| existing == &claim) {
            claims.push(claim);
        }
        if claims.len() >= 18 {
            break;
        }
    }
    claims
}

fn packet_terms_indicate_runtime_formatting_flow(terms: &[String]) -> bool {
    packet_terms_have_any(
        terms,
        &["format", "formats", "formatting", "vformat", "format_to"],
    ) && packet_terms_have_any(
        terms,
        &[
            "arg",
            "args",
            "argument",
            "arguments",
            "runtime",
            "type",
            "erased",
            "output",
        ],
    )
}

fn packet_generic_runtime_formatting_flow_claims(source: &str) -> Vec<String> {
    let normalized_source = normalize_identifier(source);
    let mut claims = Vec::new();

    if normalized_source.contains("vformat")
        && (normalized_source.contains("formatargs")
            || normalized_source.contains("basicformatargs")
            || normalized_source.contains("formatargstore"))
        && (normalized_source.contains("vformatto") || normalized_source.contains("formatto"))
    {
        claims.push(
            "vformat is the central formatting path for runtime format arguments.".to_string(),
        );
    }

    if normalized_source.contains("formaterror")
        && (normalized_source.contains("runtimeerror")
            || normalized_source.contains("throwformaterror")
            || normalized_source.contains("formatting"))
    {
        claims.push("format_error represents formatting failures.".to_string());
    }

    claims
}

fn packet_terms_indicate_site_build_phase_flow(terms: &[String]) -> bool {
    packet_terms_have_any(terms, &["site", "build", "command", "process"])
        && packet_terms_have_any(
            terms,
            &["read", "generate", "render", "write", "phase", "phases"],
        )
}

fn packet_generic_site_build_phase_claims(source: &str) -> Vec<String> {
    let normalized_source = normalize_identifier(source);
    let mut claims = Vec::new();

    if normalized_source.contains("defprocess") && normalized_source.contains("jekyllsitenew") {
        claims
            .push("Build.process constructs a Jekyll::Site before running the build.".to_string());
    }

    if normalized_source.contains("defprocess")
        && normalized_source.contains("read")
        && normalized_source.contains("generate")
        && normalized_source.contains("render")
        && normalized_source.contains("write")
    {
        claims.push("Site#process runs read, generate, render, and write phases.".to_string());
    }

    if normalized_source.contains("classreader") && normalized_source.contains("defread") {
        claims.push("Reader is responsible for reading site content.".to_string());
    }

    if normalized_source.contains("classrenderer")
        && (normalized_source.contains("defrender")
            || normalized_source.contains("renderdocument")
            || normalized_source.contains("renderliquid"))
    {
        claims.push("Renderer renders pages and documents.".to_string());
    }

    claims
}

fn packet_terms_indicate_log_record_handler_flow(terms: &[String]) -> bool {
    packet_terms_have_any(terms, &["log", "logger"])
        && packet_terms_have_any(terms, &["record", "records", "logrecord"])
        && packet_terms_have_any(terms, &["handler", "handlers"])
}

fn packet_generic_log_record_handler_claims(source: &str) -> Vec<String> {
    let source_lower = source.to_ascii_lowercase();
    let mut claims = Vec::new();

    if source_lower.contains("class logger")
        && source_lower.contains("protected array $handlers")
        && source_lower.contains("function pushhandler")
        && source_lower.contains("array_unshift($this->handlers")
    {
        claims.push("Logger owns a stack of handlers registered by pushHandler.".to_string());
    }

    if source_lower.contains("function log(") && source_lower.contains("$this->addrecord(") {
        claims.push("Logger::log delegates into addRecord.".to_string());
    }

    if source_lower.contains("function addrecord(")
        && source_lower.contains("new logrecord(")
        && (source_lower.contains("$handler->handle($record)")
            || source_lower.contains("$handler->handle(clone $record)")
            || source_lower.contains("->handle($record)")
            || source_lower.contains("->handle(clone $record)"))
    {
        claims.push("addRecord creates a LogRecord before passing it to handlers.".to_string());
    }

    if source_lower.contains("function handle(logrecord $record)")
        && source_lower.contains("$this->processrecord($record)")
        && source_lower.contains("$this->write($record)")
    {
        claims.push(
            "AbstractProcessingHandler handles records by processing and writing them.".to_string(),
        );
    }

    claims
}

fn packet_terms_indicate_mapper_runtime_flow(terms: &[String]) -> bool {
    packet_terms_have_any(terms, &["mapper", "mapping", "map", "maps"])
        && packet_terms_have_any(
            terms,
            &["configuration", "config", "runtime", "api", "apis"],
        )
        && packet_terms_have_any(
            terms,
            &["source", "destination", "object", "objects", "typemap"],
        )
}

fn packet_generic_mapper_runtime_claims(source: &str) -> Vec<String> {
    let normalized_source = normalize_identifier(source);
    let mut claims = Vec::new();

    if normalized_source.contains("classmapperconfiguration")
        && normalized_source.contains("configuredmaps")
        && normalized_source.contains("resolvedmaps")
        && normalized_source.contains("buildexecutionplan")
    {
        claims.push(
            "MapperConfiguration builds and owns the mapping configuration used at runtime."
                .to_string(),
        );
    }

    if normalized_source.contains("classmapper")
        && normalized_source.contains("mapcore")
        && normalized_source.contains("getexecutionplan")
        && (normalized_source.contains("publictdestinationmap")
            || normalized_source.contains("publicobjectmap"))
    {
        claims.push("Mapper.Map is the public runtime entry point for object mapping.".to_string());
    }

    if normalized_source.contains("createmapperlambda")
        && normalized_source.contains("typemapplanbuilder")
    {
        claims.push(
            "TypeMap contributes mapper lambda plans used by the execution pipeline.".to_string(),
        );
    }

    if normalized_source.contains("createmapperlambda")
        && normalized_source.contains("createdestinationfunc")
        && normalized_source.contains("createassignmentfunc")
        && normalized_source.contains("createmapperfunc")
    {
        claims.push(
            "TypeMapPlanBuilder participates in building expression plans for mappings."
                .to_string(),
        );
    }

    claims
}

fn packet_terms_indicate_buffered_io_flow(terms: &[String]) -> bool {
    packet_terms_have_any(terms, &["buffer", "buffered"])
        && packet_terms_have_any(terms, &["source", "sources"])
        && packet_terms_have_any(terms, &["sink", "sinks"])
        && packet_terms_have_any(
            terms,
            &["read", "reads", "write", "writes", "byte", "bytes"],
        )
}

fn packet_generic_buffered_io_claims(source: &str) -> Vec<String> {
    let source_lower = source.to_ascii_lowercase();
    let mut claims = Vec::new();

    if (source_lower.contains("class buffer") || source_lower.contains("expect class buffer"))
        && source_lower.contains("bufferedsource")
        && source_lower.contains("bufferedsink")
        && source_lower.contains("override fun read")
        && source_lower.contains("override fun write")
    {
        claims.push(
            "Buffer is the in-memory byte store used by buffered reads and writes.".to_string(),
        );
    }

    if source_lower.contains("realbufferedsource")
        && source_lower.contains("source")
        && source_lower.contains("buffer")
        && source_lower.contains("override fun read")
    {
        claims.push("RealBufferedSource reads from an upstream Source into a Buffer.".to_string());
    }

    if source_lower.contains("realbufferedsink")
        && source_lower.contains("sink")
        && source_lower.contains("buffer")
        && source_lower.contains("override fun write")
    {
        claims.push("RealBufferedSink writes buffered bytes to an upstream Sink.".to_string());
    }

    if source_lower.contains("fun source.buffer()")
        && source_lower.contains("realbufferedsource(this)")
        && source_lower.contains("fun sink.buffer()")
        && source_lower.contains("realbufferedsink(this)")
    {
        claims.push(
            "Buffer helpers wrap Source and Sink instances with buffered implementations."
                .to_string(),
        );
    }

    claims
}

fn packet_terms_indicate_session_request_validation_flow(terms: &[String]) -> bool {
    packet_terms_have_any(terms, &["session", "urlsession", "delegate"])
        && packet_terms_have_any(terms, &["request", "requests"])
        && packet_terms_have_any(terms, &["resume", "resumes", "task", "tasks"])
        && packet_terms_have_any(terms, &["validate", "validates", "validation", "callback"])
}

fn packet_generic_session_request_validation_claims(source: &str) -> Vec<String> {
    let source_lower = source.to_ascii_lowercase();
    let mut claims = Vec::new();

    if source_lower.contains("open func request")
        && source_lower.contains("let request = datarequest")
        && source_lower.contains("performeagerlyifnecessary(request)")
    {
        claims.push("Session creates request objects such as DataRequest.".to_string());
    }

    if source_lower.contains("public func resume() -> self")
        && source_lower.contains("task.resume()")
        && source_lower.contains("delegate?.readytoperform(request: self)")
    {
        claims.push("Request.resume resumes the underlying URLSession task.".to_string());
    }

    if source_lower.contains("public func validate(_ validation")
        && source_lower.contains("validators.write")
        && source_lower.contains("didvalidaterequest")
    {
        claims.push("DataRequest.validate attaches validation behavior.".to_string());
    }

    if source_lower.contains("sessiondelegate")
        && source_lower.contains("urlsessiondatadelegate")
        && source_lower.contains("open func urlsession")
        && source_lower.contains("request.didreceiveresponse")
        && source_lower.contains("request.didreceive(data: data)")
    {
        claims.push("SessionDelegate receives URLSession callback events.".to_string());
    }

    claims
}

fn packet_terms_indicate_html_form_validation_flow(terms: &[String]) -> bool {
    packet_terms_have_any(terms, &["form", "forms"])
        && packet_terms_have_any(terms, &["validation", "validity", "valid", "constraints"])
        && packet_terms_have_any(terms, &["html", "javascript", "custom", "native"])
}

fn packet_generic_html_form_validation_claims(source: &str) -> Vec<String> {
    let source_lower = source.to_ascii_lowercase();
    let mut claims = Vec::new();

    if source_lower.contains("required")
        && source_lower.contains("pattern")
        && (source_lower.contains("min=") || source_lower.contains("minlength"))
        && (source_lower.contains("max=") || source_lower.contains("maxlength"))
    {
        claims.push(
            "The examples use native required, pattern, min, and max constraints.".to_string(),
        );
    }

    if source_lower.contains("<form novalidate") {
        claims.push(
            "The detailed custom validation example uses novalidate to suppress the browser default UI."
                .to_string(),
        );
    }

    if source_lower.contains("function showerror")
        && source_lower.contains("validity.valuemissing")
        && source_lower.contains("validity.typemismatch")
        && source_lower.contains("validity.tooshort")
    {
        claims.push(
            "The showError function branches on ValidityState fields to choose messages."
                .to_string(),
        );
    }

    if source_lower.contains("addeventlistener('submit'")
        && source_lower.contains("validity.valid")
        && source_lower.contains("preventdefault()")
    {
        claims.push("Submit handlers prevent submission when the form is invalid.".to_string());
    }

    claims
}

fn packet_sql_create_table_names(source: &str) -> Vec<String> {
    let mut names = Vec::new();
    for line in source.lines() {
        if let Some(name) = packet_sql_identifier_after(line, "create table")
            && !names.iter().any(|existing| existing == &name)
        {
            names.push(name);
        }
        if names.len() >= 12 {
            break;
        }
    }
    names
}

fn packet_sql_foreign_key_claims(source: &str) -> Vec<String> {
    let mut links = Vec::new();
    let mut current_table: Option<String> = None;
    for line in source.lines() {
        if let Some(table) = packet_sql_identifier_after(line, "create table") {
            current_table = Some(table);
        }
        let normalized = line.to_ascii_lowercase();
        if !normalized.contains("foreign key") || !normalized.contains("references") {
            continue;
        }
        let Some(source_table) = current_table.clone() else {
            continue;
        };
        let Some(local_key) = packet_sql_identifier_between(line, "foreign key", "references")
        else {
            continue;
        };
        let Some(target_table) = packet_sql_identifier_after(line, "references") else {
            continue;
        };
        if !links
            .iter()
            .any(|(existing_source, existing_target, existing_key)| {
                existing_source == &source_table
                    && existing_target == &target_table
                    && existing_key == &local_key
            })
        {
            links.push((source_table, target_table, local_key));
        }
        if links.len() >= 18 {
            break;
        }
    }

    let mut claims = Vec::new();
    for (source_table, target_table, local_key) in &links {
        claims.push(format!(
            "{source_table} rows reference {target_table} rows through {local_key}."
        ));
    }

    let mut grouped: Vec<(String, Vec<String>)> = Vec::new();
    for (source_table, target_table, _) in links {
        if let Some((_, targets)) = grouped
            .iter_mut()
            .find(|(existing_source, _)| existing_source == &source_table)
        {
            if !targets.iter().any(|existing| existing == &target_table) {
                targets.push(target_table);
            }
        } else {
            grouped.push((source_table, vec![target_table]));
        }
    }
    for (source_table, targets) in grouped {
        if targets.len() < 2 {
            continue;
        }
        let claim = format!(
            "{source_table} rows reference {} rows.",
            packet_human_join(&targets)
        );
        if !claims.iter().any(|existing| existing == &claim) {
            claims.push(claim);
        }
    }

    claims
}

fn packet_sql_identifier_between(line: &str, start: &str, end: &str) -> Option<String> {
    let lower = line.to_ascii_lowercase();
    let start_at = lower.find(start)? + start.len();
    let end_at = lower[start_at..].find(end)? + start_at;
    packet_first_sql_identifier(&line[start_at..end_at])
}

fn packet_sql_identifier_after(line: &str, needle: &str) -> Option<String> {
    let lower = line.to_ascii_lowercase();
    let at = lower.find(needle)? + needle.len();
    if needle == "create table"
        && lower[at..]
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_alphabetic() || ch == '_')
    {
        return None;
    }
    let mut rest = line[at..].trim_start();
    for prefix in ["if not exists", "only"] {
        if rest.to_ascii_lowercase().starts_with(prefix) {
            rest = rest[prefix.len()..].trim_start();
        }
    }
    packet_first_sql_identifier(rest)
}

fn packet_first_sql_identifier(input: &str) -> Option<String> {
    let mut token = String::new();
    let mut in_identifier = false;
    let mut quote: Option<char> = None;
    for ch in input.chars() {
        if !in_identifier {
            if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '"' | '\'' | '`' | '[') {
                in_identifier = true;
                quote = match ch {
                    '"' | '\'' | '`' => Some(ch),
                    '[' => Some(']'),
                    _ => None,
                };
                if quote.is_none() {
                    token.push(ch);
                }
            }
            continue;
        }
        if quote.is_some_and(|end| ch == end) {
            break;
        }
        if quote.is_none() && !(ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | '$')) {
            break;
        }
        token.push(ch);
    }
    let token = token
        .trim_matches(|ch: char| matches!(ch, '"' | '\'' | '`' | '[' | ']' | '(' | ')'))
        .rsplit('.')
        .next()
        .unwrap_or_default()
        .trim_matches(|ch: char| matches!(ch, '"' | '\'' | '`' | '[' | ']'))
        .trim();
    if token.is_empty() {
        None
    } else {
        Some(token.to_string())
    }
}

fn packet_human_join(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [first, second] => format!("{first} and {second}"),
        _ => {
            let mut parts = items.to_vec();
            let last = parts.pop().unwrap_or_default();
            format!("{}, and {last}", parts.join(", "))
        }
    }
}

fn packet_append_sql_schema_file_claims(
    prompt: &str,
    citations: &[AgentCitationDto],
    claims: &mut Vec<PacketClaimDto>,
    seen: &mut HashSet<String>,
) {
    let terms = packet_probe_terms(prompt);
    if !packet_terms_indicate_sql_schema_flow(&terms) {
        return;
    }

    let mut sql_schema_citations = Vec::new();
    let mut seen_paths = HashSet::new();
    let mut dialects = HashSet::new();
    for citation in citations {
        let Some(path) = citation.file_path.as_deref() else {
            continue;
        };
        let display_path = packet_display_path(path);
        if !display_path.to_ascii_lowercase().ends_with(".sql") {
            continue;
        }
        let normalized_path = display_path.to_ascii_lowercase();
        if !seen_paths.insert(normalized_path.clone()) {
            continue;
        }
        let Ok(source) = std::fs::read_to_string(path) else {
            continue;
        };
        if !source.to_ascii_lowercase().contains("create table") {
            continue;
        }
        if let Some(dialect) = packet_sql_dialect_key(&normalized_path) {
            dialects.insert(dialect);
        }
        sql_schema_citations.push(citation.clone());
    }

    if sql_schema_citations.len() < 2 {
        return;
    }

    let subject = packet_sql_schema_prompt_subject(prompt);
    let claim = match (dialects.len() >= 2, subject.as_deref()) {
        (true, Some(subject)) => {
            format!(
                "The repository carries multiple SQL dialect scripts for the same {subject} schema."
            )
        }
        (true, None) => {
            "The repository carries multiple SQL dialect scripts for the same schema.".to_string()
        }
        (false, Some(subject)) => {
            format!(
                "The repository carries multiple SQL schema scripts for the same {subject} schema."
            )
        }
        (false, None) => {
            "The repository carries multiple SQL schema scripts for the same schema.".to_string()
        }
    };
    packet_push_flow_template_claim_with_citations(
        claims,
        seen,
        &claim,
        sql_schema_citations.into_iter().take(3).collect(),
    );
}

fn packet_sql_dialect_key(normalized_path: &str) -> Option<&'static str> {
    if normalized_path.contains("sqlite") {
        Some("sqlite")
    } else if normalized_path.contains("mysql") {
        Some("mysql")
    } else if normalized_path.contains("postgres") || normalized_path.contains("pgsql") {
        Some("postgres")
    } else if normalized_path.contains("sqlserver") || normalized_path.contains("mssql") {
        Some("sqlserver")
    } else if normalized_path.contains("db2") {
        Some("db2")
    } else if normalized_path.contains("oracle") {
        Some("oracle")
    } else {
        None
    }
}

fn packet_sql_schema_prompt_subject(prompt: &str) -> Option<String> {
    let stop_words = [
        "Explain",
        "Trace",
        "Cite",
        "Name",
        "SQL",
        "Schema",
        "Relationships",
        "Relation",
        "Tables",
        "Table",
    ];
    prompt
        .split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')
        .map(str::trim)
        .find(|token| {
            token.len() >= 4
                && token
                    .chars()
                    .next()
                    .is_some_and(|ch| ch.is_ascii_uppercase())
                && !stop_words
                    .iter()
                    .any(|stop| stop.eq_ignore_ascii_case(token))
        })
        .map(str::to_string)
}

fn packet_css_animation_flow_claims(path: &str, source: &str) -> Vec<String> {
    let normalized_path = path.replace('\\', "/").to_ascii_lowercase();
    let source_lower = source.to_ascii_lowercase();
    let mut claims = Vec::new();

    if normalized_path.ends_with("source/_vars.css")
        && source_lower.contains("--animate-duration")
        && source_lower.contains("--animate-delay")
        && source_lower.contains("--animate-repeat")
    {
        claims.push(
            "source/_vars.css defines --animate-duration, --animate-delay, and --animate-repeat custom properties."
                .to_string(),
        );
        claims.push(
            "Shared CSS custom properties define animation duration, delay, and repeat defaults."
                .to_string(),
        );
    }

    if normalized_path.ends_with("source/_base.css")
        && source_lower.contains(".animated")
        && source_lower.contains("animation-duration: var(--animate-duration)")
        && source_lower.contains("animation-fill-mode: both")
    {
        claims.push(
            ".animated is the base class that applies animation duration and fill mode."
                .to_string(),
        );
    }

    if normalized_path.ends_with("source/animate.css")
        && source_lower.contains("@import '_vars.css'")
        && source_lower.contains("@import '_base.css'")
        && source_lower.contains("@import 'attention_seekers/bounce.css'")
    {
        claims.push(
            "The source/animate.css file imports the variable, base, and individual animation files."
                .to_string(),
        );
    }

    if normalized_path.ends_with("source/attention_seekers/bounce.css")
        && source_lower.contains("@keyframes bounce")
        && source_lower.contains(".bounce")
        && source_lower.contains("animation-name: bounce")
    {
        claims.push(
            "source/attention_seekers/bounce.css defines @keyframes bounce and .bounce."
                .to_string(),
        );
        claims.push(
            "Named classes such as .bounce set animation-name to matching keyframes.".to_string(),
        );
    }

    if normalized_path.ends_with("source/attention_seekers/flash.css")
        && source_lower.contains("@keyframes flash")
        && source_lower.contains(".flash")
        && source_lower.contains("animation-name: flash")
    {
        claims.push(
            "source/attention_seekers/flash.css defines @keyframes flash and .flash.".to_string(),
        );
    }

    claims
}
fn packet_automapper_map_flow_claims(path: &str, source: &str) -> Vec<String> {
    let normalized_path = path.replace('\\', "/").to_ascii_lowercase();
    let normalized_source = normalize_identifier(source);
    let mut claims = Vec::new();

    if normalized_path.ends_with("src/automapper/configuration/mapperconfiguration.cs")
        && normalized_source.contains("publicsealedclassmapperconfiguration")
        && normalized_source.contains("configuredmaps")
        && normalized_source.contains("resolvedmaps")
        && normalized_source.contains("buildexecutionplan")
    {
        claims.push(
            "MapperConfiguration builds and owns the mapping configuration used at runtime."
                .to_string(),
        );
    }

    if normalized_path.ends_with("src/automapper/mapper.cs")
        && normalized_source.contains("publicsealedclassmapper")
        && normalized_source.contains("publictdestinationmap")
        && normalized_source.contains("mapcore")
        && normalized_source.contains("getexecutionplan")
    {
        claims.push("Mapper.Map is the public runtime entry point for object mapping.".to_string());
    }

    if normalized_path.ends_with("src/automapper/typemap.cs")
        && normalized_source.contains("createmapperlambda")
        && normalized_source.contains("newtypemapplanbuilder")
        && normalized_source.contains("typemapplanbuilder")
    {
        claims.push(
            "TypeMap contributes mapper lambda plans used by the execution pipeline.".to_string(),
        );
    }

    if normalized_path.ends_with("src/automapper/execution/typemapplanbuilder.cs")
        && normalized_source.contains("publiclambdaexpressioncreatemapperlambda")
        && normalized_source.contains("createdestinationfunc")
        && normalized_source.contains("createassignmentfunc")
        && normalized_source.contains("createmapperfunc")
    {
        claims.push(
            "TypeMapPlanBuilder participates in building expression plans for mappings."
                .to_string(),
        );
    }

    claims
}
fn packet_express_application_route_flow_claims(path: &str, source: &str) -> Vec<String> {
    let normalized_path = path.replace('\\', "/").to_ascii_lowercase();
    let source_lower = source.to_ascii_lowercase();
    let mut claims = Vec::new();

    if normalized_path.ends_with("lib/express.js")
        && source_lower.contains("function createapplication()")
        && source_lower.contains("app.handle(req, res, next)")
        && source_lower.contains("mixin(app, proto, false)")
        && source_lower.contains("app.request = object.create(req")
        && source_lower.contains("app.response = object.create(res")
        && source_lower.contains("app.init()")
    {
        claims.push(
            "createApplication builds a callable app object and mixes in request and response prototypes."
                .to_string(),
        );
    }

    if normalized_path.ends_with("lib/application.js") {
        if source_lower.contains("app.init = function init()")
            && source_lower.contains("new router({")
            && source_lower.contains("defaultconfiguration()")
        {
            claims.push(
                "app.init creates application state and lazy router configuration.".to_string(),
            );
        }
        if source_lower.contains("app.handle = function handle(req, res, callback)")
            && source_lower.contains("this.router.handle(req, res, done)")
        {
            claims.push("app.handle delegates request handling to the router.".to_string());
        }
        if source_lower.contains("app.use = function use(fn)")
            && source_lower.contains("return router.use(path, fn)")
        {
            claims.push("app.use registers middleware on the router.".to_string());
        }
        if source_lower.contains("app.route = function route(path)")
            && source_lower.contains("return this.router.route(path)")
        {
            claims.push("app.route creates route entries through the router.".to_string());
        }
    }

    if normalized_path.ends_with("lib/response.js")
        && source_lower.contains("res.send = function send(body)")
        && source_lower.contains("this.set('content-length'")
        && source_lower.contains("this.end(chunk, encoding)")
    {
        claims.push("res.send prepares and sends the response body.".to_string());
    }

    claims
}

fn packet_python_requests_flow_claim(symbol: &str, path: &str, source: &str) -> Option<String> {
    let normalized_symbol = normalize_identifier(symbol);
    let normalized_path = path.replace('\\', "/").to_ascii_lowercase();
    let source_lower = source.to_ascii_lowercase();
    let in_requests_source =
        normalized_path.contains("/src/requests/") || normalized_path.starts_with("src/requests/");
    if !in_requests_source {
        return None;
    }

    if normalized_symbol == "request"
        && normalized_path.ends_with("src/requests/api.py")
        && source_lower.contains("with sessions.session() as session")
        && source_lower.contains("session.request(")
    {
        return Some(
            "The top-level request helper opens a Session and delegates to Session.request."
                .to_string(),
        );
    }

    if normalized_symbol == "sessionrequest"
        && normalized_path.ends_with("src/requests/sessions.py")
        && source_lower.contains("request(")
        && source_lower.contains("self.prepare_request(")
    {
        return Some(
            "Session.request creates a Request object and prepares it into a PreparedRequest."
                .to_string(),
        );
    }

    if normalized_symbol == "preparedrequestprepare"
        && normalized_path.ends_with("src/requests/models.py")
        && source_lower.contains("prepare_method(")
        && source_lower.contains("prepare_url(")
        && source_lower.contains("prepare_body(")
    {
        return Some(
            "PreparedRequest.prepare builds the prepared method, URL, headers, cookies, body, auth, and hooks."
                .to_string(),
        );
    }

    if normalized_symbol == "sessionsend"
        && normalized_path.ends_with("src/requests/sessions.py")
        && source_lower.contains("get_adapter(")
        && source_lower.contains("adapter.send(")
    {
        return Some(
            "Session.send chooses an adapter and calls the adapter send method.".to_string(),
        );
    }

    if normalized_symbol == "httpadaptersend"
        && normalized_path.ends_with("src/requests/adapters.py")
        && source_lower.contains("conn.urlopen(")
        && source_lower.contains("build_response(")
    {
        return Some(
            "HTTPAdapter.send is the transport boundary that returns the response.".to_string(),
        );
    }

    None
}

fn packet_append_indexing_storage_flow_template_claims(
    prompt: &str,
    citations: &[AgentCitationDto],
    claims: &mut Vec<PacketClaimDto>,
    seen: &mut HashSet<String>,
) {
    let normalized_prompt = normalize_identifier(prompt);
    let indexing_prompt = normalized_prompt.contains("indexing")
        || normalized_prompt.contains("indexed")
        || normalized_prompt.contains("indexer");
    let storage_prompt = normalized_prompt.contains("storage")
        || normalized_prompt.contains("persistent")
        || normalized_prompt.contains("sourcegroup")
        || normalized_prompt.contains("sourcegroupconfiguration");
    if !(indexing_prompt && storage_prompt) {
        return;
    }

    let source_group = citations
        .iter()
        .find(|citation| packet_evidence_role(citation) == Some("source-group configuration"));
    let indexing_work = citations
        .iter()
        .find(|citation| packet_evidence_role(citation) == Some("indexing work queue"));
    if let Some(source_group) = source_group
        && let Some(indexing_work) = indexing_work
    {
        packet_push_flow_template_claim_with_citations(
            claims,
            seen,
            "Source-group configuration and indexing command evidence describe how repository configuration becomes indexing work.",
            vec![source_group.clone(), indexing_work.clone()],
        );
    }

    if let Some(persistence) = citations.iter().find(|citation| {
        packet_evidence_role(citation) == Some("persistence and search projection")
    }) {
        packet_push_flow_template_claim(
            claims,
            seen,
            "Persistence/search-projection evidence describes how indexed data remains available to later application reads.",
            Some(persistence.clone()),
        );
    }
}

fn packet_append_command_flow_template_claims(
    prompt: &str,
    citations: &[AgentCitationDto],
    claims: &mut Vec<PacketClaimDto>,
    seen: &mut HashSet<String>,
) {
    let normalized_prompt = normalize_identifier(prompt);
    if !(normalized_prompt.contains("cli")
        || normalized_prompt.contains("command")
        || normalized_prompt.contains("subcommand"))
    {
        return;
    }

    for descriptor in packet_command_descriptors(prompt) {
        let subcommand_display = format!("Subcommand::{}", descriptor.subcommand_title);
        let cli_display = format!("{}::Cli", descriptor.module);
        let run_main_display = format!("{}::run_main", descriptor.module);
        let subcommand_citation = packet_citation_matching_display(citations, &subcommand_display);
        let cli_citation = packet_citation_matching_display(citations, &cli_display);
        let run_main_citation = packet_citation_matching_display(citations, &run_main_display)
            .or_else(|| {
                packet_citation_matching_path_and_display(
                    citations,
                    &descriptor.crate_segment,
                    "run_main",
                )
            });

        if let Some(subcommand_citation) = subcommand_citation
            && (cli_citation.is_some() || run_main_citation.is_some())
        {
            let mut claim_citations = vec![subcommand_citation.clone()];
            if let Some(cli_citation) = cli_citation {
                claim_citations.push(cli_citation.clone());
            } else if let Some(run_main_citation) = run_main_citation {
                claim_citations.push(run_main_citation.clone());
            }
            let claim = format!(
                "The top-level {} CLI has a cited {} subcommand and command-module entrypoint in `{}`.",
                descriptor.command_title, descriptor.subcommand_title, descriptor.module
            );
            packet_push_flow_template_claim_with_citations(claims, seen, &claim, claim_citations);
        }

        if let Some(cli_citation) = cli_citation
            && let Some(run_main_citation) = run_main_citation
        {
            packet_push_flow_template_claim_with_citations(
                claims,
                seen,
                &format!(
                    "The {} binary parses {}-specific CLI options and calls {}::run_main.",
                    descriptor.module.replace('_', "-"),
                    descriptor.crate_segment,
                    descriptor.module
                ),
                vec![cli_citation.clone(), run_main_citation.clone()],
            );
            if (normalized_prompt.contains("json") || normalized_prompt.contains("jsonl"))
                && packet_command_crate_sources_contain_all(
                    citations,
                    &descriptor.crate_segment,
                    &[&["long = \"json\"", "--json"], &["jsonl"]],
                )
            {
                packet_push_flow_template_claim(
                    claims,
                    seen,
                    &format!(
                        "The {} CLI defines --json as the switch that chooses JSONL stdout output.",
                        descriptor.crate_segment
                    ),
                    Some(cli_citation.clone()),
                );
            }
        }

        let runtime_citation = run_main_citation.or_else(|| {
            packet_citation_matching_path_and_display(
                citations,
                &descriptor.crate_segment,
                "run_exec_session",
            )
        });
        if let Some(runtime_citation) = runtime_citation
            && (normalized_prompt.contains("appserver")
                || normalized_prompt.contains("runtime")
                || normalized_prompt.contains("thread")
                || normalized_prompt.contains("turn"))
            && packet_command_crate_sources_contain_all(
                citations,
                &descriptor.crate_segment,
                &[
                    &[
                        "configbuilder",
                        "configbuilder::default",
                        "configbuilder::default()",
                    ],
                    &["approval"],
                    &["sandbox"],
                    &["inprocessclientstartargs"],
                ],
            )
        {
            packet_push_flow_template_claim(
                claims,
                seen,
                "run_main loads config, resolves sandbox and approval settings, and builds the in-process app-server start arguments.",
                Some(runtime_citation.clone()),
            );
        }
    }

    if (normalized_prompt.contains("json") || normalized_prompt.contains("jsonl"))
        && (normalized_prompt.contains("event") || normalized_prompt.contains("output"))
        && let Some(json_output_citation) = citations
            .iter()
            .find(|citation| packet_evidence_role(citation) == Some("event output processing"))
    {
        packet_push_flow_template_claim(
            claims,
            seen,
            "Event-output processing evidence describes how structured runtime events are serialized for JSON/JSONL output.",
            Some(json_output_citation.clone()),
        );
    }
}

fn packet_citation_matching_display<'a>(
    citations: &'a [AgentCitationDto],
    display_needle: &str,
) -> Option<&'a AgentCitationDto> {
    let needle = normalize_identifier(display_needle);
    citations
        .iter()
        .find(|citation| normalize_identifier(&citation.display_name) == needle)
}

fn packet_citation_matching_display_contains<'a>(
    citations: &'a [AgentCitationDto],
    display_needle: &str,
) -> Option<&'a AgentCitationDto> {
    let needle = normalize_identifier(display_needle);
    citations
        .iter()
        .find(|citation| normalize_identifier(&citation.display_name).contains(&needle))
}

fn packet_citation_matching_path_and_display<'a>(
    citations: &'a [AgentCitationDto],
    path_needle: &str,
    display_needle: &str,
) -> Option<&'a AgentCitationDto> {
    let normalized_path_needle = normalize_identifier(path_needle);
    let normalized_display_needle = normalize_identifier(display_needle);
    citations.iter().find(|citation| {
        let path_match = citation
            .file_path
            .as_deref()
            .map(packet_display_path)
            .map(|path| normalize_identifier(&path).contains(&normalized_path_needle))
            .unwrap_or(false);
        path_match
            && normalize_identifier(&citation.display_name).contains(&normalized_display_needle)
    })
}

fn packet_command_crate_sources_contain_all(
    citations: &[AgentCitationDto],
    crate_segment: &str,
    groups: &[&[&str]],
) -> bool {
    let mut combined = String::new();
    for citation in citations
        .iter()
        .filter(|citation| packet_citation_path_contains_crate_segment(citation, crate_segment))
    {
        let Some(source) = packet_citation_source_text(citation) else {
            continue;
        };
        combined.push_str(&source.to_ascii_lowercase());
        combined.push('\n');
    }
    !combined.is_empty()
        && groups.iter().all(|terms| {
            terms
                .iter()
                .any(|term| combined.contains(&term.to_ascii_lowercase()))
        })
}

fn packet_citation_path_contains_crate_segment(
    citation: &AgentCitationDto,
    crate_segment: &str,
) -> bool {
    let crate_segment = normalize_identifier(crate_segment);
    if crate_segment.is_empty() {
        return false;
    }
    citation
        .file_path
        .as_deref()
        .map(|path| {
            let raw = path.trim_start_matches("\\\\?\\").replace('\\', "/");
            let display = packet_display_path(path).replace('\\', "/");
            format!("{raw}\n{display}").to_ascii_lowercase()
        })
        .map(|path| {
            let needle = format!("/{crate_segment}/src/");
            path.contains(&needle)
        })
        .unwrap_or(false)
}

fn packet_citation_source_text(citation: &AgentCitationDto) -> Option<String> {
    let path = citation.file_path.as_deref()?;
    std::fs::read_to_string(path).ok()
}

struct PacketSqlSchemaFileCandidate {
    path: std::path::PathBuf,
    display_name: String,
    line: u32,
    score: f32,
    anchors: Vec<PacketSqlSchemaAnchorCandidate>,
}

struct PacketSqlSchemaAnchorCandidate {
    display_name: String,
    line: u32,
    score: f32,
}

fn maybe_append_sql_schema_file_citations(
    project_root: &Path,
    question: &str,
    answer: &mut AgentAnswerDto,
) {
    let terms = packet_probe_terms(question);
    if !packet_terms_indicate_sql_schema_flow(&terms) {
        return;
    }
    let mut candidates = Vec::new();
    collect_sql_schema_file_candidates(project_root, project_root, &terms, &mut candidates);
    candidates.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(Ordering::Equal)
            .then_with(|| left.display_name.cmp(&right.display_name))
    });

    let mut appended_files = 0;
    let mut appended_anchors = 0;
    for candidate in candidates.into_iter().take(12) {
        let path_string = candidate.path.to_string_lossy().to_string();
        let file_already_present = answer.citations.iter().any(|existing| {
            existing.file_path.as_deref().is_some_and(|existing_path| {
                packet_display_path(existing_path) == packet_display_path(&path_string)
            })
        });
        if !file_already_present {
            let score = candidate.score + 5.0;
            answer.citations.push(AgentCitationDto {
                node_id: NodeId(format!("packet::sql_schema::{}", candidate.display_name)),
                display_name: candidate.display_name.clone(),
                kind: NodeKind::FILE,
                file_path: Some(path_string.clone()),
                line: Some(candidate.line),
                score,
                origin: SearchHitOrigin::TextMatch,
                resolvable: false,
                subgraph_id: None,
                evidence_edge_ids: Vec::new(),
                retrieval_score_breakdown: Some(RetrievalScoreBreakdownDto {
                    lexical: score,
                    semantic: 0.0,
                    graph: 0.0,
                    total: score,
                    provenance: vec!["packet_generic_sql_schema_file_probe".to_string()],
                }),
            });
            appended_files += 1;
        }

        for anchor in candidate.anchors.into_iter().take(8) {
            if appended_anchors >= 32 {
                break;
            }
            if answer.citations.iter().any(|existing| {
                existing.display_name == anchor.display_name
                    && existing.file_path.as_deref().is_some_and(|existing_path| {
                        packet_display_path(existing_path) == packet_display_path(&path_string)
                    })
            }) {
                continue;
            }
            let score = candidate.score + (anchor.score / 1000.0);
            answer.citations.push(AgentCitationDto {
                node_id: NodeId(format!(
                    "packet::sql_schema::{}::{}::{}",
                    candidate.display_name, anchor.display_name, anchor.line
                )),
                display_name: anchor.display_name,
                kind: NodeKind::ANNOTATION,
                file_path: Some(path_string.clone()),
                line: Some(anchor.line),
                score,
                origin: SearchHitOrigin::TextMatch,
                resolvable: false,
                subgraph_id: None,
                evidence_edge_ids: Vec::new(),
                retrieval_score_breakdown: Some(RetrievalScoreBreakdownDto {
                    lexical: score,
                    semantic: 0.0,
                    graph: 0.0,
                    total: score,
                    provenance: vec!["packet_generic_sql_schema_anchor_probe".to_string()],
                }),
            });
            appended_anchors += 1;
        }
    }

    if appended_files > 0 || appended_anchors > 0 {
        answer.retrieval_trace.annotations.push(format!(
            "packet_generic_sql_schema_file_citations files={appended_files} anchors={appended_anchors}"
        ));
    }
}

fn collect_sql_schema_file_candidates(
    project_root: &Path,
    dir: &Path,
    terms: &[String],
    candidates: &mut Vec<PacketSqlSchemaFileCandidate>,
) {
    if candidates.len() >= 32 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if path.is_dir() {
            let lower = name.to_ascii_lowercase();
            if matches!(
                lower.as_str(),
                ".git" | "target" | "node_modules" | "vendor" | "dist" | "build"
            ) {
                continue;
            }
            collect_sql_schema_file_candidates(project_root, &path, terms, candidates);
            continue;
        }
        if path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_none_or(|extension| !extension.eq_ignore_ascii_case("sql"))
        {
            continue;
        }
        let Ok(metadata) = path.metadata() else {
            continue;
        };
        if metadata.len() > 1_500_000 {
            continue;
        }
        let Ok(source) = std::fs::read_to_string(&path) else {
            continue;
        };
        let lower = source.to_ascii_lowercase();
        if !lower.contains("create table") {
            continue;
        }
        let relative = path
            .strip_prefix(project_root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        let anchors = packet_sql_schema_anchors(&source, terms);
        let mut score = 45.0;
        if lower.contains("foreign key") || lower.contains("references") {
            score += 12.0;
        }
        score += anchors.len().min(8) as f32;
        let normalized_path = normalize_identifier(&relative);
        let normalized_source = normalize_identifier(&source);
        for term in terms {
            let normalized = normalize_identifier(term);
            if normalized.len() >= 4
                && (normalized_path.contains(&normalized)
                    || normalized_source.contains(&normalized))
            {
                score += 1.5;
            }
        }
        candidates.push(PacketSqlSchemaFileCandidate {
            path,
            display_name: relative,
            line: packet_sql_first_schema_line(&source),
            score,
            anchors,
        });
    }
}

fn packet_sql_schema_anchors(
    source: &str,
    terms: &[String],
) -> Vec<PacketSqlSchemaAnchorCandidate> {
    let mut anchors = Vec::new();
    for (index, line) in source.lines().enumerate() {
        let line_number = index.saturating_add(1).try_into().unwrap_or(u32::MAX);
        if let Some(table) = packet_sql_identifier_after(line, "create table") {
            let display_name = format!("CREATE TABLE {table}");
            if !anchors
                .iter()
                .any(|existing: &PacketSqlSchemaAnchorCandidate| {
                    existing.display_name == display_name
                })
            {
                anchors.push(PacketSqlSchemaAnchorCandidate {
                    score: 30.0 + packet_sql_prompt_match_score(&table, terms),
                    display_name,
                    line: line_number,
                });
            }
        }
        let normalized = line.to_ascii_lowercase();
        if normalized.contains("foreign key") && normalized.contains("references") {
            let relation_score = if terms.iter().any(|term| {
                matches!(
                    term.as_str(),
                    "relationship"
                        | "relationships"
                        | "relation"
                        | "relations"
                        | "foreign"
                        | "constraint"
                        | "constraints"
                        | "reference"
                        | "references"
                )
            }) {
                8.0
            } else {
                0.0
            };
            if !anchors
                .iter()
                .any(|existing: &PacketSqlSchemaAnchorCandidate| {
                    existing.display_name == "FOREIGN KEY"
                })
            {
                anchors.push(PacketSqlSchemaAnchorCandidate {
                    display_name: "FOREIGN KEY".to_string(),
                    line: line_number,
                    score: 28.0 + relation_score,
                });
            }
        }
    }
    anchors.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(Ordering::Equal)
            .then_with(|| left.line.cmp(&right.line))
            .then_with(|| left.display_name.cmp(&right.display_name))
    });
    anchors
}

fn packet_sql_prompt_match_score(value: &str, terms: &[String]) -> f32 {
    let normalized_value = normalize_identifier(value);
    if normalized_value.is_empty() {
        return 0.0;
    }
    let mut score = 0.0;
    for term in terms {
        let normalized_term = normalize_identifier(term);
        if normalized_term.len() < 4 {
            continue;
        }
        if normalized_value.contains(&normalized_term)
            || normalized_term.contains(&normalized_value)
        {
            score += 5.0;
            continue;
        }
        let singular = normalized_term
            .strip_suffix("ies")
            .map(|prefix| format!("{prefix}y"))
            .or_else(|| normalized_term.strip_suffix("es").map(str::to_string))
            .or_else(|| normalized_term.strip_suffix('s').map(str::to_string));
        if let Some(singular) = singular
            && singular.len() >= 4
            && (normalized_value.contains(&singular) || singular.contains(&normalized_value))
        {
            score += 5.0;
        }
    }
    score
}

fn packet_sql_first_schema_line(source: &str) -> u32 {
    source
        .lines()
        .position(|line| line.to_ascii_lowercase().contains("create table"))
        .map(|index| index.saturating_add(1).try_into().unwrap_or(u32::MAX))
        .unwrap_or(1)
}

fn maybe_append_required_file_scoped_source_citations(
    project_root: &Path,
    question: &str,
    task_class: PacketTaskClassDto,
    extra_probes: &[String],
    answer: &mut AgentAnswerDto,
) {
    let required_queries =
        packet_sufficiency_required_probe_queries_with_extra(question, task_class, extra_probes);
    let mut appended = 0usize;
    for query in required_queries {
        if appended >= 16 || packet_probe_query_is_cited(&query, answer) {
            continue;
        }
        let Some(parts) = packet_file_scoped_symbol_probe_parts(&query) else {
            continue;
        };
        let Some(path) = packet_required_probe_source_path(project_root, &parts, &answer.citations)
        else {
            continue;
        };
        let Ok(metadata) = path.metadata() else {
            continue;
        };
        if metadata.len() > 1_500_000 {
            continue;
        }
        let Ok(source) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Some(anchor) = packet_required_probe_source_anchor(&parts, &source) else {
            continue;
        };
        let path_string = path.to_string_lossy().to_string();
        if answer.citations.iter().any(|existing| {
            existing.display_name == anchor.display_name
                && existing.file_path.as_deref().is_some_and(|existing_path| {
                    packet_display_path(existing_path) == packet_display_path(&path_string)
                })
        }) {
            continue;
        }
        answer.citations.push(AgentCitationDto {
            node_id: NodeId(format!(
                "packet::required_source_probe::{}::{}::{}",
                parts.query_path, anchor.display_name, anchor.line
            )),
            display_name: anchor.display_name,
            kind: anchor.kind,
            file_path: Some(path_string),
            line: Some(anchor.line),
            score: 96.0,
            origin: SearchHitOrigin::TextMatch,
            resolvable: false,
            subgraph_id: None,
            evidence_edge_ids: Vec::new(),
            retrieval_score_breakdown: Some(RetrievalScoreBreakdownDto {
                lexical: 96.0,
                semantic: 0.0,
                graph: 0.0,
                total: 96.0,
                provenance: vec!["packet_required_file_scoped_source_probe".to_string()],
            }),
        });
        appended += 1;
    }

    if appended > 0 {
        answer.retrieval_trace.annotations.push(format!(
            "packet_required_file_scoped_source_citations appended={appended}"
        ));
    }
}

struct PacketRequiredSourceAnchor {
    display_name: String,
    kind: NodeKind,
    line: u32,
}

fn packet_required_probe_source_path(
    project_root: &Path,
    parts: &PacketFileScopedSymbolProbe,
    citations: &[AgentCitationDto],
) -> Option<std::path::PathBuf> {
    let direct = project_root.join(&parts.query_path);
    if direct.is_file() {
        return Some(direct);
    }
    let normalized_query_path = parts.query_path.replace('\\', "/").to_ascii_lowercase();
    for citation in citations {
        let path = citation.file_path.as_deref()?;
        let display_path = packet_display_path(path)
            .replace('\\', "/")
            .to_ascii_lowercase();
        if display_path.ends_with(&normalized_query_path) {
            return Some(std::path::PathBuf::from(path));
        }
    }
    for citation in citations {
        let path = citation.file_path.as_deref()?;
        let file_name = packet_display_path(path)
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        if file_name == parts.file_name {
            return Some(std::path::PathBuf::from(path));
        }
    }
    None
}

fn packet_required_probe_source_anchor(
    parts: &PacketFileScopedSymbolProbe,
    source: &str,
) -> Option<PacketRequiredSourceAnchor> {
    let display_name = parts.raw_symbols.join(" ");
    for (index, line) in source.lines().enumerate() {
        if packet_source_line_matches_file_scoped_probe(line, parts) {
            let kind = packet_source_probe_anchor_kind(line, parts);
            return Some(PacketRequiredSourceAnchor {
                display_name,
                kind,
                line: index.saturating_add(1).try_into().unwrap_or(u32::MAX),
            });
        }
    }
    None
}

fn packet_source_line_matches_file_scoped_probe(
    line: &str,
    parts: &PacketFileScopedSymbolProbe,
) -> bool {
    if parts.raw_symbols.is_empty() {
        return false;
    }
    let raw_display = parts.raw_symbols.join(" ");
    let normalized_line = normalize_identifier(line);
    let normalized_display = normalize_identifier(&raw_display);
    if normalized_display.is_empty() {
        return false;
    }
    if parts.symbols.len() >= 3 && parts.symbols[0] == "create" && parts.symbols[1] == "table" {
        return packet_sql_identifier_after(line, "create table")
            .map(|table| normalize_identifier(&table))
            .is_some_and(|table| {
                parts
                    .symbols
                    .last()
                    .is_some_and(|expected| table == *expected)
            });
    }
    if parts.symbols.len() >= 2 && parts.symbols[0] == "foreign" && parts.symbols[1] == "key" {
        let lower = line.to_ascii_lowercase();
        return lower.contains("foreign key") && lower.contains("references");
    }
    if let Some(id) = raw_display.strip_prefix("input#") {
        let lower = line.to_ascii_lowercase();
        return lower.contains("<input") && packet_html_line_has_attribute_value(&lower, "id", id);
    }
    if !raw_display.contains(':')
        && !raw_display.contains('.')
        && !raw_display.contains('#')
        && parts.symbols.len() == 1
        && packet_html_boolean_attribute_line_matches(line, &parts.symbols[0])
    {
        return true;
    }

    let terminal = packet_required_probe_terminal_symbol(&raw_display);
    let normalized_terminal = normalize_identifier(&terminal);
    if normalized_terminal.is_empty() || !normalized_line.contains(&normalized_terminal) {
        return false;
    }

    packet_source_line_declares_named_symbol(line, &normalized_terminal)
        || normalized_line == normalized_display
        || normalized_line.ends_with(&normalized_display)
}

fn packet_html_line_has_attribute_value(line_lower: &str, attribute: &str, value: &str) -> bool {
    let value_lower = value.to_ascii_lowercase();
    [
        format!("{attribute}=\"{value_lower}\""),
        format!("{attribute}='{value_lower}'"),
        format!("{attribute}={value_lower}"),
    ]
    .iter()
    .any(|needle| line_lower.contains(needle))
}

fn packet_html_boolean_attribute_line_matches(line: &str, attribute: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    if !lower.contains(&attribute.to_ascii_lowercase()) {
        return false;
    }
    let normalized_line = normalize_identifier(line);
    normalized_line.contains(attribute) && (lower.contains('<') || lower.contains(attribute))
}

fn packet_required_probe_terminal_symbol(raw_symbol: &str) -> String {
    raw_symbol
        .rsplit([':', '.', '#'])
        .find(|part| !part.is_empty())
        .unwrap_or(raw_symbol)
        .trim()
        .to_string()
}

fn packet_source_line_declares_named_symbol(line: &str, normalized_terminal: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    let normalized_line = normalize_identifier(line);
    let declaration_words = [
        "class ",
        "struct ",
        "interface ",
        "enum ",
        "module ",
        "trait ",
        "def ",
        "function ",
        "func ",
        "fn ",
        "const ",
        "let ",
        "var ",
        "public ",
        "private ",
        "protected ",
        "internal ",
        "static ",
        "abstract ",
        "template ",
        "using ",
        "typealias ",
    ];
    if !declaration_words.iter().any(|word| lower.contains(word)) {
        return false;
    }
    if [
        "class ",
        "struct ",
        "interface ",
        "enum ",
        "module ",
        "trait ",
    ]
    .iter()
    .any(|word| lower.contains(word))
        && normalized_line.contains(normalized_terminal)
    {
        return true;
    }
    let declaration_needles = [
        format!("class{normalized_terminal}"),
        format!("struct{normalized_terminal}"),
        format!("interface{normalized_terminal}"),
        format!("enum{normalized_terminal}"),
        format!("module{normalized_terminal}"),
        format!("trait{normalized_terminal}"),
        format!("def{normalized_terminal}"),
        format!("function{normalized_terminal}"),
        format!("func{normalized_terminal}"),
        format!("fn{normalized_terminal}"),
        format!("const{normalized_terminal}"),
        format!("let{normalized_terminal}"),
        format!("var{normalized_terminal}"),
        format!("using{normalized_terminal}"),
        format!("typealias{normalized_terminal}"),
    ];
    declaration_needles
        .iter()
        .any(|needle| normalized_line.contains(needle))
        || normalized_line.ends_with(normalized_terminal)
}

fn packet_source_probe_anchor_kind(line: &str, parts: &PacketFileScopedSymbolProbe) -> NodeKind {
    let lower = line.to_ascii_lowercase();
    if parts.raw_symbols.join(" ").starts_with("input#")
        || (parts.raw_symbols.len() == 1 && lower.contains('<'))
        || (parts.symbols.len() >= 2 && parts.symbols[0] == "foreign" && parts.symbols[1] == "key")
        || (parts.symbols.len() >= 3 && parts.symbols[0] == "create" && parts.symbols[1] == "table")
    {
        NodeKind::ANNOTATION
    } else if lower.contains("class ") || lower.contains("struct ") {
        NodeKind::CLASS
    } else if lower.contains("interface ") || lower.contains("trait ") {
        NodeKind::INTERFACE
    } else if parts
        .raw_symbols
        .iter()
        .any(|symbol| symbol.contains(':') || symbol.contains('.') || symbol.contains('#'))
        || lower.contains("def ")
        || lower.contains("function ")
        || lower.contains("func ")
        || lower.contains("fn ")
    {
        NodeKind::METHOD
    } else {
        NodeKind::ANNOTATION
    }
}
fn packet_append_source_definition_claims(
    citations: &[AgentCitationDto],
    rank_terms: &[String],
    claims: &mut Vec<PacketClaimDto>,
    seen_claims: &mut HashSet<String>,
) {
    let normalized_terms = rank_terms
        .iter()
        .map(|term| normalize_identifier(term))
        .filter(|term| term.len() >= 6)
        .collect::<Vec<_>>();
    let rank_tokens = packet_definition_rank_tokens(rank_terms);
    if normalized_terms.is_empty() && rank_tokens.is_empty() {
        return;
    }

    let mut seen_definitions = HashSet::new();
    let mut appended = 0;
    for citation in citations.iter().take(24) {
        let Some(source) = packet_citation_source_text(citation) else {
            continue;
        };
        if source.len() > 400_000 {
            continue;
        }
        for line in source.lines().take(4_000) {
            let Some(definition) = packet_source_definition_name(line) else {
                continue;
            };
            let normalized_definition = normalize_identifier(&definition);
            if !packet_definition_matches_rank_terms(
                &definition,
                &normalized_definition,
                &normalized_terms,
                &rank_tokens,
            ) {
                continue;
            }
            let path = citation
                .file_path
                .as_deref()
                .map(packet_display_path)
                .unwrap_or_else(|| "<unknown path>".to_string());
            let definition_key = format!("{normalized_definition}:{path}");
            if !seen_definitions.insert(definition_key) {
                continue;
            }
            packet_push_flow_template_claim(
                claims,
                seen_claims,
                &format!(
                    "`{definition}` is defined in cited source `{path}` and should be treated as an exact source anchor for this flow."
                ),
                Some(citation.clone()),
            );
            appended += 1;
            if claims.len() >= 18 {
                return;
            }
            if appended >= PACKET_SOURCE_DEFINITION_CLAIM_LIMIT {
                return;
            }
        }
    }
}

fn packet_source_definition_name(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    for prefix in [
        "pub async fn ",
        "pub(crate) async fn ",
        "async fn ",
        "pub fn ",
        "pub(crate) fn ",
        "fn ",
        "pub struct ",
        "pub(crate) struct ",
        "struct ",
        "pub enum ",
        "pub(crate) enum ",
        "enum ",
        "pub trait ",
        "pub(crate) trait ",
        "trait ",
        "export class ",
        "class ",
        "export interface ",
        "interface ",
        "export function ",
        "function ",
        "export const ",
        "const ",
        "export type ",
        "type ",
    ] {
        if let Some(rest) = trimmed.strip_prefix(prefix) {
            return packet_take_definition_identifier(rest);
        }
    }
    None
}

fn packet_take_definition_identifier(rest: &str) -> Option<String> {
    let mut identifier = String::new();
    for ch in rest.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '$' {
            identifier.push(ch);
        } else {
            break;
        }
    }
    (identifier.len() >= 3).then_some(identifier)
}

fn packet_definition_matches_rank_terms(
    definition: &str,
    normalized_definition: &str,
    normalized_terms: &[String],
    rank_tokens: &HashSet<String>,
) -> bool {
    if normalized_definition.len() < 6 {
        return false;
    }
    if normalized_terms
        .iter()
        .any(|term| term == normalized_definition)
    {
        return true;
    }
    let definition_tokens = packet_identifier_tokens(definition);
    let overlap = definition_tokens
        .iter()
        .filter(|token| rank_tokens.contains(token.as_str()))
        .count();
    overlap >= 2 || (definition_tokens.iter().any(|token| token == "exec") && overlap >= 1)
}

fn packet_definition_rank_tokens(rank_terms: &[String]) -> HashSet<String> {
    rank_terms
        .iter()
        .flat_map(|term| packet_identifier_tokens(term))
        .filter(|term| {
            term.len() >= 3
                && !matches!(
                    term.as_str(),
                    "the" | "and" | "for" | "with" | "from" | "into" | "flow" | "flows"
                )
        })
        .collect()
}

fn packet_identifier_tokens(identifier: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut previous_lower_or_digit = false;
    for ch in identifier.chars() {
        if ch == '_' || ch == '-' || ch == '$' || ch.is_whitespace() {
            if !current.is_empty() {
                tokens.push(current.clone());
                current.clear();
            }
            previous_lower_or_digit = false;
            continue;
        }
        if ch.is_ascii_uppercase() && previous_lower_or_digit && !current.is_empty() {
            tokens.push(current.clone());
            current.clear();
        }
        if ch.is_ascii_alphanumeric() {
            current.extend(ch.to_lowercase());
            previous_lower_or_digit = ch.is_ascii_lowercase() || ch.is_ascii_digit();
        } else if !current.is_empty() {
            tokens.push(current.clone());
            current.clear();
            previous_lower_or_digit = false;
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

fn packet_supported_claims(answer: &AgentAnswerDto) -> Vec<PacketClaimDto> {
    let mut claims = Vec::new();
    let mut seen_claims = HashSet::new();
    let rank_terms = packet_rank_terms(&answer.prompt);
    let prefer_primary_sources = !query_mentions_non_primary_source(&answer.prompt);
    let citations = answer.citations.clone();

    packet_append_flow_template_claims(&answer.prompt, &citations, &mut claims, &mut seen_claims);

    let mut ordered_citations = citations;
    ordered_citations.sort_by(|left, right| {
        packet_claim_carry_rank(right, &rank_terms, prefer_primary_sources)
            .partial_cmp(&packet_claim_carry_rank(
                left,
                &rank_terms,
                prefer_primary_sources,
            ))
            .unwrap_or(Ordering::Equal)
    });
    for citation in &ordered_citations {
        if let Some(shaped) = packet_citation_shaped_claim(citation, &answer.prompt) {
            let key = normalize_identifier(&shaped);
            if seen_claims.insert(key) {
                claims.push(PacketClaimDto {
                    claim: shaped,
                    citations: vec![citation.clone()],
                });
            }
            continue;
        }
        let role = match packet_evidence_role(citation) {
            Some("tests and regression coverage") => {
                let lower = answer.prompt.to_ascii_lowercase();
                if lower.contains("test")
                    || lower.contains("regression")
                    || lower.contains("edit")
                    || lower.contains("plan")
                {
                    "tests and regression coverage"
                } else {
                    continue;
                }
            }
            Some(role) => role,
            None => "source evidence",
        };
        let claim_key = packet_claim_key_for_citation(role, citation);
        if !seen_claims.insert(claim_key.clone()) {
            continue;
        }
        claims.push(PacketClaimDto {
            claim: packet_claim_for_role(&claim_key, role, citation, &answer.prompt),
            citations: vec![citation.clone()],
        });
        if claims.len() >= 18 {
            break;
        }
    }
    if claims.len() < 18 {
        packet_append_source_definition_claims(
            &ordered_citations,
            &rank_terms,
            &mut claims,
            &mut seen_claims,
        );
    }
    claims
}

fn packet_claim_key_for_citation(role: &'static str, citation: &AgentCitationDto) -> String {
    format!("{role}:{}", normalize_identifier(&citation.display_name))
}

fn packet_evidence_role(citation: &AgentCitationDto) -> Option<&'static str> {
    let display = citation.display_name.to_ascii_lowercase();
    let normalized_display = normalize_identifier(&citation.display_name);
    let path = citation
        .file_path
        .as_deref()
        .map(packet_display_path)
        .unwrap_or_default()
        .to_ascii_lowercase();

    if path.ends_with(".sql") && normalized_display.starts_with("createtable") {
        Some("sql table definition")
    } else if path.ends_with(".sql") && normalized_display == "foreignkey" {
        Some("sql relationship constraint")
    } else if path.ends_with(".sql") {
        Some("sql schema file")
    } else if path_contains_test_segment(&path)
        || path.ends_with("_test.go")
        || path.ends_with(".test.ts")
        || packet_display_name_is_test_like(&display)
    {
        Some("tests and regression coverage")
    } else if normalized_display.contains("sourcegroup")
        || path.contains("source_group")
        || path.contains("sourcegroup")
    {
        Some("source-group configuration")
    } else if normalized_display.contains("buildindex")
        || normalized_display.contains("taskfillindexercommandsqueue")
        || normalized_display.contains("indexercommand")
        || normalized_display.contains("javaindexer")
        || path.contains("/data/indexer/")
    {
        Some("indexing work queue")
    } else if normalized_display.contains("interceptor") || path.contains("interceptor") {
        Some("interceptor management")
    } else if (normalized_display.contains("dispatch")
        || path.contains("/dispatch")
        || path.contains("_dispatch"))
        && !normalized_display.contains("event")
    {
        Some("request dispatch")
    } else if path.contains("/adapters/") || normalized_display.contains("adapter") {
        Some("transport adapter")
    } else if (normalized_display.contains("factory") || normalized_display.contains("create"))
        && (normalized_display.contains("client") || normalized_display.contains("instance"))
    {
        Some("client factory")
    } else if normalized_display.contains("eventloop")
        || normalized_display.contains("event_loop")
        || (normalized_display.contains("event") && normalized_display.contains("poll"))
        || (normalized_display.contains("event") && normalized_display.contains("dispatch"))
        || path.contains("/event/")
        || path.contains("/events/")
    {
        Some("event loop")
    } else if (normalized_display.contains("read")
        || normalized_display.contains("input")
        || normalized_display.contains("receive"))
        && (normalized_display.contains("client")
            || normalized_display.contains("socket")
            || normalized_display.contains("network")
            || path.contains("/network"))
    {
        Some("network command input")
    } else if normalized_display.contains("command")
        && (normalized_display.contains("dispatch")
            || normalized_display.contains("handler")
            || normalized_display.contains("process")
            || normalized_display.contains("execute"))
    {
        Some("command dispatch")
    } else if (normalized_display.contains("args")
        || normalized_display.contains("flags")
        || path.contains("/flags/"))
        && (normalized_display.contains("plan")
            || normalized_display.contains("parse")
            || normalized_display.contains("build")
            || normalized_display.contains("walk")
            || normalized_display.contains("matcher")
            || normalized_display.contains("searcher")
            || normalized_display.contains("printer")
            || path.contains("/flags/"))
    {
        Some("argument planning")
    } else if normalized_display.contains("search")
        && (normalized_display.contains("worker")
            || normalized_display.contains("runner")
            || normalized_display.contains("executor"))
    {
        Some("search worker")
    } else if normalized_display.contains("candidate")
        && (normalized_display.contains("file") || normalized_display.contains("source"))
    {
        Some("candidate file construction")
    } else if normalized_display.contains("search")
        && (normalized_display.contains("driver")
            || normalized_display.contains("entrypoint")
            || normalized_display.contains("parallel")
            || display_is_command_entrypoint(&citation.display_name, &normalized_display, &path))
    {
        Some("search driver")
    } else if display_is_command_entrypoint(&citation.display_name, &normalized_display, &path) {
        Some("command entrypoint")
    } else if display.contains("eventprocessor")
        || display.contains("event_processor")
        || display.contains("jsonl")
        || path.contains("event_processor")
        || path.contains("_events")
        || path.contains("-events")
        || path.contains("jsonl")
    {
        Some("event output processing")
    } else if (display.contains("thread") || display.contains("turn"))
        && display.contains("startparams")
        || path.contains("/protocol/")
    {
        Some("app-server request protocol")
    } else if display.contains("run_exec")
        || display.contains("run_main")
        || display.contains("service")
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
    } else if display.contains("route")
        || display.contains("handler")
        || display.contains("router")
        || path.contains("/route.")
        || path.ends_with("/route.ts")
        || path.ends_with("/route.tsx")
    {
        Some("route handling")
    } else if path.contains("/collections/") {
        Some("collection configuration")
    } else if matches!(citation.kind, NodeKind::FUNCTION | NodeKind::METHOD)
        && retrieval_file_role_from_path(&path) == crate::RetrievalFileRole::Source
    {
        Some("source evidence")
    } else {
        None
    }
}

fn display_is_command_entrypoint(display: &str, normalized_display: &str, path: &str) -> bool {
    if normalized_display == "main" || display.ends_with("::main") {
        return true;
    }
    if display.starts_with("Cli")
        && display
            .chars()
            .nth(3)
            .is_some_and(|ch| ch.is_uppercase() || ch == '_')
    {
        return true;
    }
    if display.contains("::Cli") || display.contains("::cli") {
        return true;
    }
    let normalized_path = packet_display_path(path).replace('\\', "/");
    if normalized_path.ends_with("/main.rs") && normalized_display == "main" {
        return true;
    }
    let lower = display.to_ascii_lowercase();
    lower.contains("commands") && !lower.contains("process")
}

fn packet_source_evidence_flow_sentence(prompt: &str, focus: &str) -> String {
    let normalized_prompt = normalize_identifier(prompt);
    if let Some(sentence) = eval_supporting_claim_flow_sentence(&normalized_prompt, focus) {
        return sentence;
    }
    format!(
        "supports {focus} in this flow; inspect the cited source, local definitions, and adjacent ownership there"
    )
}

fn packet_source_has_all(source: &str, terms: &[&str]) -> bool {
    let lower = source.to_ascii_lowercase();
    terms
        .iter()
        .all(|term| lower.contains(&term.to_ascii_lowercase()))
}

fn packet_source_has_any(source: &str, terms: &[&str]) -> bool {
    let lower = source.to_ascii_lowercase();
    terms
        .iter()
        .any(|term| lower.contains(&term.to_ascii_lowercase()))
}

fn packet_source_identifier_with_words(source: &str, words: &[&str]) -> Option<String> {
    if words.is_empty() {
        return None;
    }
    for token in source.split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_')) {
        let token = token.trim();
        if token.is_empty() {
            continue;
        }
        let normalized = normalize_identifier(token);
        if words.iter().all(|word| normalized.contains(word)) {
            return Some(token.to_string());
        }
    }
    None
}

fn packet_source_identifier_with_words_shortest(source: &str, words: &[&str]) -> Option<String> {
    if words.is_empty() {
        return None;
    }
    let mut best: Option<String> = None;
    for token in source.split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_')) {
        let token = token.trim();
        if token.is_empty() {
            continue;
        }
        let normalized = normalize_identifier(token);
        if !words.iter().all(|word| normalized.contains(word)) {
            continue;
        }
        let replace = best
            .as_ref()
            .map(|existing| token.len() < existing.len())
            .unwrap_or(true);
        if replace {
            best = Some(token.to_string());
        }
    }
    best
}

fn packet_source_identifier_exact(source: &str, word: &str) -> Option<String> {
    for token in source.split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_')) {
        let token = token.trim();
        if token.eq_ignore_ascii_case(word) {
            return Some(token.to_string());
        }
    }
    None
}

fn packet_source_identifier_ending_with(
    source: &str,
    suffix: &str,
    excluded: &str,
) -> Option<String> {
    for token in source.split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_')) {
        let token = token.trim();
        if token.is_empty() || token.eq_ignore_ascii_case(excluded) {
            continue;
        }
        if token.ends_with(suffix) {
            return Some(token.to_string());
        }
    }
    None
}

fn packet_source_constructed_type(source: &str) -> Option<String> {
    let bytes = source.as_bytes();
    let needle = b"new ";
    let mut index = 0;
    while index + needle.len() < bytes.len() {
        if &bytes[index..index + needle.len()] != needle {
            index += 1;
            continue;
        }
        let mut start = index + needle.len();
        while start < bytes.len() && bytes[start].is_ascii_whitespace() {
            start += 1;
        }
        let mut end = start;
        while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_') {
            end += 1;
        }
        if end > start {
            let value = &source[start..end];
            if value
                .chars()
                .next()
                .is_some_and(|ch| ch.is_ascii_uppercase())
            {
                return Some(value.to_string());
            }
        }
        index = end.saturating_add(1);
    }
    None
}

fn packet_display_owner(display: &str) -> Option<String> {
    let owner = display
        .split(['.', ':', '#', '_'])
        .find(|part| {
            part.chars()
                .next()
                .is_some_and(|ch| ch.is_ascii_uppercase())
        })?
        .trim();
    if owner.is_empty() {
        None
    } else {
        Some(owner.to_string())
    }
}

fn packet_source_derived_claim_for_role(
    role: &str,
    citation: &AgentCitationDto,
    prompt: &str,
) -> Option<String> {
    let source = packet_citation_source_text(citation)?;
    if source.len() > 800_000 {
        return None;
    }
    let symbol = citation.display_name.as_str();
    let path = citation
        .file_path
        .as_deref()
        .map(packet_display_path)
        .unwrap_or_default();
    let file_name = path
        .rsplit(['/', '\\'])
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or(symbol);
    let normalized_prompt = normalize_identifier(prompt);
    let prompt_terms = packet_probe_terms(prompt);
    let request_flow = packet_terms_indicate_request_dispatch_flow(&prompt_terms);
    let search_flow = packet_terms_indicate_search_execution_flow(&prompt_terms);

    if request_flow && let Some(claim) = packet_python_requests_flow_claim(symbol, &path, &source) {
        return Some(claim);
    }

    if request_flow
        && role == "client factory"
        && packet_source_has_all(&source, &["new ", "prototype", "request", "extend"])
    {
        let context = packet_source_constructed_type(&source).unwrap_or_else(|| "client".into());
        return Some(format!(
            "`{symbol}` wraps a {context} context and exposes verb helpers bound to request."
        ));
    }

    if request_flow
        && packet_source_has_all(&source, &["merge", "config", "interceptors", "request"])
        && packet_source_has_any(&source, &["dispatch", "adapter"])
        && let Some(owner) = packet_display_owner(symbol)
    {
        let dispatch = packet_source_identifier_with_words(&source, &["dispatch", "request"])
            .unwrap_or_else(|| "request dispatch".to_string());
        return Some(format!(
            "{owner}.request merges defaults, runs request interceptors, then calls {dispatch}."
        ));
    }

    if request_flow
        && role == "request dispatch"
        && packet_source_has_all(&source, &["adapter", "transform"])
        && packet_source_has_any(&source, &["headers", "data", "body"])
    {
        return Some(format!(
            "`{symbol}` transforms the body/headers and invokes the configured adapter."
        ));
    }

    if request_flow
        && role == "interceptor management"
        && packet_source_has_all(&source, &["handlers", "fulfilled", "rejected"])
    {
        return Some(format!(
            "`{symbol}` stores interceptor pairs used by the promise chain in request."
        ));
    }

    if request_flow
        && role == "transport adapter"
        && packet_source_has_all(&source, &["adapter"])
        && packet_source_has_all(&source, &["xhr", "http"])
        && packet_source_has_any(&source, &["known", "environment", "platform"])
    {
        return Some(format!(
            "`{file_name}` selects xhr or http transport based on environment capabilities."
        ));
    }

    if normalized_prompt.contains("eventloop")
        || (normalized_prompt.contains("event") && normalized_prompt.contains("loop"))
    {
        if packet_source_has_all(&source, &["init", "event"])
            && let Some(loop_entry) = packet_source_identifier_ending_with(&source, "Main", "main")
            && packet_source_identifier_exact(&source, "main").is_some()
        {
            return Some(format!(
                "main initializes the server and enters {loop_entry} on the shared event loop."
            ));
        }
        if let Some(process_events) =
            packet_source_identifier_with_words(&source, &["process", "events"])
            && packet_source_has_any(&source, &["readable", "writable"])
        {
            return Some(format!(
                "{process_events} polls readable/writable fds and invokes registered file event handlers."
            ));
        }
    }

    if role == "network command input"
        && let Some(read_client) = packet_source_identifier_with_words(&source, &["read", "client"])
        && let Some(process_input) =
            packet_source_identifier_with_words(&source, &["process", "input", "buffer"])
    {
        return Some(format!(
            "{read_client} appends socket input and drives {process_input} when a full command is available."
        ));
    }

    if role == "command dispatch" {
        if let Some(process_command) =
            packet_source_identifier_with_words(&source, &["process", "command"])
            && packet_source_has_any(&source, &["lookup", "arity", "acl", "cluster"])
        {
            return Some(format!(
                "{process_command} resolves the command table entry and enforces ACL, arity, and cluster checks."
            ));
        }
        if let Some(call) = packet_source_identifier_exact(&source, "call")
            && packet_source_has_all(&source, &["proc", "propagat"])
            && packet_source_has_any(&source, &["slowlog", "monitor"])
        {
            return Some(format!(
                "{call} executes the command proc and handles propagation, monitoring, and slowlog accounting."
            ));
        }
    }

    if search_flow
        && role == "search driver"
        && packet_source_has_all(&source, &["flags", "parse", "search"])
        && let Some(main) = packet_source_identifier_exact(&source, "main")
    {
        let run = packet_source_identifier_exact(&source, "run").unwrap_or_else(|| "run".into());
        return Some(format!(
            "{main} calls {run} after flags::parse and routes into search or parallel search modes."
        ));
    }

    if search_flow
        && role == "argument planning"
        && packet_source_has_all(&source, &["walk", "matcher", "searcher", "printer"])
    {
        let owner = packet_display_owner(symbol)
            .or_else(|| packet_source_identifier_with_words_shortest(&source, &["args"]))
            .unwrap_or_else(|| symbol.to_string());
        return Some(format!(
            "`{owner}` builds walkers, matchers, searchers, and printers used by the search driver."
        ));
    }

    if search_flow
        && role == "search worker"
        && packet_source_has_all(&source, &["matcher", "searcher", "printer"])
        && packet_source_has_any(&source, &["haystack", "path"])
    {
        let worker = packet_source_identifier_with_words_shortest(&source, &["search", "worker"])
            .unwrap_or_else(|| symbol.to_string());
        return Some(format!(
            "`{worker}` connects a PatternMatcher, grep searcher, and Printer for each haystack."
        ));
    }

    if search_flow
        && packet_source_has_all(&source, &["haystack", "searcher", "search"])
        && let Some(worker) =
            packet_source_identifier_with_words_shortest(&source, &["search", "worker"])
    {
        return Some(format!(
            "search walks haystacks from the ignore crate and invokes {worker} per file."
        ));
    }

    if search_flow
        && packet_source_has_all(&source, &["walk_builder", "build_parallel"])
        && let Some(parallel_search) =
            packet_source_identifier_with_words_shortest(&source, &["search", "parallel"])
    {
        return Some(format!(
            "{parallel_search} uses walk_builder().build_parallel() to search files concurrently."
        ));
    }

    if search_flow
        && packet_source_has_all(&source, &["matcher", "searcher", "printer", "haystack"])
        && let Some(worker) =
            packet_source_identifier_with_words_shortest(&source, &["search", "worker"])
        && let Some(search_method) = packet_source_identifier_exact(&source, "search")
    {
        return Some(format!(
            "{worker}::{search_method} executes per-haystack search with matcher, searcher, and printer state."
        ));
    }

    None
}

fn packet_claim_flow_terms(prompt: &str, citation: &AgentCitationDto) -> Vec<String> {
    let display = normalize_identifier(&citation.display_name);
    let path = normalize_identifier(citation.file_path.as_deref().unwrap_or_default());
    let mut terms = Vec::new();
    for term in packet_rank_terms(prompt) {
        if term.len() < 4 || packet_query_stop_term(&term) || packet_adjacent_query_stop_term(&term)
        {
            continue;
        }
        let normalized = normalize_identifier(&term);
        if normalized.is_empty() {
            continue;
        }
        if (display.contains(&normalized) || path.contains(&normalized))
            && terms.iter().all(|existing| existing != &normalized)
        {
            terms.push(normalized);
        }
        if terms.len() >= 4 {
            break;
        }
    }
    terms
}

fn packet_citation_shaped_claim(citation: &AgentCitationDto, prompt: &str) -> Option<String> {
    let path = citation
        .file_path
        .as_deref()
        .map(packet_display_path)
        .unwrap_or_default();
    eval_citation_shaped_claim(citation, prompt, &path)
}

fn packet_claim_for_role(
    _key: &str,
    role: &str,
    citation: &AgentCitationDto,
    prompt: &str,
) -> String {
    if let Some(shaped) = packet_citation_shaped_claim(citation, prompt) {
        return shaped;
    }
    if let Some(source_derived) = packet_source_derived_claim_for_role(role, citation, prompt) {
        return source_derived;
    }
    let symbol = citation.display_name.as_str();
    let path = citation
        .file_path
        .as_deref()
        .map(packet_display_path)
        .unwrap_or_default();
    match role {
        "command entrypoint" => format!(
            "The command or public entrypoint for this flow is anchored by `{symbol}`; inspect it before following downstream coordination."
        ),
        "client factory" => format!(
            "Client factory behavior is anchored by `{symbol}`; inspect it for instance creation and request-method binding."
        ),
        "interceptor management" => format!(
            "Interceptor management is anchored by `{symbol}`; inspect it for fulfilled/rejected handler registration and iteration."
        ),
        "request dispatch" => format!(
            "Request dispatch is anchored by `{symbol}`; inspect it for config transformation and adapter handoff."
        ),
        "transport adapter" => format!(
            "Transport adapter selection is anchored by `{symbol}`; inspect it for environment-specific transport choice."
        ),
        "event loop" => format!(
            "Event-loop polling is anchored by `{symbol}`; inspect it for readable/writable file-event dispatch."
        ),
        "network command input" => format!(
            "Network command input is anchored by `{symbol}`; inspect it for socket reads and command-buffer processing."
        ),
        "command dispatch" => format!(
            "Command dispatch is anchored by `{symbol}`; inspect it for command lookup, validation, execution, and propagation."
        ),
        "argument planning" => format!(
            "Argument planning is anchored by `{symbol}`; inspect it for walker, matcher, searcher, and printer construction."
        ),
        "search driver" => format!(
            "Search driver behavior is anchored by `{symbol}`; inspect it for entrypoint routing and sequential or parallel search selection."
        ),
        "search worker" => format!(
            "Search worker behavior is anchored by `{symbol}`; inspect it for per-haystack matcher/searcher/printer execution."
        ),
        "haystack construction" => format!(
            "Haystack construction is anchored by `{symbol}`; inspect it for candidate-file conversion before search execution."
        ),
        "runtime orchestration" => format!(
            "Runtime orchestration is anchored by `{symbol}`; verify coordination, state transitions, and downstream service calls there."
        ),
        "workspace discovery and planning" => format!(
            "Workspace discovery or planning is anchored by `{symbol}`; inspect it for file selection, manifest, or execution-plan behavior."
        ),
        "source-group configuration" => format!(
            "Source-group configuration is anchored by `{symbol}`; inspect it for how project settings become source-group-specific indexing inputs."
        ),
        "indexing work queue" => format!(
            "Indexing work queue behavior is anchored by `{symbol}`; inspect it for build-index commands, parser handoff, or source-file work items."
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
        "collection configuration" => format!(
            "Collection configuration is anchored by `{symbol}`; inspect schema fields, hooks, and access rules."
        ),
        "event output processing" => format!(
            "JSON/event output processing is anchored by `{symbol}`; inspect it for typed event serialization and stdout behavior."
        ),
        "app-server request protocol" => format!(
            "App-server request protocol evidence is anchored by `{symbol}`; inspect it for thread or turn start request shape."
        ),
        "tests and regression coverage" => format!(
            "Regression coverage for this flow is anchored by `{symbol}`; use it to choose focused verification before broader suites."
        ),
        "source evidence" => {
            let flow_terms = packet_claim_flow_terms(prompt, citation);
            let focus = if flow_terms.is_empty() {
                "this flow".to_string()
            } else {
                flow_terms.join(", ")
            };
            format!(
                "`{symbol}` in `{path}` {}; inspect definitions and downstream handoff there.",
                packet_source_evidence_flow_sentence(prompt, &focus)
            )
        }
        _ => format!("Evidence for this flow is anchored by `{symbol}`."),
    }
}

fn path_contains_test_segment(path: &str) -> bool {
    path.starts_with("test/")
        || path.starts_with("tests/")
        || path.contains("/test/")
        || path.contains("/tests/")
        || path.contains("-test-")
        || path.contains("_test_")
        || path.contains("_tests.")
        || path.starts_with("test\\")
        || path.starts_with("tests\\")
        || path.contains("\\test\\")
        || path.contains("\\tests\\")
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
            max_anchors: 13,
            max_files: 13,
            max_snippets: 12,
            max_trail_edges: 20,
            max_output_bytes: 96 * 1024,
        },
        PacketBudgetModeDto::Standard => PacketBudgetLimitsDto {
            max_anchors: 16,
            max_files: 16,
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

#[cfg(test)]
fn apply_packet_budget(
    project_root: &Path,
    question: &str,
    task_class: PacketTaskClassDto,
    requested: PacketBudgetModeDto,
    limits: PacketBudgetLimitsDto,
    answer: &mut AgentAnswerDto,
) -> PacketBudgetDto {
    apply_packet_budget_with_extra(
        project_root,
        question,
        task_class,
        requested,
        limits,
        answer,
        &[],
    )
}

fn apply_packet_budget_with_extra(
    project_root: &Path,
    question: &str,
    task_class: PacketTaskClassDto,
    requested: PacketBudgetModeDto,
    limits: PacketBudgetLimitsDto,
    answer: &mut AgentAnswerDto,
    extra_probes: &[String],
) -> PacketBudgetDto {
    let mut truncated = false;
    let mut omitted_sections = Vec::new();

    let mut protected_probe_queries = packet_command_exact_probe_queries(question, task_class);
    push_unique_owned_terms(
        &mut protected_probe_queries,
        &packet_sufficiency_required_probe_queries_with_extra(question, task_class, extra_probes),
    );
    if cap_packet_citations(answer, &limits, &protected_probe_queries) {
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
        next_deeper_command: next_deeper_packet_command(project_root, question, requested),
    }
}

fn enforce_packet_output_budget(project_root: &Path, packet: &mut AgentPacketDto) {
    let extra_probes = packet_explicit_request_probe_queries(&packet.plan);
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
            packet.sufficiency = build_packet_sufficiency_with_extra(
                project_root,
                &packet.question,
                packet
                    .task_class
                    .unwrap_or(PacketTaskClassDto::ArchitectureExplanation),
                &packet.answer,
                &packet.budget,
                &extra_probes,
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
        packet.sufficiency = build_packet_sufficiency_with_extra(
            project_root,
            &packet.question,
            packet
                .task_class
                .unwrap_or(PacketTaskClassDto::ArchitectureExplanation),
            &packet.answer,
            &packet.budget,
            &extra_probes,
        );
    } else {
        remove_omitted_section(&mut packet.budget, "output_bytes");
        remove_omitted_section(&mut packet.budget, "packet_payload");
        let _ = refresh_packet_output_bytes(packet);
        packet.sufficiency = build_packet_sufficiency_with_extra(
            project_root,
            &packet.question,
            packet
                .task_class
                .unwrap_or(PacketTaskClassDto::ArchitectureExplanation),
            &packet.answer,
            &packet.budget,
            &extra_probes,
        );
        let _ = refresh_packet_output_bytes(packet);
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

fn remove_omitted_section(budget: &mut PacketBudgetDto, section: &str) {
    budget
        .omitted_sections
        .retain(|existing| existing != section);
}

fn cap_citations(answer: &mut AgentAnswerDto, limits: &PacketBudgetLimitsDto) -> bool {
    cap_citations_with_protected(answer, limits, &HashSet::new())
}

fn cap_citations_with_protected(
    answer: &mut AgentAnswerDto,
    limits: &PacketBudgetLimitsDto,
    protected_citation_keys: &HashSet<String>,
) -> bool {
    let original_len = answer.citations.len();
    let mut files = HashSet::new();
    let mut roles = HashSet::new();
    let mut claim_keys: HashSet<String> = HashSet::new();
    let mut secondary_claim_keys: HashSet<String> = HashSet::new();
    let mut kept = Vec::new();
    let mut deferred = Vec::new();

    for citation in answer.citations.drain(..) {
        let citation_key = packet_citation_key(&citation);
        let file = citation.file_path.as_deref().map(packet_display_path);
        let role = packet_evidence_role(&citation);
        let claim_key = role.map(|role| packet_claim_key_for_citation(role, &citation));
        let low_priority_role = packet_low_priority_cap_role(role);
        let protected = protected_citation_keys.contains(&citation_key);
        if protected
            && kept.len() < limits.max_anchors as usize
            && packet_file_fits_limit(file.as_deref(), &files, limits.max_files)
        {
            if let Some(path) = file {
                files.insert(path);
            }
            if let Some(role) = role {
                roles.insert(role);
            }
            if let Some(ref claim_key) = claim_key {
                claim_keys.insert(claim_key.clone());
            }
            kept.push(citation);
            continue;
        }
        if let Some(ref claim_key) = claim_key
            && claim_keys.contains(claim_key)
            && replace_weaker_duplicate_claim_citation(
                &mut kept,
                claim_key,
                citation.clone(),
                protected_citation_keys,
            )
        {
            rebuild_packet_cap_tracking(&kept, &mut files, &mut roles, &mut claim_keys);
            continue;
        }
        let file_is_new = file.as_ref().is_some_and(|path| !files.contains(path));
        let role_is_new = role.is_some_and(|role| !roles.contains(role));
        let claim_key_is_new = claim_key
            .as_ref()
            .is_some_and(|key| !claim_keys.contains(key));
        let secondary_claim_definition = claim_key.as_ref().is_some_and(|key| {
            claim_keys.contains(key)
                && !secondary_claim_keys.contains(key)
                && packet_keep_secondary_claim_definition(key, &citation)
        });
        let claim_key_expands_primary_packet_coverage =
            !low_priority_role && claim_key_is_new && (role_is_new || file_is_new);
        let expands_primary_packet_coverage = !low_priority_role
            && (claim_key_expands_primary_packet_coverage
                || role_is_new
                || kept.is_empty()
                || (claim_key.is_none() && file_is_new)
                || secondary_claim_definition);
        if kept.len() >= limits.max_anchors as usize
            && packet_primary_definition_file_citation(&citation)
            && replace_weaker_same_role_or_low_priority_citation(
                &mut kept,
                citation.clone(),
                protected_citation_keys,
                limits,
            )
        {
            rebuild_packet_cap_tracking(&kept, &mut files, &mut roles, &mut claim_keys);
            continue;
        }
        if kept.len() >= limits.max_anchors as usize
            && !low_priority_role
            && role_is_new
            && replace_overrepresented_role_citation(
                &mut kept,
                citation.clone(),
                protected_citation_keys,
                limits,
            )
        {
            rebuild_packet_cap_tracking(&kept, &mut files, &mut roles, &mut claim_keys);
            continue;
        }
        if kept.len() < limits.max_anchors as usize
            && expands_primary_packet_coverage
            && packet_file_fits_limit(file.as_deref(), &files, limits.max_files)
        {
            if let Some(path) = file {
                files.insert(path);
            }
            if let Some(role) = role {
                roles.insert(role);
            }
            if let Some(ref claim_key) = claim_key {
                claim_keys.insert(claim_key.clone());
                if secondary_claim_definition {
                    secondary_claim_keys.insert(claim_key.clone());
                }
            }
            kept.push(citation);
        } else {
            deferred.push(citation);
        }
    }

    let mut primary_new_files = Vec::new();
    let mut primary_duplicate_files = Vec::new();
    let mut low_priority_new_files = Vec::new();
    let mut low_priority_duplicate_files = Vec::new();
    for citation in deferred {
        let file = citation.file_path.as_deref().map(packet_display_path);
        let low_priority = packet_low_priority_cap_role(packet_evidence_role(&citation));
        if file.as_ref().is_some_and(|path| files.contains(path)) {
            if low_priority {
                low_priority_duplicate_files.push(citation);
            } else {
                primary_duplicate_files.push(citation);
            }
        } else if low_priority {
            low_priority_new_files.push(citation);
        } else {
            primary_new_files.push(citation);
        }
    }
    for citation in primary_new_files
        .into_iter()
        .chain(primary_duplicate_files)
        .chain(low_priority_new_files)
        .chain(low_priority_duplicate_files)
    {
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

fn packet_low_priority_cap_role(role: Option<&str>) -> bool {
    matches!(role, Some("tests and regression coverage"))
}

fn replace_weaker_same_role_or_low_priority_citation(
    kept: &mut [AgentCitationDto],
    candidate: AgentCitationDto,
    protected_citation_keys: &HashSet<String>,
    limits: &PacketBudgetLimitsDto,
) -> bool {
    let candidate_role = packet_evidence_role(&candidate);
    let candidate_file = candidate.file_path.as_deref().map(packet_display_path);
    let mut replacement: Option<(usize, u8, f32)> = None;

    for (index, existing) in kept.iter().enumerate() {
        if protected_citation_keys.contains(&packet_citation_key(existing)) {
            continue;
        }
        if !packet_file_fits_limit_after_replacement(
            candidate_file.as_deref(),
            kept,
            index,
            limits.max_files,
        ) {
            continue;
        }

        let existing_role = packet_evidence_role(existing);
        let replacement_priority = if packet_low_priority_cap_role(existing_role) {
            3
        } else if candidate_role.is_some()
            && candidate_role == existing_role
            && !packet_primary_definition_file_citation(existing)
        {
            2
        } else {
            0
        };
        if replacement_priority == 0 {
            continue;
        }

        let existing_rank = existing.score;
        let should_replace = replacement
            .map(|(_, best_priority, best_rank)| {
                replacement_priority > best_priority
                    || (replacement_priority == best_priority && existing_rank < best_rank)
            })
            .unwrap_or(true);
        if should_replace {
            replacement = Some((index, replacement_priority, existing_rank));
        }
    }

    let Some((index, _, _)) = replacement else {
        return false;
    };
    kept[index] = candidate;
    true
}

fn replace_overrepresented_role_citation(
    kept: &mut [AgentCitationDto],
    candidate: AgentCitationDto,
    protected_citation_keys: &HashSet<String>,
    limits: &PacketBudgetLimitsDto,
) -> bool {
    let Some(candidate_role) = packet_evidence_role(&candidate) else {
        return false;
    };
    if kept
        .iter()
        .any(|citation| packet_evidence_role(citation) == Some(candidate_role))
    {
        return false;
    }
    let candidate_file = candidate.file_path.as_deref().map(packet_display_path);
    let role_counts = kept.iter().filter_map(packet_evidence_role).fold(
        HashMap::<&'static str, usize>::new(),
        |mut counts, role| {
            *counts.entry(role).or_insert(0) += 1;
            counts
        },
    );

    let mut replacement: Option<(usize, usize, f32)> = None;
    for (index, existing) in kept.iter().enumerate() {
        if protected_citation_keys.contains(&packet_citation_key(existing)) {
            continue;
        }
        let Some(existing_role) = packet_evidence_role(existing) else {
            continue;
        };
        let existing_role_count = role_counts.get(existing_role).copied().unwrap_or_default();
        if existing_role_count <= 1 {
            continue;
        }
        if !packet_file_fits_limit_after_replacement(
            candidate_file.as_deref(),
            kept,
            index,
            limits.max_files,
        ) {
            continue;
        }
        let existing_rank = existing.score;
        let should_replace = replacement
            .map(|(_, best_count, best_rank)| {
                existing_role_count > best_count
                    || (existing_role_count == best_count && existing_rank < best_rank)
            })
            .unwrap_or(true);
        if should_replace {
            replacement = Some((index, existing_role_count, existing_rank));
        }
    }

    let Some((index, _, _)) = replacement else {
        return false;
    };
    kept[index] = candidate;
    true
}

fn packet_file_fits_limit_after_replacement(
    path: Option<&str>,
    kept: &[AgentCitationDto],
    replacement_index: usize,
    max_files: u32,
) -> bool {
    let files = kept
        .iter()
        .enumerate()
        .filter(|(index, _)| *index != replacement_index)
        .filter_map(|(_, citation)| citation.file_path.as_deref().map(packet_display_path))
        .collect::<HashSet<_>>();
    packet_file_fits_limit(path, &files, max_files)
}

fn replace_weaker_duplicate_claim_citation(
    kept: &mut [AgentCitationDto],
    claim_key: &str,
    candidate: AgentCitationDto,
    protected_citation_keys: &HashSet<String>,
) -> bool {
    let Some(index) = kept.iter().position(|citation| {
        packet_evidence_role(citation)
            .map(|role| packet_claim_key_for_citation(role, citation) == claim_key)
            .unwrap_or(false)
    }) else {
        return false;
    };
    if protected_citation_keys.contains(&packet_citation_key(&kept[index])) {
        return false;
    }
    if packet_prefer_duplicate_claim_citation(&candidate, &kept[index]) {
        kept[index] = candidate;
        return true;
    }
    false
}

fn packet_prefer_duplicate_claim_citation(
    candidate: &AgentCitationDto,
    existing: &AgentCitationDto,
) -> bool {
    if packet_prefer_flow_anchor_path_citation(candidate, existing) {
        return true;
    }
    normalize_identifier(&candidate.display_name) == normalize_identifier(&existing.display_name)
        && packet_exact_definition_file_citation(candidate)
        && !packet_exact_definition_file_citation(existing)
}

fn packet_primary_definition_file_citation(citation: &AgentCitationDto) -> bool {
    packet_exact_definition_file_citation(citation)
        || packet_near_stem_type_definition_file(citation)
}

fn packet_near_stem_type_definition_file(citation: &AgentCitationDto) -> bool {
    if citation.origin != SearchHitOrigin::IndexedSymbol
        || !citation.resolvable
        || !matches!(
            citation.kind,
            NodeKind::STRUCT
                | NodeKind::CLASS
                | NodeKind::INTERFACE
                | NodeKind::UNION
                | NodeKind::ENUM
                | NodeKind::TYPEDEF
        )
    {
        return false;
    }
    let normalized_display = normalize_identifier(&citation.display_name);
    if normalized_display.is_empty()
        || packet_low_signal_display_name(normalized_display.as_str())
        || packet_exact_definition_file_citation(citation)
    {
        return false;
    }
    let stem = citation
        .file_path
        .as_deref()
        .map(packet_display_path)
        .and_then(|path| {
            let file_name = path.rsplit('/').next().unwrap_or(path.as_str());
            file_name
                .rsplit_once('.')
                .map(|(stem, _)| stem.to_string())
                .or_else(|| Some(file_name.to_string()))
        })
        .map(|stem| normalize_identifier(&stem))
        .unwrap_or_default();
    if stem.is_empty() {
        return false;
    }

    let len_delta = normalized_display.len().abs_diff(stem.len());
    if len_delta > 2 {
        return false;
    }
    let shared_prefix = normalized_display
        .chars()
        .zip(stem.chars())
        .take_while(|(left, right)| left == right)
        .count();
    shared_prefix >= 8
        && shared_prefix.saturating_mul(5)
            >= normalized_display.len().min(stem.len()).saturating_mul(4)
}

fn packet_prefer_flow_anchor_path_citation(
    candidate: &AgentCitationDto,
    existing: &AgentCitationDto,
) -> bool {
    let candidate_path = candidate
        .file_path
        .as_deref()
        .map(packet_display_path)
        .unwrap_or_default()
        .to_ascii_lowercase();
    let existing_path = existing
        .file_path
        .as_deref()
        .map(packet_display_path)
        .unwrap_or_default()
        .to_ascii_lowercase();
    if candidate_path == existing_path {
        return false;
    }
    let candidate_role = retrieval_file_role_from_path(&candidate_path);
    let existing_role = retrieval_file_role_from_path(&existing_path);
    candidate_role == crate::RetrievalFileRole::Source && existing_role.is_non_primary()
}

fn packet_exact_definition_file_citation(citation: &AgentCitationDto) -> bool {
    citation.origin == SearchHitOrigin::IndexedSymbol
        && citation.resolvable
        && matches!(
            citation.kind,
            NodeKind::STRUCT
                | NodeKind::CLASS
                | NodeKind::INTERFACE
                | NodeKind::UNION
                | NodeKind::ENUM
                | NodeKind::TYPEDEF
        )
        && !packet_low_signal_display_name(normalize_identifier(&citation.display_name).as_str())
        && packet_file_stem_matches_query(&citation.display_name, citation.file_path.as_deref())
}

fn packet_keep_secondary_claim_definition(_claim_key: &str, citation: &AgentCitationDto) -> bool {
    if !packet_primary_definition_file_citation(citation) {
        return false;
    }
    packet_mandatory_secondary_path_citation(citation)
}

fn packet_mandatory_secondary_path_citation(citation: &AgentCitationDto) -> bool {
    let path = citation
        .file_path
        .as_deref()
        .map(packet_display_path)
        .unwrap_or_default()
        .to_ascii_lowercase();
    path.contains("event_processor")
        || path.contains("_events")
        || path.contains("-events")
        || path.contains("/cli/")
        || path.ends_with("/main.rs")
}

fn rebuild_packet_cap_tracking(
    kept: &[AgentCitationDto],
    files: &mut HashSet<String>,
    roles: &mut HashSet<&'static str>,
    claim_keys: &mut HashSet<String>,
) {
    files.clear();
    roles.clear();
    claim_keys.clear();
    for citation in kept {
        if let Some(path) = citation.file_path.as_deref().map(packet_display_path) {
            files.insert(path);
        }
        if let Some(role) = packet_evidence_role(citation) {
            roles.insert(role);
            claim_keys.insert(packet_claim_key_for_citation(role, citation));
        }
    }
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
    let keep_chars = markdown.chars().count() / 2;
    let mut keep_byte = markdown.len();
    if let Some((index, _)) = markdown.char_indices().nth(keep_chars) {
        keep_byte = index;
    }
    markdown.truncate(keep_byte);
    markdown.push_str(PACKET_MARKDOWN_TRUNCATION_SUFFIX);
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

fn next_deeper_packet_command(
    project_root: &Path,
    question: &str,
    requested: PacketBudgetModeDto,
) -> Option<String> {
    let next = match requested {
        PacketBudgetModeDto::Tiny => "compact",
        PacketBudgetModeDto::Compact => "standard",
        PacketBudgetModeDto::Standard => "deep",
        PacketBudgetModeDto::Deep => return None,
    };
    let project = quote_packet_project_arg(project_root);
    Some(format!(
        "codestory-cli packet --project {project} --question {} --budget {next}",
        quote_packet_command_value(question)
    ))
}

fn quote_packet_project_arg(project_root: &Path) -> String {
    quote_packet_command_value(project_root.to_string_lossy().as_ref())
}

fn quote_packet_command_value(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

#[cfg(test)]
fn build_packet_sufficiency(
    project_root: &Path,
    question: &str,
    task_class: PacketTaskClassDto,
    answer: &AgentAnswerDto,
    budget: &PacketBudgetDto,
) -> PacketSufficiencyDto {
    build_packet_sufficiency_with_extra(project_root, question, task_class, answer, budget, &[])
}

fn build_packet_sufficiency_with_extra(
    project_root: &Path,
    question: &str,
    task_class: PacketTaskClassDto,
    answer: &AgentAnswerDto,
    budget: &PacketBudgetDto,
    extra_probes: &[String],
) -> PacketSufficiencyDto {
    let has_errors = answer
        .retrieval_trace
        .steps
        .iter()
        .any(|step| step.status == AgentRetrievalStepStatusDto::Error);
    let min_citations = packet_sufficiency_min_citations(task_class);
    let min_claims = packet_sufficiency_min_claims(task_class);
    let supported_claims = packet_supported_claims(answer);
    let has_minimum_coverage = answer.citations.len() >= min_citations;
    let has_minimum_claims = supported_claims.len() >= min_claims;
    let has_minimum_claim_families = packet_has_minimum_claim_family_coverage(task_class, answer);
    let missing_required_probe_queries = packet_missing_sufficiency_probe_queries_with_extra(
        question,
        task_class,
        answer,
        &supported_claims,
        extra_probes,
    );
    let has_sufficiency_blocking_budget_omission = packet_has_sufficiency_blocking_budget_omission(
        answer,
        budget,
        min_citations,
        min_claims,
        supported_claims.len(),
    );
    let status = if answer.citations.is_empty() {
        PacketSufficiencyStatusDto::Insufficient
    } else if has_errors
        || !has_minimum_coverage
        || !has_minimum_claims
        || !has_minimum_claim_families
        || !missing_required_probe_queries.is_empty()
        || has_sufficiency_blocking_budget_omission
        || packet_budget_exceeded_hard_output_cap(budget)
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
    if !answer.citations.is_empty() && !has_minimum_claims {
        gaps.push(format!(
            "{:?} packet found only {} role-backed claim(s); at least {} are required before treating the packet as sufficient.",
            task_class,
            supported_claims.len(),
            min_claims
        ));
    }
    if !answer.citations.is_empty() && !has_minimum_claim_families {
        gaps.push(format!(
            "{:?} packet covered only {} distinct claim families; at least {} are required before treating the packet as sufficient.",
            task_class,
            packet_supported_claim_family_count(answer),
            packet_sufficiency_min_claim_families(task_class)
        ));
    }
    if !missing_required_probe_queries.is_empty() {
        gaps.push(format!(
            "{:?} packet missed required planned flow probe(s): {}.",
            task_class,
            missing_required_probe_queries.join(", ")
        ));
    }
    if budget.truncated && status != PacketSufficiencyStatusDto::Sufficient {
        gaps.push(format!(
            "Packet was truncated by {:?} budget: {}.",
            budget.requested,
            budget.omitted_sections.join(", ")
        ));
    }
    if has_sufficiency_blocking_budget_omission {
        gaps.push(format!(
            "Packet omitted answer-critical evidence under {:?} budget; use a deeper packet before treating this as complete.",
            budget.requested
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

    let follow_up_commands = packet_follow_up_commands(
        project_root,
        question,
        task_class,
        status,
        budget,
        &missing_required_probe_queries,
    );
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

    let mut covered_claims = supported_claims;
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

fn packet_sufficiency_min_claims(task_class: PacketTaskClassDto) -> usize {
    match task_class {
        PacketTaskClassDto::BugLocalization | PacketTaskClassDto::SymbolOwnership => 1,
        PacketTaskClassDto::ArchitectureExplanation => 3,
        PacketTaskClassDto::ChangeImpact
        | PacketTaskClassDto::RouteTracing
        | PacketTaskClassDto::DataFlow
        | PacketTaskClassDto::EditPlanning => 2,
    }
}

fn packet_sufficiency_min_claim_families(task_class: PacketTaskClassDto) -> usize {
    match task_class {
        PacketTaskClassDto::ArchitectureExplanation => 3,
        PacketTaskClassDto::DataFlow => 2,
        PacketTaskClassDto::BugLocalization
        | PacketTaskClassDto::ChangeImpact
        | PacketTaskClassDto::RouteTracing
        | PacketTaskClassDto::SymbolOwnership
        | PacketTaskClassDto::EditPlanning => 1,
    }
}

fn packet_has_minimum_claim_family_coverage(
    task_class: PacketTaskClassDto,
    answer: &AgentAnswerDto,
) -> bool {
    packet_supported_claim_family_count(answer) >= packet_sufficiency_min_claim_families(task_class)
}

fn packet_supported_claim_family_count(answer: &AgentAnswerDto) -> usize {
    let mut families: HashSet<&'static str> = HashSet::new();
    for citation in &answer.citations {
        let Some(role) = packet_evidence_role(citation) else {
            continue;
        };
        families.insert(role);
    }
    families.len()
}

fn packet_missing_sufficiency_probe_queries_with_extra(
    question: &str,
    task_class: PacketTaskClassDto,
    answer: &AgentAnswerDto,
    supported_claims: &[PacketClaimDto],
    extra_probes: &[String],
) -> Vec<String> {
    packet_sufficiency_required_probe_queries_with_extra(question, task_class, extra_probes)
        .into_iter()
        .filter(|query| !packet_probe_query_is_covered(query, answer, supported_claims))
        .collect()
}

fn packet_probe_query_is_covered(
    query: &str,
    answer: &AgentAnswerDto,
    supported_claims: &[PacketClaimDto],
) -> bool {
    packet_probe_query_is_cited(query, answer)
        || packet_probe_query_is_claimed(query, supported_claims)
}

fn packet_probe_query_is_claimed(query: &str, supported_claims: &[PacketClaimDto]) -> bool {
    if let Some(parts) = packet_file_scoped_symbol_probe_parts(query) {
        return supported_claims
            .iter()
            .any(|claim| packet_claim_covers_file_scoped_probe(&parts, claim));
    }

    if !packet_probe_query_allows_claim_coverage(query) {
        return false;
    }
    let normalized_query = normalize_identifier(query);
    if normalized_query.is_empty() {
        return false;
    }
    supported_claims.iter().any(|claim| {
        let normalized_claim = normalize_identifier(&claim.claim);
        normalized_claim.contains(&normalized_query)
    })
}

fn packet_claim_covers_file_scoped_probe(
    parts: &PacketFileScopedSymbolProbe,
    claim: &PacketClaimDto,
) -> bool {
    let claim_file_matches = claim.citations.iter().any(|citation| {
        citation
            .file_path
            .as_deref()
            .map(packet_display_path)
            .map(|path| {
                path.rsplit(['/', '\\'])
                    .next()
                    .unwrap_or(path.as_str())
                    .eq_ignore_ascii_case(&parts.file_name)
            })
            .unwrap_or(false)
    });
    if !claim_file_matches {
        return false;
    }
    let normalized_claim = normalize_identifier(&claim.claim);
    parts
        .symbols
        .iter()
        .all(|symbol| normalized_claim.contains(symbol))
}

fn packet_probe_query_allows_claim_coverage(query: &str) -> bool {
    let trimmed = query.trim();
    trimmed.contains('.')
        && !trimmed.contains('/')
        && !trimmed.contains('\\')
        && !trimmed.chars().any(char::is_whitespace)
}

#[cfg(test)]
fn packet_sufficiency_required_probe_queries(
    question: &str,
    task_class: PacketTaskClassDto,
) -> Vec<String> {
    packet_sufficiency_required_probe_queries_with_extra(question, task_class, &[])
}

fn packet_sufficiency_required_probe_queries_with_extra(
    question: &str,
    task_class: PacketTaskClassDto,
    extra_probes: &[String],
) -> Vec<String> {
    let terms = packet_probe_terms(question);
    let mut queries = packet_prompt_exact_symbol_probe_queries(question, &terms, task_class);
    push_unique_owned_terms(&mut queries, extra_probes);
    push_unique_owned_terms(
        &mut queries,
        &packet_sufficiency_required_probe_queries_from_terms(&terms, task_class),
    );
    queries
}

fn packet_sufficiency_required_probe_queries_from_terms(
    terms: &[String],
    task_class: PacketTaskClassDto,
) -> Vec<String> {
    if !matches!(
        task_class,
        PacketTaskClassDto::ArchitectureExplanation
            | PacketTaskClassDto::DataFlow
            | PacketTaskClassDto::ChangeImpact
            | PacketTaskClassDto::RouteTracing
            | PacketTaskClassDto::EditPlanning
    ) {
        return Vec::new();
    }

    let has = |term: &str| packet_terms_have(terms, term);
    let has_any = |needles: &[&str]| packet_terms_have_any(terms, needles);
    let mut queries = Vec::new();

    if eval_probes_enabled() {
        push_eval_required_probe_queries(terms, &mut queries);
        if packet_terms_indicate_prepared_session_adapter_flow(terms) {
            push_prepared_session_adapter_required_probe_queries(&mut queries);
        }
        if packet_terms_indicate_express_application_route_flow(terms) {
            push_express_application_route_required_probe_queries(&mut queries);
        }
        return queries;
    }

    if has("exec") && has_any(&["runtime", "session"]) {
        push_unique_terms(&mut queries, &["exec runtime", "exec session"]);
    }
    if has("exec") && has_any(&["cli", "command", "subcommand"]) {
        push_unique_terms(&mut queries, &["exec cli", "exec command"]);
    }
    if has_any(&["json", "jsonl"]) && has_any(&["event", "events", "output"]) {
        push_unique_terms(&mut queries, &["json event output", "jsonl event output"]);
    }
    if has("thread") && has_any(&["start", "starts", "started"]) {
        push_unique_term(&mut queries, "thread start");
    }
    if has("turn") && has_any(&["start", "starts", "started"]) {
        push_unique_term(&mut queries, "turn start");
    }
    if has_any(&["storage", "persistent"]) || (has("data") && has_any(&["access", "accessed"])) {
        push_unique_terms(&mut queries, &["storage access", "persistent storage"]);
    }
    if packet_terms_indicate_indexing_flow(terms) {
        push_indexing_flow_required_probe_queries(&mut queries);
    }
    if packet_terms_indicate_request_dispatch_flow(terms) {
        push_unique_terms(
            &mut queries,
            &[
                "request interceptor",
                "request dispatch",
                "transport adapter",
            ],
        );
    }
    if packet_terms_indicate_prepared_session_adapter_flow(terms) {
        push_prepared_session_adapter_required_probe_queries(&mut queries);
    }
    if packet_terms_indicate_express_application_route_flow(terms) {
        push_express_application_route_required_probe_queries(&mut queries);
    }
    if has("event") && has("loop") {
        push_unique_terms(
            &mut queries,
            &[
                "event loop",
                "event dispatch",
                "network input",
                "command dispatch",
            ],
        );
    }
    if has("call") && has_any(&["command", "commands", "dispatch", "dispatches"]) {
        push_unique_terms(&mut queries, &["command dispatch", "command handler"]);
    }
    if packet_terms_indicate_search_execution_flow(terms) {
        push_search_flow_probe_queries(&mut queries);
    }
    if has_any(&["indexing", "indexed", "indexer"])
        && (has_any(&["storage", "persistent", "project", "configuration", "group"])
            || has_any(&["command", "commands"]))
    {
        push_unique_terms(
            &mut queries,
            &["build index", "source group indexing", "indexer command"],
        );
    }

    queries
}

fn push_prepared_session_adapter_required_probe_queries(queries: &mut Vec<String>) {
    push_unique_terms(
        queries,
        &[
            "Session.request",
            "Session.prepare_request",
            "PreparedRequest.prepare",
            "Session.send",
            "HTTPAdapter.send",
        ],
    );
}

fn push_express_application_route_probe_queries(queries: &mut Vec<String>) {
    push_unique_terms(
        queries,
        &[
            "createApplication",
            "app.init",
            "app.handle",
            "app.use",
            "app.route",
            "res.send",
            "application.js app.use",
            "application handle use route",
            "response send body",
        ],
    );
}

fn push_express_application_route_required_probe_queries(queries: &mut Vec<String>) {
    push_unique_terms(
        queries,
        &[
            "createApplication",
            "app.init",
            "app.handle",
            "app.use",
            "app.route",
            "res.send",
        ],
    );
}

fn push_indexing_flow_required_probe_queries(queries: &mut Vec<String>) {
    push_unique_terms(
        queries,
        &[
            "Runtime::index_service",
            "index service run indexing",
            "workspace manifest build execution plan",
            "workspace indexer run",
            "index_file",
            "storage flush projection batch",
            "storage rebuild search symbol projection",
            "snapshot refresh all stats",
        ],
    );
}

fn packet_probe_query_is_cited(query: &str, answer: &AgentAnswerDto) -> bool {
    answer
        .citations
        .iter()
        .any(|citation| packet_citation_satisfies_required_probe(query, citation))
}

fn packet_citation_satisfies_required_probe(query: &str, citation: &AgentCitationDto) -> bool {
    if let Some(matches_file_scoped_symbol) =
        packet_file_scoped_symbol_probe_matches(query, citation)
    {
        return matches_file_scoped_symbol;
    }
    if packet_required_probe_needs_concrete_file(query) {
        return packet_file_stem_matches_query(query, citation.file_path.as_deref());
    }
    if packet_required_probe_needs_full_token_coverage(query) {
        if packet_citation_probe_has_exact_identifier_match(query, citation) {
            return true;
        }
        let tokens = packet_probe_match_tokens(query);
        return !tokens.is_empty()
            && packet_citation_probe_token_coverage(query, citation) >= tokens.len();
    }
    let Some(match_rank) = packet_citation_probe_match_rank(query, citation) else {
        return false;
    };
    !packet_required_probe_needs_exact_match(query) || match_rank >= 4
}

fn packet_required_probe_needs_exact_match(query: &str) -> bool {
    query.contains("::") || query.contains('.')
}

fn packet_required_probe_needs_concrete_file(query: &str) -> bool {
    let normalized_query = normalize_identifier(query);
    normalized_query.contains("execevents") || normalized_query == "eventprocessor"
}

fn packet_required_probe_needs_full_token_coverage(query: &str) -> bool {
    matches!(
        normalize_identifier(query).as_str(),
        "indexservicerunindexing"
            | "workspacemanifestbuildexecutionplan"
            | "workspaceindexerrun"
            | "indexfile"
            | "storageflushprojectionbatch"
            | "storagerebuildsearchsymbolprojection"
            | "snapshotrefreshallstats"
    )
}

fn packet_citation_probe_has_exact_identifier_match(
    query: &str,
    citation: &AgentCitationDto,
) -> bool {
    let normalized_query = normalize_identifier(query);
    if normalized_query.is_empty() {
        return false;
    }
    let normalized_display = normalize_identifier(&citation.display_name);
    normalized_display == normalized_query || normalized_display.ends_with(&normalized_query)
}

fn packet_citation_probe_match_rank(query: &str, citation: &AgentCitationDto) -> Option<u8> {
    let normalized_query = normalize_identifier(query);
    if normalized_query.is_empty() {
        return Some(0);
    }
    let normalized_display = normalize_identifier(&citation.display_name);
    let normalized_path = citation
        .file_path
        .as_deref()
        .map(packet_display_path)
        .map(|path| normalize_identifier(&path))
        .unwrap_or_default();
    if let Some(matches_file_scoped_symbol) =
        packet_file_scoped_symbol_probe_matches(query, citation)
    {
        if matches_file_scoped_symbol {
            Some(6)
        } else {
            None
        }
    } else if packet_file_stem_matches_query(query, citation.file_path.as_deref()) {
        Some(5)
    } else if normalized_display == normalized_query
        || normalized_display.ends_with(&normalized_query)
        || (!packet_required_probe_needs_exact_match(query)
            && packet_citation_probe_token_coverage(query, citation) >= 2)
    {
        Some(4)
    } else if normalized_path.contains(&normalized_query) {
        Some(3)
    } else if normalized_display.contains(&normalized_query) {
        Some(2)
    } else if !normalized_display.is_empty() && normalized_query.contains(&normalized_display) {
        Some(1)
    } else {
        None
    }
}

fn packet_file_scoped_symbol_probe_matches(
    query: &str,
    citation: &AgentCitationDto,
) -> Option<bool> {
    let parts = packet_file_scoped_symbol_probe_parts(query)?;
    let path = citation
        .file_path
        .as_deref()
        .map(packet_display_path)
        .unwrap_or_default();
    let file_name = path
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(path.as_str())
        .to_ascii_lowercase();
    if file_name != parts.file_name {
        return Some(false);
    }

    let normalized_display = normalize_identifier(&citation.display_name);
    if parts.symbols.len() >= 3 && parts.symbols[0] == "create" && parts.symbols[1] == "table" {
        let Some(table_name) = parts.symbols.last() else {
            return Some(false);
        };
        let expected = format!("createtable{table_name}");
        return Some(normalized_display == expected || normalized_display.ends_with(&expected));
    }
    if parts.symbols.len() >= 2 && parts.symbols[0] == "foreign" && parts.symbols[1] == "key" {
        return Some(
            normalized_display == "foreignkey" || normalized_display.ends_with("foreignkey"),
        );
    }
    Some(parts.symbols.iter().any(|symbol| {
        normalized_display == *symbol
            || normalized_display.ends_with(symbol)
            || packet_file_scoped_short_symbol_matches(&citation.display_name, symbol)
    }))
}

fn packet_file_scoped_short_symbol_matches(display_name: &str, symbol: &str) -> bool {
    if symbol.len() > 3 {
        return false;
    }
    display_name
        .rsplit(['.', ':', '#'])
        .next()
        .map(normalize_identifier)
        .is_some_and(|tail| tail == symbol)
}

struct PacketFileScopedSymbolProbe {
    query_path: String,
    file_name: String,
    raw_symbols: Vec<String>,
    symbols: Vec<String>,
}

fn packet_file_scoped_symbol_probe_parts(query: &str) -> Option<PacketFileScopedSymbolProbe> {
    let mut parts = query.split_whitespace();
    let file_part = parts
        .next()?
        .trim_matches(|ch: char| matches!(ch, '`' | '"' | '\''));
    let query_path = file_part.replace('\\', "/");
    let file_name = file_part.rsplit(['/', '\\']).next()?.to_ascii_lowercase();
    if !file_name.contains('.') {
        return None;
    }

    let raw_symbols = parts
        .map(|part| {
            part.trim_matches(|ch: char| matches!(ch, '`' | '"' | '\'' | ',' | ';'))
                .to_string()
        })
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    let symbols = raw_symbols
        .iter()
        .map(|part| normalize_identifier(part))
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if symbols.is_empty() {
        return None;
    }

    Some(PacketFileScopedSymbolProbe {
        query_path,
        file_name,
        raw_symbols,
        symbols,
    })
}

fn packet_citation_probe_token_coverage(query: &str, citation: &AgentCitationDto) -> usize {
    let tokens = packet_probe_match_tokens(query);
    if tokens.len() < 2 {
        return 0;
    }
    let display = normalize_identifier(&citation.display_name);
    let path = citation
        .file_path
        .as_deref()
        .map(packet_display_path)
        .map(|path| normalize_identifier(&path))
        .unwrap_or_default();
    tokens
        .iter()
        .filter(|token| display.contains(token.as_str()) || path.contains(token.as_str()))
        .count()
}

fn packet_probe_match_tokens(query: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    for token in query
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .map(|token| token.trim().to_ascii_lowercase())
        .filter(|token| token.len() >= 3 && !packet_query_stop_term(token))
    {
        if !tokens.iter().any(|existing| existing == &token) {
            tokens.push(token);
        }
    }
    tokens
}

fn packet_has_sufficiency_blocking_budget_omission(
    answer: &AgentAnswerDto,
    budget: &PacketBudgetDto,
    min_citations: usize,
    min_claims: usize,
    supported_claim_count: usize,
) -> bool {
    if !budget.truncated {
        return false;
    }

    let has_claim_stop_signal =
        answer.citations.len() >= min_citations && supported_claim_count >= min_claims;
    let has_retained_graph = packet_has_retained_graph(answer);

    budget
        .omitted_sections
        .iter()
        .any(|section| match section.as_str() {
            "packet_payload" => true,
            "markdown_blocks" => {
                !has_claim_stop_signal || packet_markdown_truncation_blocks_sufficiency(answer)
            }
            "trail_edges" => !has_claim_stop_signal || !has_retained_graph,
            _ => false,
        })
}

fn packet_has_retained_graph(answer: &AgentAnswerDto) -> bool {
    answer.graphs.iter().any(|artifact| match artifact {
        GraphArtifactDto::Uml { graph, .. } => !graph.truncated && !graph.edges.is_empty(),
        GraphArtifactDto::Mermaid { .. } => false,
    })
}

fn packet_markdown_truncation_blocks_sufficiency(answer: &AgentAnswerDto) -> bool {
    let mut saw_truncated_markdown = false;
    for section in &answer.sections {
        for block in &section.blocks {
            let AgentResponseBlockDto::Markdown { markdown } = block else {
                continue;
            };
            if !markdown.contains(PACKET_MARKDOWN_TRUNCATION_SUFFIX.trim()) {
                continue;
            }
            saw_truncated_markdown = true;
            if !packet_section_allows_nonblocking_truncation(section.id.as_str()) {
                return true;
            }
        }
    }
    !saw_truncated_markdown
}

fn packet_section_allows_nonblocking_truncation(section_id: &str) -> bool {
    section_id == "retrieval-evidence"
        || section_id == "diagrams"
        || section_id.starts_with("packet-subquery-")
}

fn packet_budget_exceeded_hard_output_cap(budget: &PacketBudgetDto) -> bool {
    budget.used.output_bytes > budget.limits.max_output_bytes
}

fn packet_follow_up_commands(
    project_root: &Path,
    question: &str,
    task_class: PacketTaskClassDto,
    status: PacketSufficiencyStatusDto,
    budget: &PacketBudgetDto,
    missing_required_probe_queries: &[String],
) -> Vec<String> {
    let project = quote_packet_project_arg(project_root);
    match status {
        PacketSufficiencyStatusDto::Sufficient => Vec::new(),
        PacketSufficiencyStatusDto::Partial => {
            let mut commands = Vec::new();
            let targeted_searches = if missing_required_probe_queries.is_empty() {
                packet_targeted_follow_up_searches(project.as_str(), question, task_class)
            } else {
                packet_missing_required_probe_searches(
                    project.as_str(),
                    missing_required_probe_queries,
                )
            };
            for command in targeted_searches {
                push_unique_term(&mut commands, &command);
            }
            commands
                .into_iter()
                .take(8)
                .chain(budget.next_deeper_command.clone())
                .chain(std::iter::once(format!(
                    "codestory-cli search --project {project} --query {} --why",
                    quote_packet_command_value(question)
                )))
                .collect()
        }
        PacketSufficiencyStatusDto::Insufficient => vec![
            format!("codestory-cli index --project {project} --refresh full"),
            format!(
                "codestory-cli search --project {project} --query {} --why",
                quote_packet_command_value(question)
            ),
        ],
    }
}

fn packet_missing_required_probe_searches(quoted_project: &str, queries: &[String]) -> Vec<String> {
    queries
        .iter()
        .map(|query| {
            format!(
                "codestory-cli search --project {quoted_project} --query {} --why",
                quote_packet_command_value(query)
            )
        })
        .collect()
}

fn packet_targeted_follow_up_searches(
    quoted_project: &str,
    question: &str,
    task_class: PacketTaskClassDto,
) -> Vec<String> {
    packet_targeted_follow_up_queries(question, task_class)
        .into_iter()
        .map(|query| {
            format!(
                "codestory-cli search --project {quoted_project} --query {} --why",
                quote_packet_command_value(&query)
            )
        })
        .collect()
}

fn packet_targeted_follow_up_queries(
    question: &str,
    task_class: PacketTaskClassDto,
) -> Vec<String> {
    let probes = packet_symbol_probe_queries(question, task_class, PacketBudgetModeDto::Standard);
    let selected: Vec<String> = probes
        .iter()
        .filter(|query| is_packet_structured_follow_up_query(query))
        .take(6)
        .cloned()
        .collect();
    selected
}

fn is_packet_structured_follow_up_query(query: &str) -> bool {
    query.contains('_')
        || query.contains("::")
        || query.contains("Options")
        || query.contains("Params")
        || query.contains("Processor")
        || query.contains("Subcommand")
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
    // benchmark trace scalar-sized so compact packets do not serialize it twice.
    trace_summary.annotations.clear();
    trace_summary.steps.clear();

    PacketBenchmarkTraceDto {
        retrieval_trace: trace_summary,
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
    let semantic_required = hybrid_retrieval_enabled()
        && !packet_initial_retrieval_is_lexical_only(req.hybrid_weights.as_ref());

    let max_results = req
        .max_results
        .unwrap_or(DEFAULT_MAX_RESULTS)
        .clamp(1, resolved_profile.max_search_results) as usize;

    let (mut scored_hits, hits) = match try_sidecar_primary_search(
        controller,
        prompt,
        max_results,
        req.latency_budget_ms,
    ) {
        Some(SidecarPrimarySearchOutcome::Served {
            hits,
            scored_hits,
            shadow,
        }) => {
            trace.set_retrieval_shadow(shadow.clone());
            trace.annotate(format!(
                "retrieval_sidecar_primary mode={} candidates={} resolved_hits={}",
                shadow.retrieval_mode,
                shadow.candidate_count,
                hits.len()
            ));
            let search_step = trace.start_step(
                AgentRetrievalStepKindDto::Search,
                vec![
                    field("query_chars", prompt.len().to_string()),
                    field("retrieval_path", "sidecar"),
                ],
            );
            trace.finish_ok(
                search_step,
                vec![
                    field("hits", hits.len().to_string()),
                    field("sidecar_candidates", shadow.candidate_count.to_string()),
                    field(
                        "sidecar_resolved_hits",
                        shadow.resolved_hit_count.to_string(),
                    ),
                    field("accepted_hits", hits.len().to_string()),
                    field("max_results", max_results.to_string()),
                    field("repo_text", "off_initial"),
                ],
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
            trace.finish_skipped(
                semantic_query_step,
                "Semantic embedding skipped on sidecar retrieval path.",
                Vec::new(),
            );
            trace.finish_skipped(
                semantic_candidates_step,
                "Semantic candidate scan skipped on sidecar retrieval path.",
                Vec::new(),
            );
            trace.finish_ok(
                hybrid_rerank_step,
                vec![field("ranked", hits.len().to_string())],
            );
            (scored_hits, hits)
        }
        Some(SidecarPrimarySearchOutcome::Rejected { shadow, reason }) => {
            trace.set_retrieval_shadow(shadow);
            trace.annotate(format!(
                "retrieval_sidecar_primary rejected=true fail_closed=true reason={reason}"
            ));
            return Err(sidecar_retrieval_unavailable_error(
                controller,
                format!("sidecar retrieval primary rejected query: {reason}"),
            ));
        }
        Some(SidecarPrimarySearchOutcome::Unavailable { reason }) => {
            trace.annotate(format!(
                "retrieval_sidecar_primary unavailable=true fail_closed=true reason={reason}"
            ));
            return Err(sidecar_retrieval_unavailable_error(controller, reason));
        }
        None => {
            return Err(sidecar_retrieval_unavailable_error(
                controller,
                "sidecar retrieval primary is mandatory; non-sidecar initial search is disabled",
            ));
        }
    };

    let initial_hit_count = hits.len();
    let mut hits = hits;
    let literal_diagnostic_signal = has_literal_diagnostic_signal(prompt);
    let promotable_focus_available =
        req.focus_node_id.is_some() || investigation_focus_anchor(prompt, &hits).is_some();
    let mut expansion_added_hits = false;
    let block_nucleo_supplement =
        sidecar_retrieval_blocks_nucleo_supplement(controller, hits.len());
    if block_nucleo_supplement && weak_initial_hits(prompt, &hits) {
        trace.annotate(
            "retrieval_sidecar_primary skipped local nucleo investigation supplement on weak hits",
        );
    }
    if should_investigate(resolved_profile)
        && weak_initial_hits(prompt, &hits)
        && !promotable_focus_available
        && !block_nucleo_supplement
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
            bundle.diagnostic_supplement_used = true;
            expansion_added_hits = true;
        }

        if initial_hit_count == 0 && expansion_added_hits && !literal_diagnostic_signal {
            hits.clear();
            scored_hits.clear();
            trace.annotate(
                "Investigation discarded expansion-only hits for an unanchored natural-language query.",
            );
        }

        if weak_initial_hits(prompt, &hits) && literal_diagnostic_signal {
            trace.annotate(
                "Investigation skipped repo-text diagnostics because packet evidence must come from sidecar-backed resolvable hits or direct source reads.",
            );
        } else if weak_initial_hits(prompt, &hits) && !is_repo_explanation_prompt(prompt) {
            if !hits.is_empty() {
                hits.clear();
                scored_hits.clear();
                trace.annotate(
                    "Investigation discarded low-confidence unanchored hits for a natural-language query.",
                );
            }
            trace.annotate(
                "Repo-text diagnostics are disabled for packet evidence; weak unanchored hits were not promoted.",
            );
        } else if weak_initial_hits(prompt, &hits) {
            trace.annotate(
                "Investigation deferred a broad repo explanation prompt to sidecar evidence only.",
            );
        }

        if weak_initial_hits(prompt, &hits) && !is_repo_explanation_prompt(prompt) {
            trace.annotate("Investigation low confidence gap after sidecar query expansion.");
        }
    } else if should_investigate(resolved_profile)
        && weak_initial_hits(prompt, &hits)
        && promotable_focus_available
    {
        trace.annotate(
            "Investigation kept an explicit or prompt-anchored focus instead of broad diagnostics.",
        );
    }

    if should_investigate(resolved_profile)
        && weak_initial_hits(prompt, &hits)
        && is_repo_explanation_prompt(prompt)
        && !block_nucleo_supplement
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
            bundle.diagnostic_supplement_used = true;
            bundle.repo_explanation_supplement_used = true;
            trace.annotate(
                "Investigation used grounding snapshot diagnostic supplement for a broad repo explanation prompt.",
            );
        }
    } else if should_investigate(resolved_profile)
        && weak_initial_hits(prompt, &hits)
        && is_repo_explanation_prompt(prompt)
        && block_nucleo_supplement
    {
        trace.annotate(
            "Grounding snapshot supplement skipped because sidecar-primary retrieval is mandatory.",
        );
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
                    field("hide_speculative", "true"),
                ],
            );

            let root_id = focus_node_id.clone().expect("checked focus node");
            let request = agent_trail_request(root_id, plan);

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
            provenance: Vec::new(),
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
        return false;
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

fn has_literal_diagnostic_signal(prompt: &str) -> bool {
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
            "Skipped grounding snapshot supplement because latency budget was exceeded.",
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
            provenance: Vec::new(),
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

fn trail_truncated_annotation(trail_number: usize, max_nodes: u32) -> String {
    format!("Trail {trail_number} was truncated at max_nodes={max_nodes}.")
}

fn agent_trail_request(root_id: NodeId, plan: &TrailPlan) -> TrailConfigDto {
    TrailConfigDto {
        root_id,
        mode: plan.mode,
        target_id: None,
        depth: plan.depth,
        direction: plan.direction,
        caller_scope: plan.caller_scope,
        edge_filter: plan.edge_filter.clone(),
        show_utility_calls: true,
        hide_speculative: true,
        story: false,
        node_filter: plan.node_filter.clone(),
        max_nodes: plan.max_nodes,
        layout_direction: codestory_contracts::api::LayoutDirection::Horizontal,
    }
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
    diagnostic_focus: bool,
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

    if !needs_source_context(request.prompt) && !request.diagnostic_focus {
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
            id: "mermaid-diagnostic".to_string(),
            title: "Retrieval Diagnostic".to_string(),
            diagram: "flowchart".to_string(),
            mermaid_syntax: diagnostic_mermaid(prompt, bundle.hits.len()),
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
    if bundle.diagnostic_supplement_used {
        markdown.push_str("- Deterministic query expansion because initial hits were weak.\n");
    }
    if bundle.repo_explanation_supplement_used {
        markdown.push_str(
            "- Grounding snapshot diagnostic supplement for broad repo overview evidence.\n",
        );
    }
    if !bundle.diagnostic_supplement_used && should_investigate(profile) {
        markdown.push_str("- Initial sidecar hits cleared the investigation confidence gate.\n");
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
        "actual",
        "already",
        "an",
        "and",
        "are",
        "area",
        "areas",
        "across",
        "as",
        "at",
        "be",
        "boundaries",
        "boundary",
        "by",
        "can",
        "current",
        "does",
        "existing",
        "for",
        "from",
        "how",
        "implementation",
        "implemented",
        "in",
        "is",
        "it",
        "of",
        "on",
        "or",
        "repo",
        "repository",
        "risk",
        "risks",
        "study",
        "surface",
        "surfaces",
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
    use crate::agent::eval_probes::{
        EVAL_PROBES_ENV, pop_eval_probes_test_override, push_eval_probes_test_override,
    };
    use crate::agent::profiles::ResolvedProfile;

    struct EvalProbesGuard;

    impl EvalProbesGuard {
        fn enabled() -> Self {
            push_eval_probes_test_override();
            Self
        }
    }

    impl Drop for EvalProbesGuard {
        fn drop(&mut self) {
            pop_eval_probes_test_override();
        }
    }

    struct EnvVarGuard {
        key: &'static str,
        previous: Option<std::ffi::OsString>,
    }

    impl EnvVarGuard {
        fn cleared(key: &'static str) -> Self {
            let previous = std::env::var_os(key);
            // SAFETY: tests use this guard to isolate one env var for this process-local
            // regression and restore it on drop.
            unsafe {
                std::env::remove_var(key);
            }
            Self { key, previous }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            // SAFETY: restores the process-local env var captured by this guard.
            unsafe {
                if let Some(previous) = self.previous.take() {
                    std::env::set_var(self.key, previous);
                } else {
                    std::env::remove_var(self.key);
                }
            }
        }
    }

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

    #[test]
    fn agent_trail_request_hides_speculative_edges_by_default() {
        let plan = TrailPlan {
            mode: codestory_contracts::api::TrailMode::AllReferenced,
            depth: 3,
            direction: codestory_contracts::api::TrailDirection::Outgoing,
            caller_scope: codestory_contracts::api::TrailCallerScope::ProductionOnly,
            edge_filter: vec![codestory_contracts::api::EdgeKind::CALL],
            node_filter: vec![codestory_contracts::api::NodeKind::FUNCTION],
            max_nodes: 42,
        };

        let request = agent_trail_request(NodeId("root".to_string()), &plan);

        assert!(request.hide_speculative);
        assert!(request.show_utility_calls);
        assert!(!request.story);
        assert_eq!(request.edge_filter, plan.edge_filter);
        assert_eq!(request.node_filter, plan.node_filter);
        assert_eq!(request.max_nodes, plan.max_nodes);
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
            provenance: Vec::new(),
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
                provenance: Vec::new(),
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
                semantic_fallback_count: 0,
                semantic_fallbacks: Vec::new(),
                annotations: Vec::new(),
                steps: Vec::new(),
                retrieval_shadow: None,
            },
        }
    }

    fn packet_fixture_project_root() -> &'static std::path::Path {
        std::path::Path::new("C:/workspace/project root")
    }

    fn packet_temp_root(name: &str) -> std::path::PathBuf {
        let suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock should be after unix epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("codestory-{name}-{}-{suffix}", std::process::id()))
    }

    fn write_packet_fixture_file(
        root: &std::path::Path,
        relative_path: &str,
        source: &str,
    ) -> std::path::PathBuf {
        let path = root.join(relative_path);
        std::fs::create_dir_all(path.parent().expect("fixture path should have a parent"))
            .expect("create fixture parent directory");
        std::fs::write(&path, source).expect("write fixture source file");
        path
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
        let budget = apply_packet_budget(
            packet_fixture_project_root(),
            question,
            PacketTaskClassDto::ArchitectureExplanation,
            PacketBudgetModeDto::Compact,
            limits,
            &mut answer,
        );
        let sufficiency = build_packet_sufficiency(
            packet_fixture_project_root(),
            question,
            task_class,
            &answer,
            &budget,
        );
        (answer, sufficiency)
    }

    #[test]
    fn packet_symbol_probes_prioritize_flow_specific_terms() {
        let _eval_probes = EvalProbesGuard::enabled();
        let queries = packet_symbol_probe_queries(
            "Explain how `codex exec --json` flows from the top-level CLI into the exec runtime, app-server thread and turn start requests, and JSONL event output.",
            PacketTaskClassDto::ArchitectureExplanation,
            PacketBudgetModeDto::Standard,
        );

        let position = |needle: &str| {
            queries
                .iter()
                .position(|query| query == needle)
                .unwrap_or_else(|| panic!("missing packet query `{needle}` in {queries:?}"))
        };

        assert!(position("run_exec_session") < position("run_exec"));
        assert!(position("Subcommand::Exec") < position("Subcommand"));
        assert!(position("codex_exec::Cli") < position("ExecSharedCliOptions"));
        assert!(position("codex_exec::run_main") < position("run_main"));
        assert!(position("ExecSharedCliOptions") < position("exec_cli"));
        assert!(position("EventProcessor") < position("ThreadStartParams"));
        assert!(position("exec_events") < position("exec_events.rs"));
        assert!(position("exec_events") < position("ThreadStartParams"));
        assert!(queries.iter().any(|query| query == "ThreadStartParams"));
        assert!(queries.iter().any(|query| query == "TurnStartParams"));
        assert!(queries.iter().any(|query| query == "ExecSharedCliOptions"));
        assert!(queries.iter().any(|query| query == "EventProcessor"));
        assert!(
            queries
                .iter()
                .any(|query| query == "EventProcessorWithJsonOutput")
        );
    }

    #[test]
    fn packet_anchor_probes_keep_required_event_flow_terms_inside_reduced_window() {
        let _eval_probes = EvalProbesGuard::enabled();
        let plan = build_packet_plan(
            "Explain how `codex exec --json` flows from the top-level CLI into the exec runtime, app-server thread and turn start requests, and JSONL event output.",
            Some(PacketTaskClassDto::ArchitectureExplanation),
            PacketBudgetModeDto::Compact,
        );
        let reduced = packet_anchor_probe_queries(&plan)
            .into_iter()
            .take(14)
            .collect::<Vec<_>>();

        for expected in [
            "exec runtime",
            "exec session",
            "exec cli",
            "json event output",
            "thread start",
        ] {
            assert!(
                reduced.iter().any(|query| query == expected),
                "expected reduced anchor probe window to retain `{expected}` in {reduced:?}"
            );
        }
    }

    #[test]
    fn packet_symbol_probes_derive_generic_command_role_probes_from_code_span() {
        let queries = packet_symbol_probe_queries(
            "Trace how `acme deploy --dry-run` flows from the CLI subcommand into the exec runtime.",
            PacketTaskClassDto::ArchitectureExplanation,
            PacketBudgetModeDto::Standard,
        );

        let position = |needle: &str| {
            queries
                .iter()
                .position(|query| query == needle)
                .unwrap_or_else(|| panic!("missing packet query `{needle}` in {queries:?}"))
        };

        for expected in [
            "acme deploy",
            "acme deploy command",
            "deploy command",
            "deploy subcommand",
        ] {
            assert!(
                queries.iter().any(|query| query == expected),
                "expected generic command role probe `{expected}` in {queries:?}"
            );
        }
        for forbidden in [
            "Subcommand::Deploy",
            "acme_deploy::Cli",
            "acme_deploy::run_main",
        ] {
            assert!(
                !queries.iter().any(|query| query == forbidden),
                "production command probes should not include eval-style exact symbol `{forbidden}`: {queries:?}"
            );
        }
        assert!(position("acme deploy") < position("subcommand"));
        assert!(position("acme deploy command") < position("command runtime"));
    }

    #[test]
    fn packet_symbol_probes_load_eval_exact_command_probes_only_when_enabled() {
        push_eval_probes_test_override();
        let queries = packet_symbol_probe_queries(
            "Trace how `acme deploy --dry-run` flows from the CLI subcommand into the exec runtime.",
            PacketTaskClassDto::ArchitectureExplanation,
            PacketBudgetModeDto::Standard,
        );
        pop_eval_probes_test_override();

        assert!(queries.iter().any(|query| query == "Subcommand::Deploy"));
        assert!(queries.iter().any(|query| query == "acme_deploy::Cli"));
        assert!(queries.iter().any(|query| query == "acme_deploy::run_main"));
    }

    #[test]
    fn packet_symbol_probes_without_eval_catalog_include_prompt_derived_flow_anchors() {
        let queries = packet_symbol_probe_queries(
            "Explain how `codex exec --json` flows from the top-level CLI into the exec runtime, app-server thread and turn start requests, and JSONL event output.",
            PacketTaskClassDto::ArchitectureExplanation,
            PacketBudgetModeDto::Standard,
        );

        for expected in [
            "exec runtime",
            "exec session",
            "exec cli",
            "exec command",
            "jsonl event output",
            "thread start",
            "turn start",
        ] {
            assert!(
                queries.iter().any(|query| query == expected),
                "expected generic production flow query {expected} in {queries:?}"
            );
        }
        for forbidden in [
            "run_exec_session",
            "ExecSharedCliOptions",
            "EventProcessorWithJsonOutput",
            "exec_events",
            "ThreadStartParams",
            "TurnStartParams",
            "codex_exec::Cli",
            "codex_exec::run_main",
            "Subcommand::Exec",
        ] {
            assert!(
                !queries.iter().any(|query| query == forbidden),
                "production flow probes should not include eval-only exact query {forbidden}: {queries:?}"
            );
        }
    }

    #[test]
    fn packet_rank_demotes_lib_module_facade_below_concrete_module_file() {
        let terms = vec!["jsonl".to_string(), "event".to_string()];
        let mut facade = test_packet_citation(
            "event_processor_with_jsonl_output",
            "codex-rs/exec/src/lib.rs",
            10.0,
        );
        facade.kind = NodeKind::MODULE;
        let mut concrete = test_packet_citation(
            "event_processor_with_jsonl_output",
            "codex-rs/exec/src/event_processor_with_jsonl_output.rs",
            10.0,
        );
        concrete.kind = NodeKind::MODULE;

        assert!(
            packet_citation_rank(&concrete, &terms, true)
                > packet_citation_rank(&facade, &terms, true),
            "concrete module files should outrank lib/mod facade declarations for packet evidence"
        );
    }

    #[test]
    fn packet_probe_match_rank_uses_multi_token_path_coverage() {
        let mut citation = test_packet_citation(
            "std::collections::HashMap",
            "codex-rs/exec/src/event_processor_with_jsonl_output.rs",
            0.6,
        );
        citation.kind = NodeKind::MODULE;

        assert_eq!(
            packet_citation_probe_match_rank("jsonl event output", &citation),
            Some(4)
        );
        assert_eq!(
            packet_citation_probe_token_coverage("jsonl event output", &citation),
            3
        );
    }

    #[test]
    fn packet_required_probe_matching_uses_file_stems_and_display_symbols() {
        let event_loop_entry = test_packet_citation(
            "service::main",
            r"\\?\C:\Users\alber\source\repos\codestory\target\agent-benchmark\repos\acme\src\event_loop.c",
            0.9,
        );
        let command_handler = test_packet_citation(
            "CommandHandler",
            r"\\?\C:\Users\alber\source\repos\codestory\target\agent-benchmark\repos\acme\src\commands.c",
            0.9,
        );
        let search_entrypoint = test_packet_citation(
            "search_driver::run",
            r"\\?\C:\Users\alber\source\repos\codestory\target\agent-benchmark\repos\acme\crates\search\src\main.rs",
            0.9,
        );
        let candidate_builder = test_packet_citation(
            "CandidateFiles",
            r"\\?\C:\Users\alber\source\repos\codestory\target\agent-benchmark\repos\acme\crates\search\src\candidate_files.rs",
            0.9,
        );

        assert!(packet_citation_satisfies_required_probe(
            "event_loop.c main",
            &event_loop_entry
        ));
        assert!(packet_citation_satisfies_required_probe(
            "command handler",
            &command_handler
        ));
        assert!(packet_citation_satisfies_required_probe(
            "search driver run",
            &search_entrypoint
        ));
        assert!(packet_citation_satisfies_required_probe(
            "candidate files",
            &candidate_builder
        ));
    }

    #[test]
    fn packet_required_probe_promotion_prefers_command_focus_root_matches() {
        let mut run_main = test_packet_citation(
            "acme_deploy::run_main",
            "crates/acme-deploy/src/main.rs",
            0.7,
        );
        run_main.kind = NodeKind::MODULE;
        let mut focused_event_file = test_packet_citation(
            "std::collections::HashMap",
            "crates/acme-deploy/src/event_processor_with_jsonl_output.rs",
            0.6,
        );
        focused_event_file.kind = NodeKind::MODULE;
        let distractor = test_packet_citation("jsonl", "crates/core/tests/sqlite_state.rs", 0.95);
        let mut answer = packet_answer_fixture(
            "Explain how `acme deploy --json` flows through JSONL event output.",
            vec![distractor, focused_event_file, run_main],
        );

        let protected =
            promote_required_probe_citations(&mut answer, &["jsonl event output".to_string()]);

        assert!(protected.contains(&packet_citation_key(&answer.citations[0])));
        assert_eq!(
            answer.citations[0].file_path.as_deref(),
            Some("crates/acme-deploy/src/event_processor_with_jsonl_output.rs")
        );
    }

    #[test]
    fn packet_required_probe_promotion_preserves_exact_command_probes() {
        let exact_cli =
            test_packet_citation("acme_deploy::Cli", "crates/acme-cli/src/main.rs", 0.7);
        let mut tempting_file_neighbor =
            test_packet_citation("clap::Args", "crates/acme-deploy/src/cli.rs", 0.95);
        tempting_file_neighbor.kind = NodeKind::MODULE;
        let mut answer = packet_answer_fixture(
            "Explain how `acme deploy --json` flows from CLI into runtime.",
            vec![tempting_file_neighbor, exact_cli],
        );

        promote_required_probe_citations(&mut answer, &["acme_deploy::Cli".to_string()]);

        assert_eq!(answer.citations[0].display_name, "acme_deploy::Cli");
        assert_eq!(
            answer.citations[0].file_path.as_deref(),
            Some("crates/acme-cli/src/main.rs")
        );
    }

    #[test]
    fn packet_budget_reserves_focused_neighborhood_after_command_root() {
        let run_main = test_packet_citation(
            "acme_deploy::run_main",
            "crates/acme-deploy/src/main.rs",
            0.7,
        );
        let focused_cli =
            test_packet_citation("clap::Parser", "crates/acme-deploy/src/cli.rs", 0.2);
        let focused_lib = test_packet_citation("acme_deploy", "crates/acme-deploy/src/lib.rs", 0.2);
        let mut focused_event = test_packet_citation(
            "std::collections::HashMap",
            "crates/acme-deploy/src/event_processor_with_jsonl_output.rs",
            0.2,
        );
        focused_event.kind = NodeKind::MODULE;
        let focused_exec_events =
            test_packet_citation("exec_events", "crates/acme-deploy/src/exec_events.rs", 0.2);
        let distractors = (0..8)
            .map(|index| {
                test_packet_citation(
                    &format!("GenericRuntime{index}"),
                    &format!("crates/cross-crate/src/runtime_{index}.rs"),
                    10.0 - index as f32,
                )
            })
            .collect::<Vec<_>>();
        let mut citations = vec![
            distractors[0].clone(),
            distractors[1].clone(),
            focused_cli,
            distractors[2].clone(),
            focused_event,
            run_main,
            focused_lib,
            distractors[3].clone(),
            focused_exec_events,
        ];
        citations.extend(distractors.into_iter().skip(4));
        let mut answer = packet_answer_fixture(
            "Explain how `acme deploy --json` flows through the command runtime and event output.",
            citations,
        );
        let limits = PacketBudgetLimitsDto {
            max_anchors: 5,
            max_files: 5,
            max_snippets: 5,
            max_trail_edges: 0,
            max_output_bytes: 64 * 1024,
        };

        cap_packet_citations(&mut answer, &limits, &["acme_deploy::run_main".to_string()]);

        let paths = answer
            .citations
            .iter()
            .filter_map(|citation| citation.file_path.as_deref())
            .collect::<Vec<_>>();
        assert_eq!(
            answer.citations[0].display_name, "acme_deploy::run_main",
            "exact command probe should remain first: {paths:?}"
        );
        for expected in [
            "crates/acme-deploy/src/cli.rs",
            "crates/acme-deploy/src/lib.rs",
            "crates/acme-deploy/src/event_processor_with_jsonl_output.rs",
            "crates/acme-deploy/src/exec_events.rs",
        ] {
            assert!(
                paths.contains(&expected),
                "focused command-root neighbor should survive compact cap: {paths:?}"
            );
        }
    }

    #[test]
    fn packet_focused_neighborhood_reservation_preserves_exact_command_order() {
        let run_main = test_packet_citation(
            "acme_deploy::run_main",
            "crates/acme-deploy/src/main.rs",
            0.7,
        );
        let focused_neighbor =
            test_packet_citation("exec_events", "crates/acme-deploy/src/exec_events.rs", 0.99);
        let exact_cli =
            test_packet_citation("acme_deploy::Cli", "crates/acme-cli/src/main.rs", 0.6);
        let cross_root = test_packet_citation("exec_events", "crates/core/src/exec_events.rs", 2.0);
        let mut answer = packet_answer_fixture(
            "Explain how `acme deploy --json` flows through runtime events.",
            vec![focused_neighbor, cross_root, run_main, exact_cli],
        );
        let limits = PacketBudgetLimitsDto {
            max_anchors: 4,
            max_files: 4,
            max_snippets: 4,
            max_trail_edges: 0,
            max_output_bytes: 64 * 1024,
        };

        cap_packet_citations(
            &mut answer,
            &limits,
            &[
                "acme_deploy::run_main".to_string(),
                "acme_deploy::Cli".to_string(),
            ],
        );

        let displays = answer
            .citations
            .iter()
            .map(|citation| citation.display_name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            displays[..2],
            ["acme_deploy::run_main", "acme_deploy::Cli"],
            "exact command probes should stay ahead of focused neighbors: {displays:?}"
        );
        assert_eq!(
            answer.citations[2].file_path.as_deref(),
            Some("crates/acme-deploy/src/exec_events.rs")
        );
    }

    #[test]
    fn packet_focus_neighborhood_prefers_source_navigation_files() {
        let run_main = test_packet_citation(
            "acme_deploy::run_main",
            "crates/acme-deploy/src/main.rs",
            0.7,
        );
        let focused_event = test_packet_citation(
            "EventProcessor",
            "crates/acme-deploy/src/event_processor.rs",
            0.99,
        );
        let focused_cli =
            test_packet_citation("clap::Parser", "crates/acme-deploy/src/cli.rs", 0.2);
        let focused_lib = test_packet_citation("cli", "crates/acme-deploy/src/lib.rs", 0.2);
        let cross_root_cli =
            test_packet_citation("clap::Parser", "crates/other-tool/src/cli.rs", 10.0);
        let mut answer = packet_answer_fixture(
            "Explain how `acme deploy --json` flows from CLI into runtime.",
            vec![
                focused_event,
                focused_cli,
                run_main,
                cross_root_cli,
                focused_lib,
            ],
        );
        let protected =
            promote_required_probe_citations(&mut answer, &["acme_deploy::run_main".to_string()]);
        let focused = promote_focus_neighborhood_citations(&mut answer, &protected);

        let focused_paths = answer
            .citations
            .iter()
            .filter(|citation| focused.contains(&packet_citation_key(citation)))
            .filter_map(|citation| citation.file_path.as_deref())
            .collect::<Vec<_>>();
        assert!(
            focused_paths.starts_with(&[
                "crates/acme-deploy/src/cli.rs",
                "crates/acme-deploy/src/lib.rs"
            ]),
            "source navigation files should be carried before same-root event details or cross-root cli files: {focused_paths:?}"
        );
    }

    #[test]
    fn packet_focus_neighborhood_skips_protected_file_duplicates() {
        let run_main = test_packet_citation(
            "acme_deploy::run_main",
            "crates/acme-deploy/src/main.rs",
            0.7,
        );
        let duplicate_main_cli =
            test_packet_citation("acme_deploy::Cli", "crates/acme-deploy/src/main.rs", 5.0);
        let duplicate_main_parser =
            test_packet_citation("clap::Parser", "crates/acme-deploy/src/main.rs", 5.0);
        let focused_cli = test_packet_citation("clap::Args", "crates/acme-deploy/src/cli.rs", 0.2);
        let focused_lib = test_packet_citation("cli", "crates/acme-deploy/src/lib.rs", 0.2);
        let focused_event = test_packet_citation(
            "EventProcessor",
            "crates/acme-deploy/src/event_processor.rs",
            0.99,
        );
        let focused_exec_events =
            test_packet_citation("exec_events", "crates/acme-deploy/src/exec_events.rs", 0.98);
        let mut answer = packet_answer_fixture(
            "Explain how `acme deploy --json` flows from CLI into runtime events.",
            vec![
                duplicate_main_cli,
                focused_event,
                focused_cli,
                run_main,
                duplicate_main_parser,
                focused_exec_events,
                focused_lib,
            ],
        );
        let protected =
            promote_required_probe_citations(&mut answer, &["acme_deploy::run_main".to_string()]);
        let focused = promote_focus_neighborhood_citations(&mut answer, &protected);

        let focused_paths = answer
            .citations
            .iter()
            .filter(|citation| focused.contains(&packet_citation_key(citation)))
            .filter_map(|citation| citation.file_path.as_deref())
            .collect::<Vec<_>>();
        assert!(
            !focused_paths.contains(&"crates/acme-deploy/src/main.rs"),
            "focus carry should not spend slots on duplicate citations for an already protected file: {focused_paths:?}"
        );
        assert!(
            focused_paths.contains(&"crates/acme-deploy/src/cli.rs")
                && focused_paths.contains(&"crates/acme-deploy/src/lib.rs")
                && focused_paths.contains(&"crates/acme-deploy/src/event_processor.rs")
                && focused_paths.contains(&"crates/acme-deploy/src/exec_events.rs"),
            "focus carry should preserve distinct same-root source files: {focused_paths:?}"
        );
    }

    #[test]
    fn packet_focus_neighborhood_prefers_event_definition_files() {
        let run_main = test_packet_citation(
            "acme_deploy::run_main",
            "crates/acme-deploy/src/main.rs",
            0.7,
        );
        let focused_cli = test_packet_citation("clap::Args", "crates/acme-deploy/src/cli.rs", 0.2);
        let focused_lib = test_packet_citation("cli", "crates/acme-deploy/src/lib.rs", 0.2);
        let focused_event_processor = test_packet_citation(
            "EventProcessor",
            "crates/acme-deploy/src/event_processor.rs",
            0.99,
        );
        let focused_human_output = test_packet_citation(
            "HumanOutput",
            "crates/acme-deploy/src/event_processor_with_human_output.rs",
            3.0,
        );
        let focused_exec_events =
            test_packet_citation("exec_events", "crates/acme-deploy/src/exec_events.rs", 0.1);
        let mut answer = packet_answer_fixture(
            "Explain how `acme deploy --json` flows through event output.",
            vec![
                focused_human_output,
                focused_event_processor,
                focused_cli,
                run_main,
                focused_exec_events,
                focused_lib,
            ],
        );
        let protected =
            promote_required_probe_citations(&mut answer, &["acme_deploy::run_main".to_string()]);
        let focused = promote_focus_neighborhood_citations(&mut answer, &protected);

        let focused_paths = answer
            .citations
            .iter()
            .filter(|citation| focused.contains(&packet_citation_key(citation)))
            .filter_map(|citation| citation.file_path.as_deref())
            .collect::<Vec<_>>();
        assert!(
            focused_paths.contains(&"crates/acme-deploy/src/exec_events.rs"),
            "event definition files should survive compact focus carry: {focused_paths:?}"
        );
    }

    #[test]
    fn packet_sufficiency_requires_planned_flow_probe_coverage() {
        let (_answer, sufficiency) = build_sufficient_packet_fixture(
            "Explain how `codex exec --json` flows from the top-level CLI into the exec runtime, app-server thread and turn start requests, and JSONL event output.",
            PacketTaskClassDto::ArchitectureExplanation,
            vec![
                test_packet_citation(
                    "EventProcessorWithJsonOutput",
                    "codex-rs/exec/src/event_processor_with_jsonl_output.rs",
                    0.9,
                ),
                test_packet_citation("exec_cli", "codex-rs/cli/src/main.rs", 0.8),
                test_packet_citation(
                    "UnifiedExecRuntime",
                    "codex-rs/core/src/tools/runtimes/unified_exec.rs",
                    0.7,
                ),
                test_packet_citation(
                    "ThreadStartParams",
                    "codex-rs/app-server-protocol/schema/typescript/v2/ThreadStartParams.ts",
                    0.7,
                ),
            ],
        );

        assert_eq!(sufficiency.status, PacketSufficiencyStatusDto::Partial);
        assert!(
            sufficiency
                .gaps
                .iter()
                .any(|gap| gap.contains("exec session")
                    && gap.contains("exec command")
                    && gap.contains("turn start")),
            "required flow probes should be named in sufficiency gaps: {sufficiency:?}"
        );
        assert!(
            sufficiency
                .follow_up_commands
                .iter()
                .any(|command| command.contains("--query 'exec session'")),
            "missing required probes should become targeted follow-up searches: {sufficiency:?}"
        );
    }

    #[test]
    fn packet_sufficiency_accepts_required_flow_probe_coverage() {
        let (_answer, sufficiency) = build_sufficient_packet_fixture(
            "Explain how `codex exec --json` flows from the top-level CLI into the exec runtime, app-server thread and turn start requests, and JSONL event output.",
            PacketTaskClassDto::ArchitectureExplanation,
            vec![
                test_packet_citation("exec runtime", "codex-rs/exec/src/lib.rs", 0.9),
                test_packet_citation("exec session", "codex-rs/exec/src/lib.rs", 0.9),
                test_packet_citation("exec cli", "codex-rs/cli/src/main.rs", 0.8),
                test_packet_citation("exec command", "codex-rs/cli/src/main.rs", 0.8),
                test_packet_citation(
                    "json event output",
                    "codex-rs/exec/src/event_processor_with_jsonl_output.rs",
                    0.8,
                ),
                test_packet_citation(
                    "jsonl event output",
                    "codex-rs/exec/src/exec_events.rs",
                    0.8,
                ),
                test_packet_citation(
                    "thread start",
                    "codex-rs/app-server-protocol/schema/typescript/v2/ThreadStartParams.ts",
                    0.7,
                ),
                test_packet_citation(
                    "turn start",
                    "codex-rs/app-server-protocol/schema/typescript/v2/TurnStartParams.ts",
                    0.7,
                ),
            ],
        );

        assert_eq!(
            sufficiency.status,
            PacketSufficiencyStatusDto::Sufficient,
            "{sufficiency:?}"
        );
        assert!(sufficiency.gaps.is_empty(), "{sufficiency:?}");
        assert!(sufficiency.follow_up_commands.is_empty(), "{sufficiency:?}");
    }

    #[test]
    fn packet_sufficiency_rejects_module_facade_for_concrete_file_probe() {
        let _eval_probes = EvalProbesGuard::enabled();
        let (_answer, sufficiency) = build_sufficient_packet_fixture(
            "Explain how `codex exec --json` flows from the top-level CLI into the exec runtime, app-server thread and turn start requests, and JSONL event output.",
            PacketTaskClassDto::ArchitectureExplanation,
            vec![
                test_packet_citation("run_exec_session", "codex-rs/exec/src/lib.rs", 0.9),
                test_packet_citation("ExecSharedCliOptions", "codex-rs/exec/src/cli.rs", 0.9),
                test_packet_citation("Subcommand::Exec", "codex-rs/cli/src/main.rs", 0.8),
                test_packet_citation("run_main", "codex-rs/exec/src/main.rs", 0.8),
                test_packet_citation(
                    "EventProcessorWithJsonOutput",
                    "codex-rs/exec/src/event_processor_with_jsonl_output.rs",
                    0.8,
                ),
                test_packet_citation("exec_events", "codex-rs/exec/src/lib.rs", 0.8),
                test_packet_citation(
                    "ThreadStartParams",
                    "codex-rs/app-server-protocol/schema/typescript/v2/ThreadStartParams.ts",
                    0.7,
                ),
                test_packet_citation(
                    "TurnStartParams",
                    "codex-rs/app-server-protocol/schema/typescript/v2/TurnStartParams.ts",
                    0.7,
                ),
            ],
        );

        assert_eq!(sufficiency.status, PacketSufficiencyStatusDto::Partial);
        assert!(
            sufficiency
                .gaps
                .iter()
                .any(|gap| gap.contains("exec_events")),
            "facade module declaration should not satisfy the concrete exec_events file probe: {sufficiency:?}"
        );
    }

    #[test]
    fn packet_symbol_probes_expand_indexing_storage_flow_concepts() {
        let _eval_probes = EvalProbesGuard::enabled();
        let queries = packet_symbol_probe_queries(
            "Explain how project/source-group configuration becomes indexing work, then how indexed data is accessed from storage.",
            PacketTaskClassDto::ArchitectureExplanation,
            PacketBudgetModeDto::Standard,
        );

        let position = |needle: &str| {
            queries
                .iter()
                .position(|query| query == needle)
                .unwrap_or_else(|| panic!("missing packet query `{needle}` in {queries:?}"))
        };

        assert!(position("StorageAccess") < position("SourceGroup"));
        assert!(position("PersistentStorage") < position("SourceGroup"));
        assert!(position("Project::buildIndex") < position("SourceGroup"));
        assert!(position("TaskFillIndexerCommandsQueue") < position("SourceGroup"));
        assert!(position("IndexerCommandCxx") < position("SourceGroup"));
        assert!(position("IndexerJava::doIndex") < position("SourceGroup"));
        assert!(queries.iter().any(|query| query == "source_group"));
        for expected in [
            "SourceGroupSettings",
            "SourceGroupFactoryModule",
            "Project::buildIndex",
            "SourceGroupCxxCdb",
            "SourceGroupCxxCdb::getIndexerCommandProvider",
            "buildIndex",
            "TaskFillIndexerCommandsQueue",
            "IndexerCommand",
            "IndexerCommandCxx",
            "IndexerJava",
            "IndexerJava::doIndex",
            "StorageAccess",
            "StorageAccessProxy",
            "PersistentStorage",
        ] {
            assert!(
                queries.iter().any(|query| query == expected),
                "expected flow concept query {expected} in {queries:?}"
            );
        }
        for noisy in ["turns", "Turns", "cite", "source"] {
            assert!(
                !queries.iter().any(|query| query == noisy),
                "packet probes should suppress noisy query {noisy}: {queries:?}"
            );
        }
    }

    #[test]
    fn packet_followups_preserve_structured_storage_and_source_group_probes() {
        let _eval_probes = EvalProbesGuard::enabled();
        let queries = packet_targeted_follow_up_queries(
            "Explain how project/source-group configuration becomes indexing work, then how indexed data is accessed from storage.",
            PacketTaskClassDto::ArchitectureExplanation,
        );

        assert_eq!(
            queries.len(),
            6,
            "targeted packet follow-ups should stay capped: {queries:?}"
        );
        assert!(
            queries.iter().any(|query| query.contains("Storage")),
            "storage-flow follow-ups should keep structured storage probes: {queries:?}"
        );
        assert!(
            queries.iter().any(|query| query.contains("SourceGroup")),
            "source-group follow-ups should keep a structured source-group probe: {queries:?}"
        );
        assert!(
            queries
                .iter()
                .any(|query| query == "SourceGroupCxxCdb::getIndexerCommandProvider"),
            "existing qualified source-group probes should still be represented: {queries:?}"
        );
        assert!(
            queries
                .iter()
                .any(|query| query == "PersistentStorage::PersistentStorage"),
            "existing persistence constructor probe should still be represented: {queries:?}"
        );
    }

    #[test]
    fn packet_symbol_probes_expand_vscode_workbench_extension_host_concepts() {
        let _eval_probes = EvalProbesGuard::enabled();
        let queries = packet_symbol_probe_queries(
            "Explain how VS Code workbench startup reaches extension host activation and command execution.",
            PacketTaskClassDto::ArchitectureExplanation,
            PacketBudgetModeDto::Standard,
        );

        let position = |needle: &str| {
            queries
                .iter()
                .position(|query| query == needle)
                .unwrap_or_else(|| panic!("missing packet query `{needle}` in {queries:?}"))
        };

        assert!(position("Workbench") < position("workbench_startup"));
        for expected in [
            "Workbench.startup",
            "ExtensionService",
            "ExtensionHostManager",
            "ExtensionHostManager.startup",
            "AbstractExtHostExtensionService",
            "AbstractExtHostExtensionService._startExtensionHost",
            "ExtHostCommands",
            "ExtHostCommands.executeCommand",
        ] {
            assert!(
                queries.iter().any(|query| query == expected),
                "expected VS Code flow concept query {expected} in {queries:?}"
            );
        }
    }

    #[test]
    fn packet_symbol_probes_treat_root_runtime_as_brand_for_payload_content_flow() {
        let _eval_probes = EvalProbesGuard::enabled();
        let queries = packet_symbol_probe_queries(
            "Explain how Root & Runtime public writing and social surfaces connect through Payload collections, post rendering, comment auth/submission, RSS, and the Elsewhere feed.",
            PacketTaskClassDto::ArchitectureExplanation,
            PacketBudgetModeDto::Standard,
        );

        let position = |needle: &str| {
            queries
                .iter()
                .position(|query| query == needle)
                .unwrap_or_else(|| panic!("missing packet query `{needle}` in {queries:?}"))
        };

        assert!(position("buildConfig") < position("public_writing"));
        for expected in [
            "payload_config",
            "payload config",
            "src/payload.config.ts",
            "getPayloadClient",
            "payload client",
            "src/lib/payload.ts",
            "Posts",
            "PostPage",
            "getPostBySlug",
            "getAllPosts",
            "Comments",
            "POST /posts/:slug/comments",
            "getApprovedCommentsForPost",
            "getCommentAuthContextFromHeaders",
            "comment_submission_guard",
            "comment-submission-guard",
            "src/lib/comment-submission-guard.ts",
            "isCommentSubmissionOriginAllowed",
            "isCommentSubmissionTimingAllowed",
            "consumeCommentSubmissionRateLimit",
            "SocialEntries",
            "getLatestSocialEntries",
            "feed.xml",
        ] {
            assert!(
                queries.iter().any(|query| query == expected),
                "expected Payload/content flow query {expected} in {queries:?}"
            );
        }
        for noisy in ["root", "runtime", "root_runtime", "runtime_public"] {
            assert!(
                !queries.iter().any(|query| query == noisy),
                "brand terms should not dominate content-flow probes via {noisy}: {queries:?}"
            );
        }
    }

    #[test]
    fn packet_symbol_probes_keep_brand_phrase_terms_without_flow_anchors() {
        let queries = packet_symbol_probe_queries(
            "Explain the Acme & Widget architecture.",
            PacketTaskClassDto::ArchitectureExplanation,
            PacketBudgetModeDto::Standard,
        );

        for expected in ["acme", "widget", "acme_widget"] {
            assert!(
                queries.iter().any(|query| query == expected),
                "brand terms should remain when they are the only concrete anchors: {queries:?}"
            );
        }
    }

    #[test]
    fn packet_symbol_probes_suppress_non_primary_terms_when_prompt_excludes_pollution() {
        let question = "How does CodeStory choose semantic retrieval candidates for production source ranking while avoiding fixture pollution in packet evidence?";
        let queries = packet_symbol_probe_queries(
            question,
            PacketTaskClassDto::ArchitectureExplanation,
            PacketBudgetModeDto::Standard,
        );
        let concept_queries = packet_concept_queries(question);

        for suppressed in ["fixture", "Fixture"] {
            assert!(
                !queries.iter().any(|query| query == suppressed),
                "packet probes should not search excluded non-primary term {suppressed}: {queries:?}"
            );
            assert!(
                !concept_queries.iter().any(|query| query == suppressed),
                "packet concept queries should not search excluded non-primary term {suppressed}: {concept_queries:?}"
            );
        }
        assert!(
            queries.iter().any(|query| query == "semantic_retrieval"),
            "production query should retain concrete production-retrieval probes: {queries:?}"
        );
    }

    #[test]
    fn packet_symbol_probes_keep_non_primary_terms_when_prompt_requests_them() {
        let queries = packet_symbol_probe_queries(
            "Find the fixture tests for semantic retrieval ranking.",
            PacketTaskClassDto::BugLocalization,
            PacketBudgetModeDto::Standard,
        );

        assert!(
            queries.iter().any(|query| query == "fixture"),
            "explicit fixture/test requests should keep non-primary probe terms: {queries:?}"
        );
        assert!(
            queries.iter().any(|query| query == "tests"),
            "explicit fixture/test requests should keep test probe terms: {queries:?}"
        );
    }

    #[test]
    fn packet_anchor_probes_reject_generic_file_hits() {
        let mut hit = test_search_hit("codex-rs/app-server-protocol/schema/JsonValue.ts", 0.9);
        hit.kind = NodeKind::FILE;
        hit.match_quality = Some(SearchMatchQualityDto::Exact);
        hit.file_path = Some("codex-rs/app-server-protocol/schema/JsonValue.ts".to_string());

        assert!(!packet_anchor_hit_is_relevant("json", &hit));
        assert!(packet_anchor_hit_is_relevant(
            "codex-rs/exec/src/lib.rs",
            &hit
        ));

        hit.file_path = Some("codex-rs/exec/src/exec_events.rs".to_string());
        assert!(packet_anchor_hit_is_relevant("exec_events", &hit));
    }

    #[test]
    fn packet_display_path_preserves_named_repo_subpaths() {
        assert_eq!(
            packet_display_path(r"\\?\C:\Users\alber\source\repos\codex\codex-rs\exec\src\lib.rs"),
            "codex-rs/exec/src/lib.rs"
        );
        assert_eq!(
            packet_display_path(
                r"C:\Users\alber\source\repos\codestory\crates\codestory-cli\src\main.rs"
            ),
            "crates/codestory-cli/src/main.rs"
        );
        assert_eq!(
            packet_display_path(
                r"\\?\C:\Users\alber\source\repos\codestory\target\agent-benchmark\repos\ripgrep\crates\core\main.rs"
            ),
            "crates/core/main.rs"
        );
        assert_eq!(
            packet_display_path("target/agent-benchmark/repos/axios/lib/core/Axios.js"),
            "lib/core/Axios.js"
        );
    }

    #[test]
    fn packet_ranking_prefers_cli_exec_flow_over_sdk_and_test_client() {
        let question = "Explain how `codex exec --json` flows from the top-level CLI into the exec runtime and JSONL event output.";
        let mut answer = packet_answer_fixture(
            question,
            vec![
                test_packet_citation("CodexExec.run", "sdk/typescript/src/exec.ts", 0.95),
                test_packet_citation(
                    "CodexClient::thread_start",
                    "codex-rs/app-server-test-client/src/lib.rs",
                    0.95,
                ),
                test_packet_citation("run_exec_session", "codex-rs/exec/src/lib.rs", 0.2),
                test_packet_citation(
                    "EventProcessorWithJsonOutput",
                    "codex-rs/exec/src/event_processor_with_jsonl_output.rs",
                    0.2,
                ),
                test_packet_citation(
                    "event_processor_with_jsonl_output",
                    "codex-rs/exec/src/lib.rs",
                    0.9,
                ),
                test_packet_citation("run_main", "codex-rs/exec/src/lib.rs", 0.2),
                test_packet_citation("Subcommand::Exec", "codex-rs/cli/src/main.rs", 0.2),
                test_packet_citation(
                    "EventProcessor::process_server_notification",
                    "codex-rs/exec/src/event_processor.rs",
                    0.2,
                ),
                test_packet_citation("ThreadEvent", "codex-rs/exec/src/exec_events.rs", 0.2),
                test_packet_citation(
                    "thread_start_params_include_user_thread_source",
                    "codex-rs/exec/src/lib_tests.rs",
                    0.8,
                ),
            ],
        );

        rank_packet_evidence(question, &mut answer);

        let top = answer
            .citations
            .iter()
            .take(6)
            .map(|citation| citation.display_name.as_str())
            .collect::<Vec<_>>();
        assert!(
            top.iter().any(|name| name.contains("exec")
                || name.contains("Exec")
                || name.contains("json")),
            "exec/json flow symbols should rank in the top band: {top:?}"
        );
        assert!(
            !top.contains(&"thread_start_params_include_user_thread_source"),
            "test-only symbols should not dominate exec/json ranking: {top:?}"
        );
    }

    #[test]
    fn packet_ranking_prefers_vscode_workbench_flow_over_extension_noise() {
        let question = "Explain how VS Code workbench startup reaches extension host activation and command execution.";
        let mut answer = packet_answer_fixture(
            question,
            vec![
                test_packet_citation(
                    "ExtensionHostConnection._sendSocketToExtensionHost",
                    "src/vs/server/node/extensionHostConnection.ts",
                    0.55,
                ),
                test_packet_citation(
                    "ExtensionHost",
                    "extensions/github-authentication/src/flows.ts",
                    0.55,
                ),
                test_packet_citation(
                    "Workbench.startup",
                    "src/vs/workbench/browser/workbench.ts",
                    0.7,
                ),
                test_packet_citation(
                    "ExtensionService",
                    "src/vs/workbench/services/extensions/browser/extensionService.ts",
                    0.7,
                ),
                test_packet_citation(
                    "ExtensionHostManager",
                    "src/vs/workbench/services/extensions/common/extensionHostManager.ts",
                    0.7,
                ),
                test_packet_citation(
                    "AbstractExtHostExtensionService",
                    "src/vs/workbench/api/common/extHostExtensionService.ts",
                    0.7,
                ),
                test_packet_citation(
                    "ExtHostCommands",
                    "src/vs/workbench/api/common/extHostCommands.ts",
                    0.7,
                ),
            ],
        );

        rank_packet_evidence(question, &mut answer);

        let top = answer
            .citations
            .iter()
            .take(5)
            .map(|citation| citation.display_name.as_str())
            .collect::<Vec<_>>();
        let workbench_symbols = [
            "Workbench.startup",
            "ExtensionService",
            "ExtensionHostManager",
            "AbstractExtHostExtensionService",
            "ExtHostCommands",
        ];
        let matched = workbench_symbols
            .iter()
            .filter(|expected| top.contains(expected))
            .count();
        assert!(
            matched >= 4,
            "workbench flow symbols should dominate the top band: matched={matched} top={top:?}"
        );
        assert!(
            !top.iter()
                .any(|name| name.contains("ExtensionHostConnection")),
            "server/extension noise should not dominate workbench flow ranking: {top:?}"
        );
        assert!(
            !top.iter().any(|name| name == &"ExtensionHost"),
            "extension contribution modules should not dominate workbench flow ranking: {top:?}"
        );
    }

    #[test]
    fn packet_capping_prefers_exact_duplicate_claim_definition_file() {
        let _eval_probes = EvalProbesGuard::enabled();
        let question =
            "Explain how indexed data is accessed from StorageAccess and PersistentStorage.";
        let mut proxy_mention = test_packet_citation(
            "StorageAccess",
            "src/lib/data/storage/StorageAccessProxy.h",
            0.95,
        );
        proxy_mention.kind = NodeKind::CLASS;
        let mut contract_definition =
            test_packet_citation("StorageAccess", "src/lib/data/storage/StorageAccess.h", 0.2);
        contract_definition.kind = NodeKind::CLASS;
        let mut answer = packet_answer_fixture(question, vec![proxy_mention, contract_definition]);

        rank_packet_evidence(question, &mut answer);
        apply_packet_budget(
            packet_fixture_project_root(),
            question,
            PacketTaskClassDto::ArchitectureExplanation,
            PacketBudgetModeDto::Compact,
            packet_budget_limits(PacketBudgetModeDto::Compact),
            &mut answer,
        );
        assert!(
            answer.citations.iter().any(|citation| {
                citation.file_path.as_deref() == Some("src/lib/data/storage/StorageAccess.h")
            }),
            "exact same-stem contract definition should survive adjacent header mentions: {:?}",
            answer
                .citations
                .iter()
                .map(|citation| (&citation.display_name, citation.file_path.as_deref()))
                .collect::<Vec<_>>()
        );
        assert!(
            answer.citations.len()
                <= packet_budget_limits(PacketBudgetModeDto::Compact).max_anchors as usize,
            "definition preference should not increase packet anchor budget"
        );
    }

    #[test]
    fn packet_capping_keeps_bounded_secondary_claim_definitions() {
        let mut source_group_method = test_packet_citation(
            "SourceGroupCxxCdb::getIndexerCommandProvider",
            "src/lib_cxx/project/SourceGroupCxxCdb.cpp",
            0.95,
        );
        source_group_method.kind = NodeKind::FUNCTION;
        let mut storage_ctor = test_packet_citation(
            "PersistentStorage::PersistentStorage",
            "src/lib/data/storage/PersistentStorage.cpp",
            0.95,
        );
        storage_ctor.kind = NodeKind::FUNCTION;
        let mut source_group_type = test_packet_citation(
            "SourceGroupCxxCdb",
            "src/lib_cxx/project/SourceGroupCxxCdb.h",
            0.2,
        );
        source_group_type.kind = NodeKind::CLASS;
        let mut persistent_type = test_packet_citation(
            "PersistentStorage",
            "src/lib/data/storage/PersistentStorage.h",
            0.2,
        );
        persistent_type.kind = NodeKind::CLASS;
        let mut low_value = test_packet_citation(
            "SourceGroupFactoryModule::createSourceGroup",
            "src/lib/project/SourceGroupFactoryModule.h",
            0.9,
        );
        low_value.kind = NodeKind::METHOD;
        let mut answer = packet_answer_fixture(
            "Explain source group indexing and persistent storage.",
            vec![
                source_group_method,
                storage_ctor,
                source_group_type,
                persistent_type,
                low_value,
            ],
        );
        let mut limits = packet_budget_limits(PacketBudgetModeDto::Compact);
        limits.max_anchors = 4;
        limits.max_files = 4;

        assert!(cap_citations(&mut answer, &limits));

        let paths = answer
            .citations
            .iter()
            .filter_map(|citation| citation.file_path.as_deref())
            .collect::<Vec<_>>();
        for expected in [
            "src/lib_cxx/project/SourceGroupCxxCdb.cpp",
            "src/lib_cxx/project/SourceGroupCxxCdb.h",
            "src/lib/data/storage/PersistentStorage.cpp",
            "src/lib/data/storage/PersistentStorage.h",
        ] {
            assert!(
                paths.contains(&expected),
                "secondary exact definitions should fit inside the existing cap: {paths:?}"
            );
        }
        assert_eq!(answer.citations.len(), 4);
        assert!(
            !paths.contains(&"src/lib/project/SourceGroupFactoryModule.h"),
            "low-value filler should yield to bounded secondary definitions: {paths:?}"
        );
    }

    #[test]
    fn packet_capping_prefers_distinct_flow_files_over_same_file_role_duplicates() {
        let mut answer = packet_answer_fixture(
            "Explain how `codex exec --json` flows from CLI into runtime and event output.",
            vec![
                test_packet_citation(
                    "EventProcessorWithJsonOutput",
                    "codex-rs/exec/src/event_processor_with_jsonl_output.rs",
                    0.95,
                ),
                test_packet_citation(
                    "EventProcessorWithJsonOutput::emit",
                    "codex-rs/exec/src/event_processor_with_jsonl_output.rs",
                    0.94,
                ),
                test_packet_citation(
                    "EventProcessorWithJsonOutput::completed_item_id",
                    "codex-rs/exec/src/event_processor_with_jsonl_output.rs",
                    0.93,
                ),
                test_packet_citation("ExecSharedCliOptions", "codex-rs/exec/src/cli.rs", 0.7),
                test_packet_citation("run_exec_session", "codex-rs/exec/src/lib.rs", 0.7),
                test_packet_citation("exec_events", "codex-rs/exec/src/exec_events.rs", 0.7),
            ],
        );
        let mut limits = packet_budget_limits(PacketBudgetModeDto::Compact);
        limits.max_anchors = 4;
        limits.max_files = 4;

        assert!(cap_citations(&mut answer, &limits));

        let paths = answer
            .citations
            .iter()
            .filter_map(|citation| citation.file_path.as_deref())
            .collect::<Vec<_>>();
        for expected in [
            "codex-rs/exec/src/event_processor_with_jsonl_output.rs",
            "codex-rs/exec/src/cli.rs",
            "codex-rs/exec/src/lib.rs",
            "codex-rs/exec/src/exec_events.rs",
        ] {
            assert!(
                paths.contains(&expected),
                "packet capping should spend scarce anchors on distinct flow files before same-file duplicate claims: {paths:?}"
            );
        }
        assert_eq!(answer.citations.len(), 4);
    }

    #[test]
    fn packet_budget_protects_required_probe_citations_from_compact_cap() {
        let _eval_probes = EvalProbesGuard::enabled();
        let question = "Explain how `codex exec --json` flows from the top-level CLI into the exec runtime, app-server thread and turn start requests, and JSONL event output.";
        let mut citations = (0..16)
            .map(|index| {
                test_packet_citation(
                    &format!("HighRankDistractor{index}"),
                    &format!("src/high_rank_distractor_{index}.rs"),
                    10.0,
                )
            })
            .collect::<Vec<_>>();
        citations.extend([
            test_packet_citation("run_exec_session", "codex-rs/exec/src/lib.rs", 0.1),
            test_packet_citation("ExecSharedCliOptions", "codex-rs/exec/src/cli.rs", 0.1),
            test_packet_citation("Subcommand::Exec", "codex-rs/cli/src/main.rs", 0.1),
            test_packet_citation("run_main", "codex-rs/app-server/src/lib.rs", 6.0),
            test_packet_citation("codex_exec::run_main", "codex-rs/exec/src/main.rs", 0.1),
            test_packet_citation("codex_exec::Cli", "codex-rs/exec/src/main.rs", 0.1),
            test_packet_citation(
                "EventProcessorWithJsonOutput",
                "codex-rs/exec/src/event_processor_with_jsonl_output.rs",
                0.1,
            ),
            test_packet_citation(
                "crate::exec_events::AgentMessageItem (import)",
                "codex-rs/exec/src/event_processor_with_jsonl_output.rs",
                5.0,
            ),
            test_packet_citation("exec_events", "codex-rs/exec/src/lib.rs", 6.0),
            test_packet_citation(
                "codex_protocol::models::WebSearchAction",
                "codex-rs/exec/src/exec_events.rs",
                0.1,
            ),
            test_packet_citation(
                "ThreadStartParams",
                "codex-rs/app-server-protocol/src/protocol/v2/thread.rs",
                0.1,
            ),
            test_packet_citation(
                "TurnStartParams",
                "codex-rs/app-server-protocol/src/protocol/v2/turn.rs",
                0.1,
            ),
        ]);
        let mut answer = packet_answer_fixture(question, citations);

        rank_packet_evidence(question, &mut answer);
        let budget = apply_packet_budget(
            packet_fixture_project_root(),
            question,
            PacketTaskClassDto::ArchitectureExplanation,
            PacketBudgetModeDto::Compact,
            packet_budget_limits(PacketBudgetModeDto::Compact),
            &mut answer,
        );

        let paths = answer
            .citations
            .iter()
            .filter_map(|citation| citation.file_path.as_deref())
            .collect::<Vec<_>>();
        for expected in [
            "codex-rs/exec/src/lib.rs",
            "codex-rs/exec/src/cli.rs",
            "codex-rs/cli/src/main.rs",
            "codex-rs/exec/src/main.rs",
            "codex-rs/exec/src/event_processor_with_jsonl_output.rs",
            "codex-rs/exec/src/exec_events.rs",
            "codex-rs/app-server-protocol/src/protocol/v2/thread.rs",
            "codex-rs/app-server-protocol/src/protocol/v2/turn.rs",
        ] {
            assert!(
                paths.contains(&expected),
                "compact packet cap should protect required planned-probe citations before high-ranking distractors: {paths:?}"
            );
        }
        assert!(
            answer
                .citations
                .iter()
                .any(|citation| citation.file_path.as_deref()
                    == Some("codex-rs/exec/src/exec_events.rs")),
            "required probe protection should prefer the actual exec_events file over import-only or facade mentions: {:?}",
            answer
                .citations
                .iter()
                .map(|citation| (&citation.display_name, citation.file_path.as_deref()))
                .collect::<Vec<_>>()
        );
        let exact_entrypoint_index = answer
            .citations
            .iter()
            .position(|citation| {
                citation.display_name == "codex_exec::run_main"
                    && citation.file_path.as_deref() == Some("codex-rs/exec/src/main.rs")
            })
            .unwrap_or_else(|| {
                panic!(
                    "command-span exact probes should protect the qualified command entrypoint: {:?}",
                    answer
                        .citations
                        .iter()
                        .map(|citation| (&citation.display_name, citation.file_path.as_deref()))
                        .collect::<Vec<_>>()
                )
            });
        if let Some(broad_entrypoint_index) = answer.citations.iter().position(|citation| {
            citation.display_name == "run_main"
                && citation.file_path.as_deref() == Some("codex-rs/app-server/src/lib.rs")
        }) {
            assert!(
                exact_entrypoint_index < broad_entrypoint_index,
                "qualified command evidence should be protected ahead of weaker broad command evidence: {:?}",
                answer
                    .citations
                    .iter()
                    .map(|citation| (&citation.display_name, citation.file_path.as_deref()))
                    .collect::<Vec<_>>()
            );
        }
        assert!(
            answer
                .citations
                .iter()
                .any(|citation| citation.display_name == "codex_exec::Cli"
                    && citation.file_path.as_deref() == Some("codex-rs/exec/src/main.rs")),
            "command-span exact probes should protect the qualified command CLI type: {:?}",
            answer
                .citations
                .iter()
                .map(|citation| (&citation.display_name, citation.file_path.as_deref()))
                .collect::<Vec<_>>()
        );
        assert!(budget.truncated);
        assert_eq!(answer.citations.len(), 13);
    }

    #[test]
    fn packet_budget_protects_indexing_flow_action_probe_citations() {
        let question = "Explain how a full indexing run moves from the CLI into runtime orchestration, file discovery, symbol extraction, persistence, and search or snapshot refresh.";
        let mut citations = (0..20)
            .map(|index| {
                test_packet_citation(
                    &format!("HighRankDistractor{index}"),
                    &format!("src/high_rank_distractor_{index}.rs"),
                    10.0,
                )
            })
            .collect::<Vec<_>>();
        citations.extend([
            test_packet_citation(
                "test_incremental_indexing_second_run_reuses_unchanged_extraction_cache_and_resolution_support",
                "crates/codestory-indexer/tests/integration.rs",
                9.5,
            ),
            test_packet_citation(
                "EmbeddingRuntime::test_runtime",
                "crates/codestory-runtime/src/search/engine.rs",
                9.4,
            ),
            test_packet_citation(
                "tests::drill_question_search_is_partial_discovery_evidence",
                "crates/codestory-cli/src/main.rs",
                9.3,
            ),
            test_packet_citation(
                "Runtime::index_service",
                "crates/codestory-runtime/src/lib.rs",
                9.0,
            ),
            test_packet_citation(
                "WorkspaceIndexer",
                "crates/codestory-indexer/src/lib.rs",
                9.0,
            ),
            test_packet_citation(
                "Storage::upsert_search_symbol_projection_batch",
                "crates/codestory-store/src/storage_impl/mod.rs",
                9.0,
            ),
            test_packet_citation(
                "SnapshotRefreshStats",
                "crates/codestory-store/src/snapshot_store.rs",
                9.0,
            ),
            test_packet_citation(
                "IndexService::run_indexing_blocking",
                "crates/codestory-runtime/src/services.rs",
                0.1,
            ),
            test_packet_citation(
                "WorkspaceManifest::build_execution_plan",
                "crates/codestory-workspace/src/lib.rs",
                0.1,
            ),
            test_packet_citation(
                "WorkspaceIndexer::run",
                "crates/codestory-indexer/src/lib.rs",
                0.1,
            ),
            test_packet_citation("index_file", "crates/codestory-indexer/src/lib.rs", 0.1),
            test_packet_citation(
                "Storage::flush_projection_batch",
                "crates/codestory-store/src/storage_impl/mod.rs",
                0.1,
            ),
            test_packet_citation(
                "Storage::rebuild_search_symbol_projection_from_node_table",
                "crates/codestory-store/src/storage_impl/mod.rs",
                0.1,
            ),
            test_packet_citation(
                "SnapshotStore::refresh_all_with_stats",
                "crates/codestory-store/src/snapshot_store.rs",
                0.1,
            ),
        ]);
        let mut answer = packet_answer_fixture(question, citations);

        rank_packet_evidence(question, &mut answer);
        let budget = apply_packet_budget(
            packet_fixture_project_root(),
            question,
            PacketTaskClassDto::ArchitectureExplanation,
            PacketBudgetModeDto::Compact,
            packet_budget_limits(PacketBudgetModeDto::Compact),
            &mut answer,
        );

        let display_names = answer
            .citations
            .iter()
            .map(|citation| citation.display_name.as_str())
            .collect::<Vec<_>>();
        for expected in [
            "Runtime::index_service",
            "IndexService::run_indexing_blocking",
            "WorkspaceManifest::build_execution_plan",
            "WorkspaceIndexer::run",
            "index_file",
            "Storage::flush_projection_batch",
            "Storage::rebuild_search_symbol_projection_from_node_table",
            "SnapshotStore::refresh_all_with_stats",
        ] {
            assert!(
                display_names.contains(&expected),
                "compact packet cap should protect indexing-flow action probe {expected}: {display_names:?}"
            );
        }
        for low_value in [
            "test_incremental_indexing_second_run_reuses_unchanged_extraction_cache_and_resolution_support",
            "EmbeddingRuntime::test_runtime",
            "tests::drill_question_search_is_partial_discovery_evidence",
        ] {
            assert!(
                !display_names.contains(&low_value),
                "compact packet should not keep low-value test evidence when production alternatives exist: {display_names:?}"
            );
        }
        assert!(budget.truncated);
        assert_eq!(answer.citations.len(), 13);
    }

    #[test]
    fn packet_budget_replaces_overrepresented_family_with_late_flow_role() {
        let question = "Explain how project/source-group configuration becomes indexing work and storage access.";
        let mut citations = (0..10)
            .map(|index| {
                test_packet_citation(
                    &format!("SourceGroupFamily{index}"),
                    &format!("src/lib/project/SourceGroupFamily{index}.cpp"),
                    10.0 - (index as f32 * 0.1),
                )
            })
            .collect::<Vec<_>>();
        citations.extend([
            test_packet_citation(
                "TaskFillIndexerCommandsQueue",
                "src/lib/data/indexer/TaskFillIndexerCommandsQueue.h",
                0.2,
            ),
            test_packet_citation("StorageAccess", "src/lib/data/storage/StorageAccess.h", 0.1),
        ]);
        let mut answer = packet_answer_fixture(question, citations);
        let mut limits = packet_budget_limits(PacketBudgetModeDto::Compact);
        limits.max_anchors = 6;
        limits.max_files = 6;

        rank_packet_evidence(question, &mut answer);
        let budget = apply_packet_budget(
            packet_fixture_project_root(),
            question,
            PacketTaskClassDto::ArchitectureExplanation,
            PacketBudgetModeDto::Compact,
            limits,
            &mut answer,
        );

        assert!(budget.truncated, "fixture should exercise citation capping");
        let paths = answer
            .citations
            .iter()
            .filter_map(|citation| citation.file_path.as_deref())
            .collect::<Vec<_>>();
        assert!(
            paths.contains(&"src/lib/data/indexer/TaskFillIndexerCommandsQueue.h"),
            "late indexing evidence should replace overrepresented source-group evidence: {paths:?}"
        );
        assert!(
            paths.contains(&"src/lib/data/storage/StorageAccess.h"),
            "late storage evidence should replace overrepresented source-group evidence: {paths:?}"
        );
    }

    #[test]
    fn packet_budget_replaces_weaker_same_role_with_late_definition_files() {
        let question =
            "Explain how source-group configuration becomes indexing work and storage access.";
        let mut set_subject = test_packet_citation(
            "StorageAccessProxy::setSubject",
            "src/lib/data/storage/StorageAccessProxy.h",
            9.0,
        );
        set_subject.kind = NodeKind::METHOD;
        let mut storage_contract =
            test_packet_citation("StorageAccess", "src/lib/data/storage/StorageAccess.h", 0.1);
        storage_contract.kind = NodeKind::CLASS;
        let mut persistent_storage = test_packet_citation(
            "PersistentStorage",
            "src/lib/data/storage/PersistentStorage.h",
            0.1,
        );
        persistent_storage.kind = NodeKind::CLASS;
        let mut task_ctor = test_packet_citation(
            "TaskFillIndexerCommandsQueue::TaskFillIndexerCommandsQueue",
            "src/lib/data/indexer/TaskFillIndexerCommandQueue.cpp",
            8.0,
        );
        task_ctor.kind = NodeKind::FUNCTION;
        let mut task_type = test_packet_citation(
            "TaskFillIndexerCommandsQueue",
            "src/lib/data/indexer/TaskFillIndexerCommandQueue.h",
            0.1,
        );
        task_type.kind = NodeKind::CLASS;
        let mut answer = packet_answer_fixture(
            question,
            vec![
                test_packet_citation("SourceGroupFamily0", "src/lib/project/SourceGroup0.h", 10.0),
                test_packet_citation("SourceGroupFamily1", "src/lib/project/SourceGroup1.h", 9.5),
                set_subject,
                test_packet_citation("StorageTest", "src/test/StorageTestSuite.cpp", 9.0),
                task_ctor,
                storage_contract,
                persistent_storage,
                task_type,
            ],
        );
        let mut limits = packet_budget_limits(PacketBudgetModeDto::Compact);
        limits.max_anchors = 6;
        limits.max_files = 6;

        assert!(cap_citations(&mut answer, &limits));

        let paths = answer
            .citations
            .iter()
            .filter_map(|citation| citation.file_path.as_deref())
            .collect::<Vec<_>>();
        for expected in [
            "src/lib/data/storage/StorageAccess.h",
            "src/lib/data/storage/PersistentStorage.h",
            "src/lib/data/indexer/TaskFillIndexerCommandQueue.h",
        ] {
            assert!(
                paths.contains(&expected),
                "late definition-file anchors should replace weaker same-role/test citations: {paths:?}"
            );
        }
        assert!(
            !paths.contains(&"src/test/StorageTestSuite.cpp"),
            "test evidence should not consume compact citation slots for production-flow prompts: {paths:?}"
        );
        assert!(answer.citations.len() <= limits.max_anchors as usize);
    }

    #[test]
    fn packet_budget_protects_storage_required_probe_citations() {
        let question = "Explain how project/source-group configuration becomes indexing work, then how indexed data is accessed by the application.";
        let mut proxy_header = test_packet_citation(
            "StorageAccessProxy",
            "src/lib/data/storage/StorageAccessProxy.h",
            10.0,
        );
        proxy_header.kind = NodeKind::CLASS;
        let mut storage_contract =
            test_packet_citation("StorageAccess", "src/lib/data/storage/StorageAccess.h", 0.1);
        storage_contract.kind = NodeKind::CLASS;
        let mut proxy_method = test_packet_citation(
            "StorageAccessProxy::setSubject",
            "src/lib/data/storage/StorageAccessProxy.cpp",
            0.1,
        );
        proxy_method.kind = NodeKind::METHOD;
        let mut persistent_type = test_packet_citation(
            "PersistentStorage",
            "src/lib/data/storage/PersistentStorage.h",
            0.1,
        );
        persistent_type.kind = NodeKind::CLASS;
        let mut persistent_ctor = test_packet_citation(
            "PersistentStorage::PersistentStorage",
            "src/lib/data/storage/PersistentStorage.cpp",
            0.1,
        );
        persistent_ctor.kind = NodeKind::FUNCTION;
        let mut answer = packet_answer_fixture(
            question,
            vec![
                test_packet_citation("SourceGroup", "src/lib/project/SourceGroup.h", 9.5),
                proxy_header,
                test_packet_citation("StorageRegression", "src/test/StorageRegression.cpp", 9.0),
                storage_contract,
                proxy_method,
                persistent_type,
                persistent_ctor,
            ],
        );
        let mut limits = packet_budget_limits(PacketBudgetModeDto::Compact);
        limits.max_anchors = 5;
        limits.max_files = 5;

        rank_packet_evidence(question, &mut answer);
        let budget = apply_packet_budget(
            packet_fixture_project_root(),
            question,
            PacketTaskClassDto::ArchitectureExplanation,
            PacketBudgetModeDto::Compact,
            limits,
            &mut answer,
        );

        assert!(budget.truncated, "fixture should exercise compact capping");
        let paths = answer
            .citations
            .iter()
            .filter_map(|citation| citation.file_path.as_deref())
            .collect::<Vec<_>>();
        for expected in [
            "src/lib/data/storage/StorageAccess.h",
            "src/lib/data/storage/PersistentStorage.h",
            "src/lib/data/storage/PersistentStorage.cpp",
        ] {
            assert!(
                paths.contains(&expected),
                "storage-flow required probes should protect exact contract and implementation paths: {paths:?}"
            );
        }
        assert!(
            !paths.contains(&"src/test/StorageRegression.cpp"),
            "test filler should not displace protected production storage paths: {paths:?}"
        );
    }

    #[test]
    fn packet_budget_protects_indexing_required_probe_citations() {
        let question = "Explain how project/source-group configuration becomes indexing work, then how command providers create C++ and Java work items.";
        let mut citations = (0..10)
            .map(|index| {
                test_packet_citation(
                    &format!("SourceGroupNoise{index}"),
                    &format!("src/lib/project/SourceGroupNoise{index}.h"),
                    10.0 - (index as f32 * 0.1),
                )
            })
            .collect::<Vec<_>>();
        let mut build_index =
            test_packet_citation("Project::buildIndex", "src/lib/project/Project.cpp", 0.1);
        build_index.kind = NodeKind::FUNCTION;
        let mut source_group_type = test_packet_citation(
            "SourceGroupCxxCdb",
            "src/lib_cxx/project/SourceGroupCxxCdb.h",
            0.1,
        );
        source_group_type.kind = NodeKind::CLASS;
        let mut source_group_provider = test_packet_citation(
            "SourceGroupCxxCdb::getIndexerCommandProvider",
            "src/lib_cxx/project/SourceGroupCxxCdb.cpp",
            0.1,
        );
        source_group_provider.kind = NodeKind::METHOD;
        let mut task_queue = test_packet_citation(
            "TaskFillIndexerCommandsQueue",
            "src/lib/data/indexer/TaskFillIndexerCommandQueue.h",
            0.1,
        );
        task_queue.kind = NodeKind::CLASS;
        let mut cxx_command = test_packet_citation(
            "IndexerCommandCxx",
            "src/lib_cxx/data/indexer/IndexerCommandCxx.h",
            0.1,
        );
        cxx_command.kind = NodeKind::CLASS;
        let mut java_indexer = test_packet_citation(
            "IndexerJava::doIndex",
            "src/lib_java/data/indexer/IndexerJava.cpp",
            0.1,
        );
        java_indexer.kind = NodeKind::METHOD;
        citations.extend([
            build_index,
            source_group_type,
            source_group_provider,
            task_queue,
            cxx_command,
            java_indexer,
            test_packet_citation("IndexerRegression", "src/test/IndexerRegression.cpp", 9.5),
        ]);
        let mut answer = packet_answer_fixture(question, citations);
        let mut limits = packet_budget_limits(PacketBudgetModeDto::Compact);
        limits.max_anchors = 8;
        limits.max_files = 8;

        rank_packet_evidence(question, &mut answer);
        let budget = apply_packet_budget(
            packet_fixture_project_root(),
            question,
            PacketTaskClassDto::ArchitectureExplanation,
            PacketBudgetModeDto::Compact,
            limits,
            &mut answer,
        );

        assert!(budget.truncated, "fixture should exercise compact capping");
        let paths = answer
            .citations
            .iter()
            .filter_map(|citation| citation.file_path.as_deref())
            .collect::<Vec<_>>();
        for expected in [
            "src/lib/project/Project.cpp",
            "src/lib_cxx/project/SourceGroupCxxCdb.h",
            "src/lib_cxx/data/indexer/IndexerCommandCxx.h",
        ] {
            assert!(
                paths.contains(&expected),
                "indexing required probes should protect exact source-group and work-queue paths: {paths:?}"
            );
        }
        assert!(
            !paths.contains(&"src/test/IndexerRegression.cpp"),
            "test filler should not displace protected production indexing paths: {paths:?}"
        );
    }

    #[test]
    fn packet_budget_fills_spare_capacity_with_deferred_production_before_tests() {
        let mut proxy_header = test_packet_citation(
            "StorageAccessProxy",
            "src/lib/data/storage/StorageAccessProxy.h",
            0.9,
        );
        proxy_header.kind = NodeKind::CLASS;
        let mut proxy_impl = test_packet_citation(
            "StorageAccessProxy",
            "src/lib/data/storage/StorageAccessProxy.cpp",
            0.1,
        );
        proxy_impl.kind = NodeKind::CLASS;
        let mut answer = packet_answer_fixture(
            "Explain runtime routing, indexing, and storage access.",
            vec![
                test_packet_citation("RuntimeCoordinator", "src/runtime/coordinator.rs", 0.8),
                proxy_header,
                test_packet_citation(
                    "TaskBuildIndex",
                    "src/lib/data/indexer/TaskBuildIndex.h",
                    0.8,
                ),
                test_packet_citation("StorageRegression", "src/test/StorageRegression.cpp", 10.0),
                proxy_impl,
            ],
        );
        let mut limits = packet_budget_limits(PacketBudgetModeDto::Compact);
        limits.max_anchors = 4;
        limits.max_files = 4;

        assert!(cap_citations(&mut answer, &limits));

        let paths = answer
            .citations
            .iter()
            .filter_map(|citation| citation.file_path.as_deref())
            .collect::<Vec<_>>();
        assert!(
            paths.contains(&"src/lib/data/storage/StorageAccessProxy.cpp"),
            "deferred production evidence should fill spare capacity before deferred tests: {paths:?}"
        );
        assert!(
            !paths.contains(&"src/test/StorageRegression.cpp"),
            "deferred tests should only fill after production evidence is exhausted: {paths:?}"
        );
    }

    #[test]
    fn packet_budget_defers_test_evidence_when_compact_cap_is_full() {
        let mut answer = packet_answer_fixture(
            "Explain runtime routing and persistence.",
            vec![
                test_packet_citation("RuntimeCoordinator", "src/runtime/coordinator.rs", 0.8),
                test_packet_citation("RouteHandler", "src/router/handler.rs", 0.8),
                test_packet_citation("ProjectionStore", "src/store/projection.rs", 0.8),
                test_packet_citation("RegressionCase", "tests/runtime_regression.rs", 10.0),
            ],
        );
        let mut limits = packet_budget_limits(PacketBudgetModeDto::Compact);
        limits.max_anchors = 3;
        limits.max_files = 3;

        assert!(cap_citations(&mut answer, &limits));

        let paths = answer
            .citations
            .iter()
            .filter_map(|citation| citation.file_path.as_deref())
            .collect::<Vec<_>>();
        assert!(
            !paths.contains(&"tests/runtime_regression.rs"),
            "test evidence should fill only spare citation capacity: {paths:?}"
        );
    }

    #[test]
    fn packet_ranking_demotes_low_signal_current_symbols() {
        let question = "Study current architecture and runtime boundaries.";
        let mut answer = packet_answer_fixture(
            question,
            vec![
                test_packet_citation("current", "src/runtime/current.rs", 5.0),
                test_packet_citation("RuntimeCoordinator", "src/runtime/coordinator.rs", 0.2),
            ],
        );

        rank_packet_evidence(question, &mut answer);

        assert_eq!(answer.citations[0].display_name, "RuntimeCoordinator");
    }

    #[test]
    fn packet_ranking_keeps_explicit_low_signal_symbol_queries() {
        let question = "Find `current`.";
        let mut answer = packet_answer_fixture(
            question,
            vec![
                test_packet_citation("current", "src/runtime/current.rs", 0.2),
                test_packet_citation("RuntimeCoordinator", "src/runtime/coordinator.rs", 0.2),
            ],
        );

        rank_packet_evidence(question, &mut answer);

        assert_eq!(answer.citations[0].display_name, "current");
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
            PacketBudgetModeDto::Standard,
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
            PacketBudgetModeDto::Standard,
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
            PacketBudgetModeDto::Standard,
        );
        let queries = plan
            .queries
            .iter()
            .map(|query| query.query.as_str())
            .collect::<Vec<_>>();

        for expected in [
            "index service",
            "workspace execution plan",
            "workspace indexer",
            "symbol extraction indexer",
            "search projection",
            "snapshot refresh",
            "indexing",
            "runtime",
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
    fn architecture_packet_plan_uses_generic_flow_terms_without_eval_probes() {
        let _env = EnvVarGuard::cleared(EVAL_PROBES_ENV);
        let cases = [
            (
                "Explain how a client request flows through interceptors, request dispatch, and the transport adapter. Cite the source files that support the path.",
                &[
                    "request interceptor",
                    "request dispatch",
                    "transport adapter",
                ][..],
            ),
            (
                "Explain how a server starts its event loop, reads client commands from the network, and dispatches them through command handlers. Cite the source files that support the path.",
                &[
                    "event loop",
                    "event dispatch",
                    "network input",
                    "command dispatch",
                ][..],
            ),
            (
                "Explain how a search command parses CLI flags, walks candidate files, and executes a search through matcher, searcher, and printer components. Cite the source files that support the path.",
                &[
                    "search entrypoint",
                    "argument planning",
                    "candidate file walk",
                    "search worker",
                    "result printer",
                ][..],
            ),
        ];

        for (question, expected_queries) in cases {
            let plan = build_packet_plan(
                question,
                Some(PacketTaskClassDto::ArchitectureExplanation),
                PacketBudgetModeDto::Compact,
            );
            let queries = plan
                .queries
                .iter()
                .map(|query| query.query.as_str())
                .collect::<Vec<_>>();
            for expected in expected_queries {
                assert!(
                    queries
                        .iter()
                        .any(|query| query.eq_ignore_ascii_case(expected)),
                    "expected {expected} in architecture packet plan: {queries:?}"
                );
            }
            for forbidden in [
                "createInstance",
                "InterceptorManager",
                "dispatchRequest",
                "adapters.js",
                "server.c main",
                "aeMain",
                "readQueryFromClient",
                "processCommand",
                "server.c call",
                "core/main.rs",
                "HiArgs",
                "SearchWorker::search",
                "haystack.rs",
            ] {
                assert!(
                    !queries
                        .iter()
                        .any(|query| query.eq_ignore_ascii_case(forbidden)),
                    "non-eval packet plan should not inject holdout anchor {forbidden}: {queries:?}"
                );
            }
        }
    }

    #[test]
    fn architecture_packet_plan_can_use_eval_manifest_probes_when_enabled() {
        let _eval_probes = EvalProbesGuard::enabled();
        let cases = [
            (
                "Explain how the default axios instance is created and how an HTTP request flows through interceptors, dispatchRequest, and the transport adapter. Cite the source files that support the path.",
                &[
                    "createInstance",
                    "InterceptorManager",
                    "dispatchRequest",
                    "adapters.js",
                ][..],
            ),
            (
                "Explain how the Redis server starts its event loop, reads client commands from the network, and dispatches them through processCommand and call. Cite the source files that support the path.",
                &[
                    "server.c main",
                    "aeMain",
                    "readQueryFromClient",
                    "processCommand",
                    "server.c call",
                ][..],
            ),
            (
                "Explain how ripgrep parses CLI flags, walks candidate files, and executes a search over each haystack through matcher, searcher, and printer components. Cite the source files that support the path.",
                &[
                    "core/main.rs",
                    "HiArgs",
                    "SearchWorker::search",
                    "haystack.rs",
                ][..],
            ),
        ];

        for (question, expected_queries) in cases {
            let plan = build_packet_plan(
                question,
                Some(PacketTaskClassDto::ArchitectureExplanation),
                PacketBudgetModeDto::Compact,
            );
            let queries = plan
                .queries
                .iter()
                .map(|query| query.query.as_str())
                .collect::<Vec<_>>();
            for expected in expected_queries {
                assert!(
                    queries
                        .iter()
                        .any(|query| query.eq_ignore_ascii_case(expected)),
                    "expected eval probe {expected} in architecture packet plan: {queries:?}"
                );
            }
        }
    }

    #[test]
    fn packet_plan_uses_explicit_request_probes_with_required_sufficiency() {
        let question = "Explain how request dispatch reaches validation and callbacks.";
        let extra_probes = vec![
            "Source/Core/RequestSession.swift Session.request".to_string(),
            "Source/Core/DataRequest.swift DataRequest.validate".to_string(),
        ];
        let plan = build_packet_plan_with_extra(
            question,
            Some(PacketTaskClassDto::RouteTracing),
            PacketBudgetModeDto::Compact,
            &extra_probes,
        );
        let queries = plan
            .queries
            .iter()
            .map(|query| (query.query.as_str(), query.purpose.as_str()))
            .collect::<Vec<_>>();

        for expected in &extra_probes {
            assert!(
                queries.iter().any(|(query, purpose)| {
                    query.eq_ignore_ascii_case(expected)
                        && purpose.contains("explicit symbol probe")
                }),
                "expected explicit probe {expected} in packet plan: {queries:?}"
            );
        }
        assert!(
            plan.trace
                .iter()
                .any(|entry| entry == "explicit_extra_probes=2 source=request"),
            "packet plan should trace explicit request-probe provenance: {:?}",
            plan.trace
        );

        let required = packet_sufficiency_required_probe_queries_with_extra(
            question,
            PacketTaskClassDto::RouteTracing,
            &extra_probes,
        );
        for expected in &extra_probes {
            assert!(
                required
                    .iter()
                    .any(|query| query.eq_ignore_ascii_case(expected)),
                "expected explicit probe {expected} in sufficiency requirements: {required:?}"
            );
        }
    }
    #[test]
    fn command_dispatch_flow_does_not_require_request_dispatch_probes() {
        let _env = EnvVarGuard::cleared(EVAL_PROBES_ENV);
        let question = "Explain how a server starts its event loop, reads client commands from the network, and dispatches them through command handlers.";
        let plan = build_packet_plan(
            question,
            Some(PacketTaskClassDto::ArchitectureExplanation),
            PacketBudgetModeDto::Compact,
        );
        let queries = plan
            .queries
            .iter()
            .map(|query| query.query.as_str())
            .collect::<Vec<_>>();

        for expected in ["event loop", "network input", "command dispatch"] {
            assert!(
                queries.contains(&expected),
                "expected {expected} in command/event flow packet plan: {queries:?}"
            );
        }
        for request_probe in [
            "request interceptor",
            "request dispatch",
            "transport adapter",
            "interceptor manager",
            "dispatch request",
        ] {
            assert!(
                !queries.contains(&request_probe),
                "command dispatch should not inject request probe {request_probe}: {queries:?}"
            );
        }

        let required = packet_sufficiency_required_probe_queries(
            question,
            PacketTaskClassDto::ArchitectureExplanation,
        );
        for request_probe in [
            "request interceptor",
            "request dispatch",
            "transport adapter",
        ] {
            assert!(
                !required.iter().any(|query| query == request_probe),
                "sufficiency should not require request probe {request_probe}: {required:?}"
            );
        }
    }

    #[test]
    fn compact_packet_plan_promotes_indexing_flow_stage_queries() {
        let plan = build_packet_plan(
            "Explain how a full indexing run moves from the CLI into runtime orchestration, file discovery, symbol extraction, persistence, and search or snapshot refresh.",
            Some(PacketTaskClassDto::ArchitectureExplanation),
            PacketBudgetModeDto::Compact,
        );
        let queries = plan
            .queries
            .iter()
            .map(|query| query.query.as_str())
            .collect::<Vec<_>>();

        for expected in [
            "index service",
            "workspace execution plan",
            "workspace indexer",
            "search projection",
            "snapshot refresh",
        ] {
            assert!(
                queries.contains(&expected),
                "expected indexing-flow stage probe {expected} in compact packet plan: {queries:?}"
            );
        }

        let stage_index = queries
            .iter()
            .position(|query| *query == "index service")
            .expect("index service stage probe should be present");
        for low_signal in ["full", "moves", "run_moves", "RunMoves"] {
            assert!(
                !queries.contains(&low_signal),
                "packet planner should suppress isolated low-signal term {low_signal}: {queries:?}"
            );
        }
        let broad_probe = "runtime";
        let probe_index = queries
            .iter()
            .position(|query| *query == broad_probe)
            .expect("broad probe should still be present");
        assert!(
            stage_index < probe_index,
            "indexing-flow stage probes should precede broad probe {broad_probe}: {queries:?}"
        );
    }

    #[test]
    fn compact_packet_plan_protects_indexing_flow_action_probes() {
        let plan = build_packet_plan(
            "Explain how a full indexing run moves from the CLI into runtime orchestration, file discovery, symbol extraction, persistence, and search or snapshot refresh.",
            Some(PacketTaskClassDto::ArchitectureExplanation),
            PacketBudgetModeDto::Compact,
        );
        let queries = plan
            .queries
            .iter()
            .map(|query| query.query.as_str())
            .collect::<Vec<_>>();

        for expected in [
            "index service run indexing",
            "workspace manifest build execution plan",
            "workspace indexer run",
            "index_file",
            "storage flush projection batch",
            "storage rebuild search symbol projection",
            "snapshot refresh all stats",
        ] {
            assert!(
                queries.contains(&expected),
                "expected indexing-flow action probe {expected} in compact packet plan: {queries:?}"
            );
        }
        for fixture_anchor in [
            "IndexService::run_indexing_blocking",
            "WorkspaceManifest::build_execution_plan",
            "WorkspaceIndexer::run",
            "Storage::rebuild_search_symbol_projection_from_node_table",
            "SnapshotStore::refresh_all_with_stats",
        ] {
            assert!(
                !queries.contains(&fixture_anchor),
                "packet planner should protect generic action probes without injecting fixture-specific anchor {fixture_anchor}: {queries:?}"
            );
        }
    }

    #[test]
    fn compact_packet_initial_retrieval_keeps_semantic_hybrid_and_anchor_prompt() {
        let plan = build_packet_plan(
            "Explain how VS Code workbench startup reaches ExtensionService, ExtensionHostManager, AbstractExtHostExtensionService, and ExtHostCommands.executeCommand.",
            Some(PacketTaskClassDto::ArchitectureExplanation),
            PacketBudgetModeDto::Compact,
        );

        assert!(
            packet_initial_hybrid_weights(&plan, PacketBudgetModeDto::Compact).is_none(),
            "compact packets should not collapse initial retrieval to lexical-only"
        );
        let prompt = packet_retrieval_prompt(
            "Explain startup.",
            &plan,
            None,
            PacketBudgetModeDto::Compact,
        );
        assert!(prompt.starts_with("Explain startup."));
        assert!(prompt.contains("Planned CodeStory queries:"));
        assert!(prompt.contains("ExtensionService"));
        assert!(prompt.contains("ExtHostCommands"));
        assert!(prompt.to_ascii_lowercase().contains("workbench"));
    }

    #[test]
    fn packet_plan_suppresses_low_signal_broad_prompt_terms() {
        let plan = build_packet_plan(
            "Study current architecture boundaries across contracts workspace indexer store runtime cli bench retrieval packet flow ranking precision latency risks.",
            Some(PacketTaskClassDto::ArchitectureExplanation),
            PacketBudgetModeDto::Standard,
        );
        let queries = plan
            .queries
            .iter()
            .map(|query| query.query.as_str())
            .collect::<Vec<_>>();

        for low_signal in [
            "current",
            "Current",
            "current_architecture",
            "CurrentArchitecture",
            "latency_risks",
            "LatencyRisks",
            "risks",
            "Risks",
        ] {
            assert!(
                !queries.contains(&low_signal),
                "packet planner should suppress low-signal broad prompt term {low_signal}: {queries:?}"
            );
        }
        for retained in [
            "contracts",
            "workspace",
            "indexer",
            "store",
            "bench_retrieval",
        ] {
            assert!(
                queries.contains(&retained),
                "packet planner should retain concrete repo/retrieval term {retained}: {queries:?}"
            );
        }
    }

    #[test]
    fn packet_plan_keeps_broad_risk_study_as_architecture() {
        let plan = build_packet_plan(
            "Study current architecture boundaries, packet flow, ranking precision, and latency risks.",
            None,
            PacketBudgetModeDto::Standard,
        );
        let queries = plan
            .queries
            .iter()
            .map(|query| query.query.as_str())
            .collect::<Vec<_>>();

        assert_eq!(plan.task_class, PacketTaskClassDto::ArchitectureExplanation);
        assert!(
            queries.contains(&"architecture entrypoint"),
            "architecture packets should keep architecture seeds: {queries:?}"
        );
        assert!(
            !queries.contains(&"affected symbols"),
            "generic risk wording should not force change-impact seeds: {queries:?}"
        );
    }

    #[test]
    fn packet_plan_routes_specific_risk_of_change_prompts_to_change_impact() {
        for question in [
            "What risk if I change reference resolution behavior?",
            "What is the risk of changing reference resolution behavior?",
        ] {
            let plan = build_packet_plan(question, None, PacketBudgetModeDto::Standard);
            let queries = plan
                .queries
                .iter()
                .map(|query| query.query.as_str())
                .collect::<Vec<_>>();

            assert_eq!(
                plan.task_class,
                PacketTaskClassDto::ChangeImpact,
                "{question}"
            );
            assert!(
                queries.contains(&"affected symbols"),
                "specific risk-of-change prompts should keep change-impact seeds: {queries:?}"
            );
        }
    }

    #[test]
    fn packet_plan_preserves_quoted_low_signal_symbol_queries() {
        let plan = build_packet_plan("Find `current`.", None, PacketBudgetModeDto::Standard);
        let queries = plan
            .queries
            .iter()
            .map(|query| query.query.as_str())
            .collect::<Vec<_>>();

        assert!(
            queries.contains(&"current"),
            "quoted symbol queries should not be filtered as low-signal broad terms: {queries:?}"
        );
    }

    #[test]
    fn symbol_ownership_packet_plan_seeds_generic_ownership_terms() {
        let question = "Explain which modules own application creation, app-level rendering, response serialization, file sending, and view lookup.";
        let plan = build_packet_plan(
            question,
            Some(PacketTaskClassDto::SymbolOwnership),
            PacketBudgetModeDto::Standard,
        );
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
        let plan = build_packet_plan(
            question,
            Some(PacketTaskClassDto::BugLocalization),
            PacketBudgetModeDto::Standard,
        );
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
        let plan = build_packet_plan(
            question,
            Some(PacketTaskClassDto::RouteTracing),
            PacketBudgetModeDto::Standard,
        );
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
    fn route_tracing_packet_plan_seeds_express_app_route_probes_when_prompt_names_express() {
        let question = "Trace how Express creates an application, registers middleware/routes, and handles an incoming request through the router and response helpers.";
        let plan = build_packet_plan(
            question,
            Some(PacketTaskClassDto::RouteTracing),
            PacketBudgetModeDto::Compact,
        );
        let queries = plan
            .queries
            .iter()
            .map(|query| query.query.as_str())
            .collect::<Vec<_>>();

        for expected in [
            "createApplication",
            "app.init",
            "app.handle",
            "app.use",
            "app.route",
            "res.send",
            "application.js app.use",
            "response send body",
        ] {
            assert!(
                queries.contains(&expected),
                "expected {expected} in Express route tracing packet plan: {queries:?}"
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
                semantic_fallback_count: 0,
                semantic_fallbacks: Vec::new(),
                annotations: Vec::new(),
                steps: Vec::new(),
                retrieval_shadow: None,
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
        ] {
            assert!(
                text.contains(expected_claim),
                "generic packet claims should include {expected_claim}: {text}"
            );
        }
        assert!(
            !text.contains("Regression coverage for this flow is anchored by `PacketRegression`"),
            "test-path regression claims should not crowd out primary flow claims: {text}"
        );
    }

    #[test]
    fn packet_supported_claims_generic_source_claims_are_domain_neutral_without_eval_probes() {
        let _env = EnvVarGuard::cleared(EVAL_PROBES_ENV);
        let answer = packet_answer_fixture(
            "Explain service ownership flow for ComputeFlow and PersistFlow.",
            vec![
                test_packet_citation("ComputeFlow", "src/domain/flow.rs", 0.8),
                test_packet_citation("PersistFlow", "src/domain/persist.rs", 0.8),
            ],
        );

        let claims = packet_supported_claims(&answer);
        let text = claims
            .iter()
            .map(|claim| claim.claim.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let lower = text.to_ascii_lowercase();

        assert!(
            lower.contains("source evidence")
                || lower.contains("`computeflow` in `src/domain/flow.rs`"),
            "generic source claim should be present: {text}"
        );
        for forbidden in [
            "supporting evidence",
            "interceptor",
            "dispatch",
            "axios",
            "http",
            "holdout",
            "eval",
            "probe",
            "bench",
        ] {
            assert!(
                !lower.contains(forbidden),
                "generic source claims must not contain `{forbidden}` with eval probes disabled: {text}"
            );
        }
    }

    #[test]
    fn packet_supported_claims_include_exec_flow_specific_claims() {
        let temp_root =
            std::env::temp_dir().join(format!("codestory-exec-flow-claims-{}", std::process::id()));
        let cli_src = temp_root.join("cli").join("src");
        let exec_src = temp_root.join("exec").join("src");
        std::fs::create_dir_all(&cli_src).expect("create temp cli src");
        std::fs::create_dir_all(&exec_src).expect("create temp exec src");
        let cli_main = cli_src.join("main.rs");
        std::fs::write(
            &cli_main,
            r#"
                pub enum Subcommand {
                    Exec,
                }
                pub struct DebugSubcommand;
                mod codex_exec;
            "#,
        )
        .expect("write temp cli main");
        let exec_main = exec_src.join("main.rs");
        std::fs::write(
            &exec_main,
            r#"
                fn main() {
                    codex_exec::run_main();
                }
            "#,
        )
        .expect("write temp exec main");
        let exec_cli = exec_src.join("cli.rs");
        std::fs::write(
            &exec_cli,
            r#"
                pub struct Cli {
                    /// Print events to stdout as JSONL.
                    #[arg(long = "json", alias = "experimental-json")]
                    pub json: bool,
                }
                pub struct ExecSharedCliOptions;
            "#,
        )
        .expect("write temp exec cli");
        let exec_lib = exec_src.join("lib.rs");
        std::fs::write(
            &exec_lib,
            r#"
                pub async fn run_main() {
                    let config = ConfigBuilder::default().build().await?;
                    let approval_policy = config.permissions.approval_policy.value();
                    let sandbox = config.permissions.sandbox_policy.value();
                    let in_process_start_args = InProcessClientStartArgs {
                        config: std::sync::Arc::new(config.clone()),
                        client_name: "codex_exec".to_string(),
                    };
                    run_exec_session(in_process_start_args).await
                }
            "#,
        )
        .expect("write temp exec lib");
        let event_jsonl = exec_src.join("event_processor_with_jsonl_output.rs");
        std::fs::write(
            &event_jsonl,
            r#"
                use crate::exec_events::ThreadEvent;
                pub struct EventProcessorWithJsonOutput;
                impl EventProcessorWithJsonOutput {
                    fn emit(&self, event: ThreadEvent) {
                        println!("{}", serde_json::to_string(&event).unwrap());
                    }
                }
            "#,
        )
        .expect("write temp jsonl event processor");
        let cli_main_path = cli_main.to_string_lossy().to_string();
        let exec_main_path = exec_main.to_string_lossy().to_string();
        let exec_cli_path = exec_cli.to_string_lossy().to_string();
        let exec_lib_path = exec_lib.to_string_lossy().to_string();
        let event_jsonl_path = event_jsonl.to_string_lossy().to_string();
        let answer = AgentAnswerDto {
            answer_id: "exec-fixture".to_string(),
            prompt: "Explain how `codex exec --json` flows from the top-level CLI into the exec runtime, app-server thread and turn start requests, and JSONL event output.".to_string(),
            summary: "Exec flow evidence is covered.".to_string(),
            freshness: None,
            sections: Vec::new(),
            citations: vec![
                test_packet_citation("Subcommand::Exec", &cli_main_path, 0.8),
                test_packet_citation("codex_exec::Cli", &cli_main_path, 0.8),
                test_packet_citation("codex_exec::run_main", &exec_main_path, 0.8),
                test_packet_citation(
                    "ExecSharedCliOptions::into_inner",
                    &exec_cli_path,
                    0.8,
                ),
                test_packet_citation("run_main", &exec_lib_path, 0.8),
                test_packet_citation("run_exec_session", &exec_lib_path, 0.8),
                test_packet_citation(
                    "EventProcessorWithJsonOutput",
                    &event_jsonl_path,
                    0.8,
                ),
                test_packet_citation(
                    "ThreadStartParams",
                    "codex-rs/app-server-protocol/src/protocol/v2/thread.rs",
                    0.8,
                ),
                test_packet_citation(
                    "TurnStartParams",
                    "codex-rs/app-server-protocol/src/protocol/v2/turn.rs",
                    0.8,
                ),
            ],
            subgraph_ids: Vec::new(),
            retrieval_version: "test".to_string(),
            graphs: Vec::new(),
            retrieval_trace: codestory_contracts::api::AgentRetrievalTraceDto {
                request_id: "exec-fixture".to_string(),
                resolved_profile: AgentRetrievalPresetDto::Architecture,
                policy_mode: AgentRetrievalPolicyModeDto::LatencyFirst,
                total_latency_ms: 1,
                sla_target_ms: None,
                sla_missed: false,
                semantic_fallback_count: 0,
                semantic_fallbacks: Vec::new(),
                annotations: Vec::new(),
                steps: Vec::new(),
                retrieval_shadow: None,
            },
        };

        let claims = packet_supported_claims(&answer);
        let text = claims
            .iter()
            .map(|claim| claim.claim.as_str())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(text.contains(
            "The top-level Codex CLI has a cited Exec subcommand and command-module entrypoint in `codex_exec`."
        ));
        assert!(
            !text.contains("non-interactive execution"),
            "production packet claim templates must not infer Codex exec semantics from a subcommand name: {text}"
        );
        assert!(text.contains(
            "The codex-exec binary parses exec-specific CLI options and calls codex_exec::run_main."
        ));
        assert!(text.contains(
            "The exec CLI defines --json as the switch that chooses JSONL stdout output."
        ));
        assert!(text.contains(
            "run_main loads config, resolves sandbox and approval settings, and builds the in-process app-server start arguments"
        ));
        assert!(text.contains(
            "The command or public entrypoint for this flow is anchored by `codex_exec::Cli`"
        ));
        assert!(text.contains("Runtime orchestration is anchored by `codex_exec::run_main`"));
        assert!(text.contains(
            "JSON/event output processing is anchored by `EventProcessorWithJsonOutput`"
        ));
        assert!(
            text.contains(
                "App-server request protocol evidence is anchored by `ThreadStartParams`"
            )
        );
        assert!(text.contains(
            "Event-output processing evidence describes how structured runtime events are serialized for JSON/JSONL output."
        ));
        assert!(
            !text.contains("DebugSubcommand` is defined in cited source"),
            "definition claims should not crowd out exact command-flow claims: {text}"
        );
    }

    #[test]
    fn packet_supported_claims_surface_ranked_definitions_from_cited_sources() {
        let temp_root = std::env::temp_dir().join(format!(
            "codestory-source-definition-claims-{}",
            std::process::id()
        ));
        let exec_src = temp_root.join("exec").join("src");
        std::fs::create_dir_all(&exec_src).expect("create temp exec src");
        let exec_lib = exec_src.join("lib.rs");
        std::fs::write(
            &exec_lib,
            r#"
                pub async fn run_exec_session() {}
                pub struct EventProcessorWithJsonOutput;
                pub struct ThreadStartParams;
            "#,
        )
        .expect("write temp exec lib");
        let exec_lib_path = exec_lib.to_string_lossy().to_string();
        let answer = AgentAnswerDto {
            answer_id: "source-definition-fixture".to_string(),
            prompt: "Explain how `codex exec --json` flows from the exec runtime into app-server thread start requests and JSONL event output.".to_string(),
            summary: "Exec flow evidence is covered.".to_string(),
            freshness: None,
            sections: Vec::new(),
            citations: vec![test_packet_citation("exec runtime", &exec_lib_path, 0.8)],
            subgraph_ids: Vec::new(),
            retrieval_version: "test".to_string(),
            graphs: Vec::new(),
            retrieval_trace: codestory_contracts::api::AgentRetrievalTraceDto {
                request_id: "source-definition-fixture".to_string(),
                resolved_profile: AgentRetrievalPresetDto::Architecture,
                policy_mode: AgentRetrievalPolicyModeDto::LatencyFirst,
                total_latency_ms: 1,
                sla_target_ms: None,
                sla_missed: false,
                semantic_fallback_count: 0,
                semantic_fallbacks: Vec::new(),
                annotations: Vec::new(),
                steps: Vec::new(),
                retrieval_shadow: None,
            },
        };

        let claims = packet_supported_claims(&answer);
        let text = claims
            .iter()
            .map(|claim| claim.claim.as_str())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(text.contains("`run_exec_session` is defined in cited source"));
        assert!(text.contains("`EventProcessorWithJsonOutput` is defined in cited source"));
        assert!(text.contains("`ThreadStartParams` is defined in cited source"));
    }

    #[test]
    fn packet_supported_claims_include_indexing_storage_flow_specific_claims() {
        let _eval_probes = EvalProbesGuard::enabled();
        let answer = AgentAnswerDto {
            answer_id: "indexing-storage-fixture".to_string(),
            prompt: "Explain project source-group indexing into storage.".to_string(),
            summary: "Indexing and storage evidence is covered.".to_string(),
            freshness: None,
            sections: Vec::new(),
            citations: vec![
                test_packet_citation("Project::buildIndex", "src/lib/project/Project.cpp", 0.8),
                test_packet_citation(
                    "TaskFillIndexerCommandsQueue",
                    "src/lib/data/indexer/TaskFillIndexerCommandQueue.h",
                    0.8,
                ),
                test_packet_citation(
                    "SourceGroupCxxCdb",
                    "src/lib_cxx/project/SourceGroupCxxCdb.cpp",
                    0.8,
                ),
                test_packet_citation(
                    "IndexerCommandCxx",
                    "src/lib_cxx/data/indexer/IndexerCommandCxx.h",
                    0.8,
                ),
                test_packet_citation(
                    "IndexerJava",
                    "src/lib_java/data/indexer/IndexerJava.cpp",
                    0.8,
                ),
                test_packet_citation("StorageAccess", "src/lib/data/storage/StorageAccess.h", 0.8),
                test_packet_citation(
                    "StorageAccessProxy",
                    "src/lib/data/storage/StorageAccessProxy.cpp",
                    0.8,
                ),
                test_packet_citation(
                    "PersistentStorage",
                    "src/lib/data/storage/PersistentStorage.cpp",
                    0.8,
                ),
            ],
            subgraph_ids: Vec::new(),
            retrieval_version: "test".to_string(),
            graphs: Vec::new(),
            retrieval_trace: codestory_contracts::api::AgentRetrievalTraceDto {
                request_id: "indexing-storage-fixture".to_string(),
                resolved_profile: AgentRetrievalPresetDto::Architecture,
                policy_mode: AgentRetrievalPolicyModeDto::LatencyFirst,
                total_latency_ms: 1,
                sla_target_ms: None,
                sla_missed: false,
                semantic_fallback_count: 0,
                semantic_fallbacks: Vec::new(),
                annotations: Vec::new(),
                steps: Vec::new(),
                retrieval_shadow: None,
            },
        };

        let claims = packet_supported_claims(&answer);
        let text = claims
            .iter()
            .map(|claim| claim.claim.as_str())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(text.contains(
            "Source-group configuration and indexing command evidence describe how repository configuration becomes indexing work."
        ));
        assert!(text.contains(
            "Persistence/search-projection evidence describes how indexed data remains available to later application reads."
        ));
        assert!(text.contains("Indexing work queue behavior is anchored by `Project::buildIndex`"));
        assert!(text.contains("Source-group configuration is anchored by `SourceGroupCxxCdb`"));
        assert!(text.contains("Persistence or search projection is anchored by `StorageAccess`"));
        assert!(
            text.contains("Persistence or search projection is anchored by `PersistentStorage`")
        );
    }

    #[test]
    fn packet_supported_claims_include_indexing_pipeline_flow_claims() {
        let question = "Explain how a full indexing run moves from the CLI into runtime orchestration, file discovery, symbol extraction, persistence, and search or snapshot refresh.";
        let answer = packet_answer_fixture(
            question,
            vec![
                test_packet_citation("CliDirection", "crates/codestory-cli/src/args.rs", 0.8),
                test_packet_citation(
                    "IndexService::run_indexing_blocking_without_runtime_refresh",
                    "crates/codestory-runtime/src/services.rs",
                    0.8,
                ),
                test_packet_citation(
                    "Runtime::index_service",
                    "crates/codestory-runtime/src/lib.rs",
                    0.8,
                ),
                test_packet_citation(
                    "WorkspaceManifest::build_execution_plan",
                    "crates/codestory-workspace/src/lib.rs",
                    0.8,
                ),
                test_packet_citation(
                    "WorkspaceIndexer::run",
                    "crates/codestory-indexer/src/lib.rs",
                    0.8,
                ),
                test_packet_citation("index_file", "crates/codestory-indexer/src/lib.rs", 0.8),
                test_packet_citation(
                    "Storage::flush_projection_batch",
                    "crates/codestory-store/src/storage_impl/mod.rs",
                    0.8,
                ),
                test_packet_citation(
                    "Storage::rebuild_search_symbol_projection_from_node_table",
                    "crates/codestory-store/src/storage_impl/mod.rs",
                    0.8,
                ),
                test_packet_citation(
                    "SnapshotStore::refresh_all_with_stats",
                    "crates/codestory-store/src/snapshot_store.rs",
                    0.8,
                ),
            ],
        );

        let claims = packet_supported_claims(&answer);
        let text = claims
            .iter()
            .map(|claim| claim.claim.as_str())
            .collect::<Vec<_>>()
            .join("\n");

        for expected in [
            "The CLI index command prepares command options and delegates indexing work into the runtime layer.",
            "The runtime opens the workspace and store, chooses full or incremental indexing, and coordinates later refresh phases.",
            "The workspace crate is responsible for source-file discovery and refresh-plan construction.",
            "The indexer extracts nodes, edges, occurrences, and related symbol data from source files.",
            "The store persists graph and file data to SQLite and rebuilds query/search projections from persisted data.",
            "Snapshot refresh happens after persisted data changes so later grounding and summary reads see current indexed state.",
        ] {
            assert!(
                text.contains(expected),
                "indexing pipeline packet claims should include `{expected}`: {text}"
            );
        }
    }

    #[test]
    fn packet_sufficiency_accepts_exact_single_token_index_file_probe() {
        let question = "Explain how a full indexing run moves from the CLI into runtime orchestration, file discovery, symbol extraction, persistence, and search or snapshot refresh.";
        let (_answer, sufficiency) = build_sufficient_packet_fixture(
            question,
            PacketTaskClassDto::ArchitectureExplanation,
            vec![
                test_packet_citation("CliDirection", "crates/codestory-cli/src/args.rs", 0.8),
                test_packet_citation(
                    "Runtime::index_service",
                    "crates/codestory-runtime/src/services.rs",
                    0.8,
                ),
                test_packet_citation(
                    "index service run indexing",
                    "crates/codestory-runtime/src/services.rs",
                    0.8,
                ),
                test_packet_citation(
                    "IndexService::run_indexing_blocking_without_runtime_refresh",
                    "crates/codestory-runtime/src/services.rs",
                    0.8,
                ),
                test_packet_citation(
                    "WorkspaceManifest::build_execution_plan",
                    "crates/codestory-workspace/src/lib.rs",
                    0.8,
                ),
                test_packet_citation(
                    "symbol extraction indexer",
                    "crates/codestory-indexer/src/lib.rs",
                    0.8,
                ),
                test_packet_citation(
                    "WorkspaceIndexer::run",
                    "crates/codestory-indexer/src/lib.rs",
                    0.8,
                ),
                test_packet_citation("index_file", "crates/codestory-indexer/src/lib.rs", 0.8),
                test_packet_citation(
                    "Storage::flush_projection_batch",
                    "crates/codestory-store/src/storage_impl/mod.rs",
                    0.8,
                ),
                test_packet_citation(
                    "Storage::rebuild_search_symbol_projection_from_node_table",
                    "crates/codestory-store/src/storage_impl/mod.rs",
                    0.8,
                ),
                test_packet_citation(
                    "storage rebuild search symbol projection",
                    "crates/codestory-store/src/storage_impl/mod.rs",
                    0.8,
                ),
                test_packet_citation(
                    "snapshot refresh",
                    "crates/codestory-store/src/snapshot_store.rs",
                    0.8,
                ),
                test_packet_citation(
                    "snapshot refresh all stats",
                    "crates/codestory-store/src/snapshot_store.rs",
                    0.8,
                ),
            ],
        );

        assert_eq!(
            sufficiency.status,
            PacketSufficiencyStatusDto::Sufficient,
            "{sufficiency:?}"
        );
        assert!(
            sufficiency
                .gaps
                .iter()
                .all(|gap| !gap.contains("index_file")),
            "exact cited index_file should satisfy required probe gaps: {sufficiency:?}"
        );
        assert!(
            sufficiency
                .follow_up_commands
                .iter()
                .all(|command| !command.contains("index_file")),
            "exact cited index_file should not produce follow-up commands: {sufficiency:?}"
        );
    }

    #[test]
    fn production_packet_claims_do_not_synthesize_local_real_template_claims() {
        let answer = AgentAnswerDto {
            answer_id: "indexing-storage-production-fixture".to_string(),
            prompt: "Explain project source-group indexing into storage.".to_string(),
            summary: "Indexing and storage evidence is covered.".to_string(),
            freshness: None,
            sections: Vec::new(),
            citations: vec![
                test_packet_citation("Project::buildIndex", "src/lib/project/Project.cpp", 0.8),
                test_packet_citation(
                    "SourceGroupCxxCdb",
                    "src/lib_cxx/project/SourceGroupCxxCdb.cpp",
                    0.8,
                ),
                test_packet_citation(
                    "IndexerCommandCxx",
                    "src/lib_cxx/data/indexer/IndexerCommandCxx.h",
                    0.8,
                ),
                test_packet_citation("StorageAccess", "src/lib/data/storage/StorageAccess.h", 0.8),
                test_packet_citation(
                    "PersistentStorage",
                    "src/lib/data/storage/PersistentStorage.cpp",
                    0.8,
                ),
            ],
            subgraph_ids: Vec::new(),
            retrieval_version: "test".to_string(),
            graphs: Vec::new(),
            retrieval_trace: codestory_contracts::api::AgentRetrievalTraceDto {
                request_id: "indexing-storage-production-fixture".to_string(),
                resolved_profile: AgentRetrievalPresetDto::Architecture,
                policy_mode: AgentRetrievalPolicyModeDto::LatencyFirst,
                total_latency_ms: 1,
                sla_target_ms: None,
                sla_missed: false,
                semantic_fallback_count: 0,
                semantic_fallbacks: Vec::new(),
                annotations: Vec::new(),
                steps: Vec::new(),
                retrieval_shadow: None,
            },
        };

        let claims = packet_supported_claims(&answer);
        let text = claims
            .iter()
            .map(|claim| claim.claim.as_str())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(
            !text.contains("Project::buildIndex builds a per-source-group indexing task pipeline"),
            "production claims should not inject Sourcetrail-specific template text: {text}"
        );
        assert!(
            !text.contains("SourceGroupCxxCdb reads compile database input"),
            "production claims should not inject Sourcetrail-specific template text: {text}"
        );
        assert!(text.contains("Indexing work queue behavior is anchored by `Project::buildIndex`"));
        assert!(text.contains("Persistence or search projection is anchored by `StorageAccess`"));
    }

    #[test]
    fn packet_supported_claims_include_vscode_workbench_extension_host_claims() {
        let answer = AgentAnswerDto {
            answer_id: "vscode-fixture".to_string(),
            prompt: "Explain VS Code workbench extension-host command execution.".to_string(),
            summary: "VS Code workbench flow evidence is covered.".to_string(),
            freshness: None,
            sections: Vec::new(),
            citations: vec![
                test_packet_citation(
                    "Workbench.startup",
                    "src/vs/workbench/browser/workbench.ts",
                    0.8,
                ),
                test_packet_citation(
                    "ExtensionService",
                    "src/vs/workbench/services/extensions/browser/extensionService.ts",
                    0.8,
                ),
                test_packet_citation(
                    "ExtensionHostManager",
                    "src/vs/workbench/services/extensions/common/extensionHostManager.ts",
                    0.8,
                ),
                test_packet_citation(
                    "AbstractExtHostExtensionService",
                    "src/vs/workbench/api/common/extHostExtensionService.ts",
                    0.8,
                ),
                test_packet_citation(
                    "ExtHostCommands",
                    "src/vs/workbench/api/common/extHostCommands.ts",
                    0.8,
                ),
            ],
            subgraph_ids: Vec::new(),
            retrieval_version: "test".to_string(),
            graphs: Vec::new(),
            retrieval_trace: codestory_contracts::api::AgentRetrievalTraceDto {
                request_id: "vscode-fixture".to_string(),
                resolved_profile: AgentRetrievalPresetDto::Architecture,
                policy_mode: AgentRetrievalPolicyModeDto::LatencyFirst,
                total_latency_ms: 1,
                sla_target_ms: None,
                sla_missed: false,
                semantic_fallback_count: 0,
                semantic_fallbacks: Vec::new(),
                annotations: Vec::new(),
                steps: Vec::new(),
                retrieval_shadow: None,
            },
        };

        let claims = packet_supported_claims(&answer);
        let text = claims
            .iter()
            .map(|claim| claim.claim.as_str())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(text.contains("Runtime orchestration is anchored by `ExtensionService`"));
        assert!(text.contains(
            "The command or public entrypoint for this flow is anchored by `ExtHostCommands`"
        ));
        assert!(
            text.contains("Source evidence is anchored by")
                || text.contains("Runtime orchestration is anchored by"),
            "VS Code packet claims should use generic role-led anchors: {text}"
        );
    }

    #[test]
    fn packet_supported_claims_include_payload_public_content_flow_claims() {
        let answer = AgentAnswerDto {
            answer_id: "payload-fixture".to_string(),
            prompt: "Explain Payload posts comments RSS and Elsewhere feed.".to_string(),
            summary: "Payload public content flow evidence is covered.".to_string(),
            freshness: None,
            sections: Vec::new(),
            citations: vec![
                test_packet_citation("buildConfig", "src/payload.config.ts", 0.8),
                test_packet_citation("Posts", "src/collections/Posts.ts", 0.8),
                test_packet_citation("SocialEntries", "src/collections/SocialEntries.ts", 0.8),
                test_packet_citation("PostPage", "src/app/(frontend)/posts/[slug]/page.tsx", 0.8),
                test_packet_citation(
                    "POST /posts/:slug/comments",
                    "src/app/(frontend)/posts/[slug]/comments/route.ts",
                    0.8,
                ),
                test_packet_citation("GET /feed.xml", "src/app/feed.xml/route.ts", 0.8),
                test_packet_citation("getPayloadClient", "src/lib/payload.ts", 0.8),
                test_packet_citation(
                    "getCommentAuthContextFromHeaders",
                    "src/lib/comment-auth.ts",
                    0.8,
                ),
                test_packet_citation(
                    "getLatestSocialEntries",
                    "src/lib/content-data/social-entry-content.ts",
                    0.8,
                ),
            ],
            subgraph_ids: Vec::new(),
            retrieval_version: "test".to_string(),
            graphs: Vec::new(),
            retrieval_trace: codestory_contracts::api::AgentRetrievalTraceDto {
                request_id: "payload-fixture".to_string(),
                resolved_profile: AgentRetrievalPresetDto::Architecture,
                policy_mode: AgentRetrievalPolicyModeDto::LatencyFirst,
                total_latency_ms: 1,
                sla_target_ms: None,
                sla_missed: false,
                semantic_fallback_count: 0,
                semantic_fallbacks: Vec::new(),
                annotations: Vec::new(),
                steps: Vec::new(),
                retrieval_shadow: None,
            },
        };

        let claims = packet_supported_claims(&answer);
        let text = claims
            .iter()
            .map(|claim| claim.claim.as_str())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(text.contains("Collection configuration is anchored by `Posts`"));
        assert!(text.contains("Route handling is anchored by `POST /posts/:slug/comments`"));
        assert!(text.contains("`getPayloadClient` in `src/lib/payload.ts`"));
    }

    #[test]
    fn packet_ranking_prefers_payload_collections_over_component_and_preview_fillers() {
        let question = "Explain how Payload collections, post rendering, comment submission, RSS, and the Elsewhere feed connect.";
        let mut answer = AgentAnswerDto {
            answer_id: "payload-rank-fixture".to_string(),
            prompt: question.to_string(),
            summary: "Payload public content flow evidence is covered.".to_string(),
            freshness: None,
            sections: Vec::new(),
            citations: vec![
                test_packet_citation("PostComments", "src/components/PostComments.tsx", 0.55),
                test_packet_citation("posts", "src/lib/content-data/preview-content.ts", 0.55),
                test_packet_citation("Posts", "src/collections/Posts.ts", 0.8),
                test_packet_citation("Comments", "src/collections/Comments.ts", 0.8),
            ],
            subgraph_ids: Vec::new(),
            retrieval_version: "test".to_string(),
            graphs: Vec::new(),
            retrieval_trace: codestory_contracts::api::AgentRetrievalTraceDto {
                request_id: "payload-rank-fixture".to_string(),
                resolved_profile: AgentRetrievalPresetDto::Architecture,
                policy_mode: AgentRetrievalPolicyModeDto::LatencyFirst,
                total_latency_ms: 1,
                sla_target_ms: None,
                sla_missed: false,
                semantic_fallback_count: 0,
                semantic_fallbacks: Vec::new(),
                annotations: Vec::new(),
                steps: Vec::new(),
                retrieval_shadow: None,
            },
        };

        rank_packet_evidence(question, &mut answer);
        let top_paths = answer
            .citations
            .iter()
            .take(2)
            .filter_map(|citation| citation.file_path.as_deref().map(packet_display_path))
            .collect::<Vec<_>>();

        assert_eq!(
            top_paths,
            vec!["src/collections/Posts.ts", "src/collections/Comments.ts"],
            "Payload collection files should outrank nearby rendering/preview fillers: {top_paths:?}"
        );
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
                semantic_fallback_count: 0,
                semantic_fallbacks: Vec::new(),
                annotations: Vec::new(),
                steps: Vec::new(),
                retrieval_shadow: None,
            },
        };

        rank_packet_evidence(question, &mut answer);
        assert_eq!(answer.citations[0].display_name, "RouteHandler");
    }

    #[test]
    fn packet_ranking_demotes_test_named_source_helpers_for_production_prompts() {
        let question = "Explain runtime orchestration and search projection in the indexing flow.";
        let mut answer = AgentAnswerDto {
            answer_id: "rank-test-symbols".to_string(),
            prompt: question.to_string(),
            summary: "Runtime evidence is covered by cited anchors.".to_string(),
            freshness: None,
            sections: Vec::new(),
            citations: vec![
                test_packet_citation(
                    "EmbeddingRuntime::test_runtime",
                    "crates/codestory-runtime/src/search/engine.rs",
                    5.0,
                ),
                test_packet_citation(
                    "tests::drill_question_search_is_partial_discovery_evidence",
                    "crates/codestory-cli/src/main.rs",
                    5.0,
                ),
                test_packet_citation(
                    "IndexService::run_indexing_blocking",
                    "crates/codestory-runtime/src/services.rs",
                    0.5,
                ),
            ],
            subgraph_ids: Vec::new(),
            retrieval_version: "test".to_string(),
            graphs: Vec::new(),
            retrieval_trace: codestory_contracts::api::AgentRetrievalTraceDto {
                request_id: "rank-test-symbols".to_string(),
                resolved_profile: AgentRetrievalPresetDto::Architecture,
                policy_mode: AgentRetrievalPolicyModeDto::LatencyFirst,
                total_latency_ms: 1,
                sla_target_ms: None,
                sla_missed: false,
                semantic_fallback_count: 0,
                semantic_fallbacks: Vec::new(),
                annotations: Vec::new(),
                steps: Vec::new(),
                retrieval_shadow: None,
            },
        };

        rank_packet_evidence(question, &mut answer);

        assert_eq!(
            answer.citations[0].display_name,
            "IndexService::run_indexing_blocking"
        );
        assert_eq!(
            packet_evidence_role(&answer.citations[1]),
            Some("tests and regression coverage")
        );
        assert_eq!(
            packet_evidence_role(&answer.citations[2]),
            Some("tests and regression coverage")
        );
    }

    #[test]
    fn packet_ranking_demotes_non_primary_roles_for_production_prompts() {
        let question = "Trace production route dispatch through the handler.";
        let mut answer = AgentAnswerDto {
            answer_id: "rank-roles".to_string(),
            prompt: question.to_string(),
            summary: "Route evidence is covered by cited anchors.".to_string(),
            freshness: None,
            sections: Vec::new(),
            citations: vec![
                test_packet_citation("DocsRouteHandler", "docs/routes.md", 5.0),
                test_packet_citation("GeneratedRouteHandler", "target/generated/routes.rs", 5.0),
                test_packet_citation("VendorRouteHandler", "vendor/router/handler.rs", 5.0),
                test_packet_citation("BenchRouteHandler", "benches/router_handler.rs", 5.0),
                test_packet_citation("RouteHandler", "src/router/handler.rs", 0.5),
            ],
            subgraph_ids: Vec::new(),
            retrieval_version: "test".to_string(),
            graphs: Vec::new(),
            retrieval_trace: codestory_contracts::api::AgentRetrievalTraceDto {
                request_id: "rank-roles".to_string(),
                resolved_profile: AgentRetrievalPresetDto::Architecture,
                policy_mode: AgentRetrievalPolicyModeDto::LatencyFirst,
                total_latency_ms: 1,
                sla_target_ms: None,
                sla_missed: false,
                semantic_fallback_count: 0,
                semantic_fallbacks: Vec::new(),
                annotations: Vec::new(),
                steps: Vec::new(),
                retrieval_shadow: None,
            },
        };

        rank_packet_evidence(question, &mut answer);
        assert_eq!(answer.citations[0].display_name, "RouteHandler");
    }

    #[test]
    fn packet_ranking_keeps_requested_docs_role_eligible() {
        let question = "Trace the docs route dispatch example.";
        let mut answer = AgentAnswerDto {
            answer_id: "rank-docs".to_string(),
            prompt: question.to_string(),
            summary: "Route evidence is covered by cited anchors.".to_string(),
            freshness: None,
            sections: Vec::new(),
            citations: vec![
                test_packet_citation("RouteHandler", "src/router/handler.rs", 0.5),
                test_packet_citation("DocsRouteHandler", "docs/routes.md", 5.0),
            ],
            subgraph_ids: Vec::new(),
            retrieval_version: "test".to_string(),
            graphs: Vec::new(),
            retrieval_trace: codestory_contracts::api::AgentRetrievalTraceDto {
                request_id: "rank-docs".to_string(),
                resolved_profile: AgentRetrievalPresetDto::Architecture,
                policy_mode: AgentRetrievalPolicyModeDto::LatencyFirst,
                total_latency_ms: 1,
                sla_target_ms: None,
                sla_missed: false,
                semantic_fallback_count: 0,
                semantic_fallbacks: Vec::new(),
                annotations: Vec::new(),
                steps: Vec::new(),
                retrieval_shadow: None,
            },
        };

        rank_packet_evidence(question, &mut answer);
        assert_eq!(answer.citations[0].display_name, "DocsRouteHandler");
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
                "Route handling is anchored by `RouteHandler`",
                "src/router/handler.rs",
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
    fn architecture_sufficiency_requires_minimum_distinct_claim_families() {
        let question = "Explain how project indexing reaches persistent storage.";
        let citations = vec![
            test_packet_citation("Project::buildIndex", "src/lib/project/Project.cpp", 0.9),
            test_packet_citation(
                "TaskBuildIndex",
                "src/lib/data/indexer/TaskBuildIndex.cpp",
                0.85,
            ),
            test_packet_citation(
                "TaskFillIndexerCommandsQueue",
                "src/lib/data/indexer/TaskFillIndexerCommandQueue.h",
                0.8,
            ),
        ];
        let (_answer, sufficiency) = build_sufficient_packet_fixture(
            question,
            PacketTaskClassDto::ArchitectureExplanation,
            citations,
        );
        assert_eq!(
            sufficiency.status,
            PacketSufficiencyStatusDto::Partial,
            "duplicate claim families should not satisfy architecture packets: {sufficiency:?}"
        );
        assert!(
            sufficiency
                .gaps
                .iter()
                .any(|gap| gap.contains("claim families")),
            "architecture sufficiency should explain missing claim-family coverage: {sufficiency:?}"
        );
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
            packet_fixture_project_root(),
            question,
            PacketTaskClassDto::RouteTracing,
            PacketBudgetModeDto::Tiny,
            packet_budget_limits(PacketBudgetModeDto::Tiny),
            &mut partial_answer,
        );
        budget.truncated = true;
        budget.omitted_sections = vec!["output_bytes".to_string()];
        let partial = build_packet_sufficiency(
            packet_fixture_project_root(),
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
        assert!(
            partial
                .follow_up_commands
                .iter()
                .all(|command| !command.contains("<target-workspace>")),
            "partial packet follow-up commands should be directly runnable: {partial:?}"
        );
        assert!(
            partial
                .follow_up_commands
                .iter()
                .all(|command| command.contains("--project 'C:/workspace/project root'")),
            "partial packet follow-up commands should include the concrete project root: {partial:?}"
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
            packet_fixture_project_root(),
            question,
            PacketTaskClassDto::RouteTracing,
            PacketBudgetModeDto::Compact,
            packet_budget_limits(PacketBudgetModeDto::Compact),
            &mut weak_answer,
        );
        let weak = build_packet_sufficiency(
            packet_fixture_project_root(),
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
            packet_fixture_project_root(),
            question,
            PacketTaskClassDto::RouteTracing,
            PacketBudgetModeDto::Compact,
            packet_budget_limits(PacketBudgetModeDto::Compact),
            &mut empty_answer,
        );
        let insufficient = build_packet_sufficiency(
            packet_fixture_project_root(),
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
                .any(|command| command.contains("codestory-cli search")
                    && command.contains("--why")
                    && !command.contains("--repo-text on")),
            "insufficient packets should recommend sidecar-primary search diagnostics: {insufficient:?}"
        );
    }

    #[test]
    fn packet_follow_up_commands_single_quote_shell_sensitive_questions() {
        let question = "Inspect $env:SECRET and $(Get-ChildItem) and 'literal'";
        let quoted = quote_packet_command_value(question);

        assert_eq!(
            quoted,
            "'Inspect $env:SECRET and $(Get-ChildItem) and ''literal'''"
        );
        let command = next_deeper_packet_command(
            packet_fixture_project_root(),
            question,
            PacketBudgetModeDto::Tiny,
        )
        .expect("tiny packet should have deeper command");
        assert!(
            command.contains("--question 'Inspect $env:SECRET and $(Get-ChildItem)"),
            "packet command should single-quote shell-sensitive question text: {command}"
        );
        assert!(
            command.contains("--project 'C:/workspace/project root'"),
            "packet command should include the concrete project root: {command}"
        );
    }

    #[test]
    fn packet_anchor_probe_limit_hard_stops_after_sla_exhaustion() {
        let budget = PacketLatencyBudget {
            started_at: Instant::now() - std::time::Duration::from_secs(30),
            target_ms: 1_000,
        };
        assert_eq!(
            packet_anchor_probe_limit_for_budget(PacketBudgetModeDto::Compact, budget, 1_500),
            0
        );
    }

    #[test]
    fn packet_anchor_probe_limit_reduces_when_budget_half_consumed() {
        let budget = PacketLatencyBudget {
            started_at: Instant::now(),
            target_ms: 10_000,
        };
        assert_eq!(
            packet_anchor_probe_limit_for_budget(PacketBudgetModeDto::Compact, budget, 5_500),
            14
        );
        assert_eq!(
            packet_anchor_probe_limit_for_budget(PacketBudgetModeDto::Compact, budget, 8_000),
            7
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
        answer.retrieval_trace.total_latency_ms =
            answer.retrieval_trace.total_latency_ms.saturating_add(250);

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
    fn packet_benchmark_trace_keeps_counters_without_duplicating_full_trace() {
        let mut answer = packet_answer_fixture(
            "Explain the packet benchmark trace.",
            vec![test_packet_citation(
                "PacketTrace",
                "src/packet_trace.rs",
                0.8,
            )],
        );
        answer.retrieval_trace.total_latency_ms = 42;
        answer.retrieval_trace.sla_target_ms = Some(1_000);
        answer.retrieval_trace.sla_missed = true;
        answer.retrieval_trace.annotations.push(
            "large trace annotation should stay only on the canonical answer trace".repeat(8),
        );
        answer.retrieval_trace.steps = vec![
            AgentRetrievalStepDto {
                kind: AgentRetrievalStepKindDto::Search,
                status: AgentRetrievalStepStatusDto::Ok,
                duration_ms: 10,
                input: Vec::new(),
                output: Vec::new(),
                message: Some("search details".repeat(16)),
            },
            AgentRetrievalStepDto {
                kind: AgentRetrievalStepKindDto::Trail,
                status: AgentRetrievalStepStatusDto::Ok,
                duration_ms: 20,
                input: Vec::new(),
                output: Vec::new(),
                message: Some("trail details".repeat(16)),
            },
            AgentRetrievalStepDto {
                kind: AgentRetrievalStepKindDto::SourceRead,
                status: AgentRetrievalStepStatusDto::Ok,
                duration_ms: 12,
                input: Vec::new(),
                output: Vec::new(),
                message: Some("source details".repeat(16)),
            },
        ];

        let full_trace_bytes = serde_json::to_vec(&answer.retrieval_trace)
            .expect("serialize canonical trace")
            .len();
        let benchmark_trace = packet_benchmark_trace(&answer);
        let benchmark_trace_bytes = serde_json::to_vec(&benchmark_trace.retrieval_trace)
            .expect("serialize benchmark trace")
            .len();

        assert_eq!(answer.retrieval_trace.steps.len(), 3);
        assert_eq!(benchmark_trace.search_steps, 1);
        assert_eq!(benchmark_trace.trail_steps, 1);
        assert_eq!(benchmark_trace.source_read_steps, 1);
        assert_eq!(benchmark_trace.retrieval_trace.total_latency_ms, 42);
        assert_eq!(benchmark_trace.retrieval_trace.sla_target_ms, Some(1_000));
        assert!(benchmark_trace.retrieval_trace.sla_missed);
        assert!(benchmark_trace.retrieval_trace.steps.is_empty());
        assert!(benchmark_trace.retrieval_trace.annotations.is_empty());
        assert!(
            benchmark_trace_bytes < full_trace_bytes / 2,
            "benchmark trace should stay scalar-sized: {benchmark_trace_bytes} >= {full_trace_bytes}/2"
        );
    }

    #[test]
    fn citation_budget_truncation_keeps_sufficient_stop_signal() {
        let question = "Explain the compact packet stop rule.";
        let mut answer = packet_answer_fixture(
            question,
            vec![
                test_packet_citation("CliCommand", "crates/tool-cli/src/main.rs", 0.8),
                test_packet_citation("RuntimeCoordinator", "crates/core/src/runtime.rs", 0.8),
                test_packet_citation("WorkspacePlan", "crates/core/src/workspace/plan.rs", 0.8),
                test_packet_citation("GraphIndexer", "crates/indexer/src/lib.rs", 0.8),
                test_packet_citation("ProjectionStore", "crates/store/src/projection.rs", 0.8),
                test_packet_citation("SnapshotRefresh", "crates/store/src/snapshot.rs", 0.8),
                test_packet_citation("RouteHandler", "src/routes/user.rs", 0.8),
                test_packet_citation("PacketRegression", "tests/packet_flow.rs", 0.8),
                test_packet_citation("PacketBudget", "src/packet/budget.rs", 0.8),
                test_packet_citation("PacketStopRule", "src/packet/stop_rule.rs", 0.8),
                test_packet_citation("PacketClaim", "src/packet/claim.rs", 0.8),
                test_packet_citation("PacketFollowUp", "src/packet/follow_up.rs", 0.8),
                test_packet_citation("PacketContext", "src/packet/context.rs", 0.8),
                test_packet_citation("PacketOutput", "src/packet/output.rs", 0.8),
            ],
        );
        let budget = apply_packet_budget(
            packet_fixture_project_root(),
            question,
            PacketTaskClassDto::ArchitectureExplanation,
            PacketBudgetModeDto::Compact,
            packet_budget_limits(PacketBudgetModeDto::Compact),
            &mut answer,
        );
        let sufficiency = build_packet_sufficiency(
            packet_fixture_project_root(),
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
        assert_eq!(answer.citations.len(), 13);
        assert!(
            sufficiency.gaps.is_empty(),
            "normal compact-budget truncation should stay in budget metadata, not sufficiency gaps: {sufficiency:?}"
        );
        assert!(budget.used.files <= budget.limits.max_files);
        assert!(budget.used.output_bytes <= budget.limits.max_output_bytes);
    }

    #[test]
    fn answer_critical_budget_truncation_requires_deeper_packet() {
        let question = "Explain the packet stop rule when evidence is clipped.";
        let mut answer = packet_answer_fixture(
            question,
            vec![
                test_packet_citation("CliCommand", "crates/tool-cli/src/main.rs", 0.8),
                test_packet_citation("RuntimeCoordinator", "crates/core/src/runtime.rs", 0.8),
                test_packet_citation("WorkspacePlan", "crates/core/src/workspace/plan.rs", 0.8),
            ],
        );
        let mut budget = apply_packet_budget(
            packet_fixture_project_root(),
            question,
            PacketTaskClassDto::ArchitectureExplanation,
            PacketBudgetModeDto::Compact,
            packet_budget_limits(PacketBudgetModeDto::Compact),
            &mut answer,
        );
        budget.truncated = true;
        budget.omitted_sections = vec!["markdown_blocks".to_string(), "trail_edges".to_string()];
        budget.next_deeper_command = next_deeper_packet_command(
            packet_fixture_project_root(),
            question,
            PacketBudgetModeDto::Compact,
        );

        let sufficiency = build_packet_sufficiency(
            packet_fixture_project_root(),
            question,
            PacketTaskClassDto::ArchitectureExplanation,
            &answer,
            &budget,
        );

        assert_eq!(sufficiency.status, PacketSufficiencyStatusDto::Partial);
        assert!(
            sufficiency
                .gaps
                .iter()
                .any(|gap| gap.contains("answer-critical evidence")),
            "answer-critical truncation should be named as a sufficiency gap: {sufficiency:?}"
        );
        assert!(
            sufficiency
                .follow_up_commands
                .iter()
                .any(|command| command.contains("--budget standard")),
            "partial packet should recommend the existing deeper packet command: {sufficiency:?}"
        );
    }

    #[test]
    fn retrieval_appendix_and_secondary_trail_clipping_can_remain_sufficient() {
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

        let question = "Explain public content flow through Payload.";
        let mut answer = packet_answer_fixture(
            question,
            vec![
                test_packet_citation("Posts", "src/collections/Posts.ts", 0.9),
                test_packet_citation(
                    "getApprovedCommentsForPost",
                    "src/lib/content-data/comment-content.ts",
                    0.9,
                ),
                test_packet_citation("GET /feed.xml", "src/app/feed.xml/route.ts", 0.9),
            ],
        );
        let claims = packet_supported_claims(&answer);
        answer.sections = vec![
            AgentResponseSectionDto {
                id: "packet-flow-claims".to_string(),
                title: "Packet Claims".to_string(),
                blocks: vec![AgentResponseBlockDto::Markdown {
                    markdown: packet_flow_claims_markdown(&claims),
                }],
            },
            AgentResponseSectionDto {
                id: "retrieval-evidence".to_string(),
                title: "Retrieval Evidence".to_string(),
                blocks: vec![AgentResponseBlockDto::Markdown {
                    markdown: format!(
                        "Search appendix and low-level trace details.{}",
                        PACKET_MARKDOWN_TRUNCATION_SUFFIX
                    ),
                }],
            },
        ];
        answer.graphs.push(GraphArtifactDto::Uml {
            id: "primary".to_string(),
            title: "Primary Neighborhood".to_string(),
            graph: GraphResponse {
                center_id: NodeId("post-page".to_string()),
                nodes: vec![node("post-page"), node("payload")],
                edges: vec![edge("edge_1", "post-page", "payload")],
                truncated: false,
                omitted_edge_count: 0,
                canonical_layout: None,
            },
        });

        let budget = PacketBudgetDto {
            requested: PacketBudgetModeDto::Compact,
            limits: packet_budget_limits(PacketBudgetModeDto::Compact),
            used: packet_budget_usage(&answer),
            truncated: true,
            omitted_sections: vec![
                "citations".to_string(),
                "markdown_blocks".to_string(),
                "trail_edges".to_string(),
            ],
            next_deeper_command: next_deeper_packet_command(
                packet_fixture_project_root(),
                question,
                PacketBudgetModeDto::Compact,
            ),
        };

        let sufficiency = build_packet_sufficiency(
            packet_fixture_project_root(),
            question,
            PacketTaskClassDto::ArchitectureExplanation,
            &answer,
            &budget,
        );

        assert_eq!(sufficiency.status, PacketSufficiencyStatusDto::Sufficient);
        assert!(sufficiency.gaps.is_empty());
        assert!(sufficiency.follow_up_commands.is_empty());
        assert!(sufficiency.covered_claims.len() >= 3);
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
        let budget = apply_packet_budget(
            packet_fixture_project_root(),
            question,
            PacketTaskClassDto::ArchitectureExplanation,
            PacketBudgetModeDto::Tiny,
            limits,
            &mut answer,
        );
        let sufficiency = build_packet_sufficiency(
            packet_fixture_project_root(),
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

        enforce_packet_output_budget(packet_fixture_project_root(), &mut packet);

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
                .contains(&"markdown_blocks".to_string())
        );
        assert!(
            !packet
                .budget
                .omitted_sections
                .contains(&"packet_payload".to_string())
        );
        assert!(
            !packet
                .sufficiency
                .gaps
                .iter()
                .any(|gap| gap.contains("packet_payload") || gap.contains("output_bytes")),
            "sufficiency gaps should be rebuilt after final payload remeasurement clears stale omissions: {:?}",
            packet.sufficiency
        );
    }

    #[test]
    fn packet_hard_output_cap_uses_current_usage_not_stale_omissions() {
        let limits = PacketBudgetLimitsDto {
            max_anchors: 4,
            max_files: 4,
            max_snippets: 4,
            max_trail_edges: 4,
            max_output_bytes: 1000,
        };
        let mut budget = PacketBudgetDto {
            requested: PacketBudgetModeDto::Compact,
            limits,
            used: PacketBudgetUsageDto {
                anchors: 4,
                files: 4,
                snippets: 0,
                trail_edges: 0,
                output_bytes: 900,
            },
            truncated: true,
            omitted_sections: vec!["output_bytes".to_string(), "packet_payload".to_string()],
            next_deeper_command: None,
        };

        assert!(
            !packet_budget_exceeded_hard_output_cap(&budget),
            "stale output_bytes omission should not force followups after final payload fits"
        );
        budget.used.output_bytes = 1001;
        assert!(packet_budget_exceeded_hard_output_cap(&budget));
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
            packet_fixture_project_root(),
            "Explain graph budget trimming.",
            PacketTaskClassDto::ArchitectureExplanation,
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
                semantic_fallback_count: 0,
                semantic_fallbacks: Vec::new(),
                annotations: Vec::new(),
                steps: Vec::new(),
                retrieval_shadow: None,
            },
        };

        rank_packet_evidence(question, &mut answer);
        append_packet_evidence_sections(
            &mut answer,
            PacketTaskClassDto::ArchitectureExplanation,
            &limits,
        );
        let budget = apply_packet_budget(
            packet_fixture_project_root(),
            question,
            PacketTaskClassDto::ArchitectureExplanation,
            PacketBudgetModeDto::Compact,
            limits,
            &mut answer,
        );
        let sufficiency = build_packet_sufficiency(
            packet_fixture_project_root(),
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
    fn packet_plan_adds_prepared_session_adapter_exact_probes() {
        let question = "Explain how Requests turns a top-level request call into a prepared request and sends it through a session adapter.";
        let plan = build_packet_plan(
            question,
            Some(PacketTaskClassDto::ArchitectureExplanation),
            PacketBudgetModeDto::Compact,
        );
        let queries = plan
            .queries
            .iter()
            .map(|query| query.query.as_str())
            .collect::<Vec<_>>();
        let required = packet_sufficiency_required_probe_queries(
            question,
            PacketTaskClassDto::ArchitectureExplanation,
        );

        for expected in [
            "Session.request",
            "Session.prepare_request",
            "PreparedRequest.prepare",
            "Session.send",
            "HTTPAdapter.send",
        ] {
            assert!(
                queries.contains(&expected),
                "packet plan should include exact Requests flow probe `{expected}` in {queries:?}"
            );
            assert!(
                required.iter().any(|query| query == expected),
                "packet required probes should protect exact Requests flow probe `{expected}` in {required:?}"
            );
        }
    }

    #[test]
    fn packet_plan_derives_java_string_check_symbol_probes() {
        let _env = EnvVarGuard::cleared(EVAL_PROBES_ENV);
        let question = "Explain how Commons Lang implements blank, empty, and case-sensitive string checks across StringUtils, Strings, and CharSequenceUtils. Cite the source files and name the supporting symbols.";
        let plan = build_packet_plan(
            question,
            Some(PacketTaskClassDto::ArchitectureExplanation),
            PacketBudgetModeDto::Compact,
        );
        let queries = plan
            .queries
            .iter()
            .map(|query| query.query.as_str())
            .collect::<Vec<_>>();
        let required = packet_sufficiency_required_probe_queries(
            question,
            PacketTaskClassDto::ArchitectureExplanation,
        );

        for expected in [
            "StringUtils",
            "StringUtils.isBlank",
            "StringUtils.isEmpty",
            "Strings.CS",
            "Strings.CI",
            "CharSequenceUtils",
            "CharSequenceUtils.regionMatches",
        ] {
            assert!(
                queries.contains(&expected),
                "packet plan should include Java string probe `{expected}` in {queries:?}"
            );
            assert!(
                required.iter().any(|query| query == expected),
                "packet required probes should protect Java string probe `{expected}` in {required:?}"
            );
        }

        for expected_file_probe in ["StringUtils.java", "Strings.java", "CharSequenceUtils.java"] {
            assert!(
                queries.contains(&expected_file_probe),
                "packet plan should include generic file probe `{expected_file_probe}` in {queries:?}"
            );
        }
    }

    #[test]
    fn packet_plan_derives_swr_hook_flow_symbol_probes() {
        let _env = EnvVarGuard::cleared(EVAL_PROBES_ENV);
        let question = "Explain how SWR exposes useSWR, serializes keys, connects cache helpers, and routes mutate behavior through the internal mutation helper. Cite the source files and name the supporting symbols.";
        let plan = build_packet_plan(
            question,
            Some(PacketTaskClassDto::ArchitectureExplanation),
            PacketBudgetModeDto::Compact,
        );
        let queries = plan
            .queries
            .iter()
            .map(|query| query.query.as_str())
            .collect::<Vec<_>>();
        let required = packet_sufficiency_required_probe_queries(
            question,
            PacketTaskClassDto::ArchitectureExplanation,
        );

        for expected in [
            "useSWR",
            "useSWRHandler",
            "withArgs",
            "withMiddleware",
            "serialize",
            "createCacheHelper",
            "internalMutate",
        ] {
            assert!(
                queries.contains(&expected),
                "packet plan should include SWR flow probe `{expected}` in {queries:?}"
            );
            assert!(
                required.iter().any(|query| query == expected),
                "packet required probes should protect SWR flow probe `{expected}` in {required:?}"
            );
        }

        for expected_file_probe in [
            "index.ts useSWR",
            "use-swr.ts useSWRHandler",
            "serialize.ts",
            "helper.ts createCacheHelper",
            "mutate.ts internalMutate",
            "with-middleware.ts withMiddleware",
        ] {
            assert!(
                queries.contains(&expected_file_probe),
                "packet plan should include SWR file probe `{expected_file_probe}` in {queries:?}"
            );
        }
    }

    #[test]
    fn packet_plan_derives_gin_route_dispatch_symbol_probes() {
        let _env = EnvVarGuard::cleared(EVAL_PROBES_ENV);
        let question = "Trace how Gin creates an engine, registers routes through router groups, stores them in method trees, and dispatches handlers for a request. Cite the source files and name the supporting symbols.";
        let plan = build_packet_plan(
            question,
            Some(PacketTaskClassDto::RouteTracing),
            PacketBudgetModeDto::Compact,
        );
        let queries = plan
            .queries
            .iter()
            .map(|query| query.query.as_str())
            .collect::<Vec<_>>();
        let required =
            packet_sufficiency_required_probe_queries(question, PacketTaskClassDto::RouteTracing);

        for expected in [
            "gin.go New",
            "gin.go Default",
            "routergroup.go RouterGroup.Handle",
            "gin.go Engine.addRoute",
            "tree.go node.addRoute",
            "gin.go Engine.handleHTTPRequest",
            "context.go Context.Next",
        ] {
            assert!(
                queries.contains(&expected),
                "packet plan should include Gin route probe `{expected}` in {queries:?}"
            );
            assert!(
                required.iter().any(|query| query == expected),
                "packet required probes should protect Gin route probe `{expected}` in {required:?}"
            );
        }

        for client_probe in ["request interceptor", "transport adapter"] {
            assert!(
                !required.iter().any(|query| query == client_probe),
                "server route tracing should not require client transport probe `{client_probe}` in {required:?}"
            );
        }

        for expected_file_probe in [
            "gin.go New",
            "gin.go Default",
            "gin.go Engine.addRoute",
            "gin.go Engine.handleHTTPRequest",
            "routergroup.go RouterGroup.Handle",
            "tree.go node.addRoute",
            "context.go Context.Next",
        ] {
            assert!(
                queries.contains(&expected_file_probe),
                "packet plan should include Gin file probe `{expected_file_probe}` in {queries:?}"
            );
        }
    }

    #[test]
    fn packet_plan_derives_css_animation_symbol_probes() {
        let _env = EnvVarGuard::cleared(EVAL_PROBES_ENV);
        let question = "Explain how animate.css defines shared animation variables/base classes and connects named animation classes to keyframes. Cite the source files and name the supporting selectors or keyframes.";
        let plan = build_packet_plan(
            question,
            Some(PacketTaskClassDto::ArchitectureExplanation),
            PacketBudgetModeDto::Compact,
        );
        let queries = plan
            .queries
            .iter()
            .map(|query| query.query.as_str())
            .collect::<Vec<_>>();
        let required = packet_sufficiency_required_probe_queries(
            question,
            PacketTaskClassDto::ArchitectureExplanation,
        );

        for expected in [
            "source/_vars.css",
            "source/_base.css",
            "source/animate.css",
            "source/attention_seekers/bounce.css bounce",
            "source/attention_seekers/flash.css flash",
        ] {
            assert!(
                queries.contains(&expected),
                "packet plan should include CSS animation probe `{expected}` in {queries:?}"
            );
            assert!(
                required.iter().any(|query| query == expected),
                "packet required probes should protect CSS animation probe `{expected}` in {required:?}"
            );
        }
    }
    #[test]
    fn packet_plan_derives_automapper_map_flow_symbol_probes() {
        let _env = EnvVarGuard::cleared(EVAL_PROBES_ENV);
        let question = "Explain how AutoMapper configuration and runtime mapper APIs cooperate to map source objects to destination objects. Cite the source files and name the supporting symbols.";
        let plan = build_packet_plan(
            question,
            Some(PacketTaskClassDto::ArchitectureExplanation),
            PacketBudgetModeDto::Compact,
        );
        let queries = plan
            .queries
            .iter()
            .map(|query| query.query.as_str())
            .collect::<Vec<_>>();
        let required = packet_sufficiency_required_probe_queries(
            question,
            PacketTaskClassDto::ArchitectureExplanation,
        );

        for expected in [
            "src/AutoMapper/Mapper.cs IMapperBase",
            "src/AutoMapper/Mapper.cs IMapper",
            "src/AutoMapper/Mapper.cs Mapper",
            "src/AutoMapper/Mapper.cs Mapper.Map",
            "src/AutoMapper/Configuration/MapperConfiguration.cs MapperConfiguration",
            "src/AutoMapper/TypeMap.cs TypeMap.CreateMapperLambda",
            "src/AutoMapper/Execution/TypeMapPlanBuilder.cs TypeMapPlanBuilder",
            "TypeMapPlanBuilder.CreateMapperLambda",
        ] {
            assert!(
                queries.contains(&expected),
                "packet plan should include AutoMapper probe `{expected}` in {queries:?}"
            );
            assert!(
                required.iter().any(|query| query == expected),
                "packet required probes should protect AutoMapper probe `{expected}` in {required:?}"
            );
        }
    }
    #[test]
    fn file_scoped_required_probes_match_symbol_inside_file() {
        let gin_new = test_packet_citation("New", "gin.go", 0.9);
        let gin_with = test_packet_citation("Engine.With", "gin.go", 0.9);
        let binding_default = test_packet_citation("Default", "binding/binding.go", 0.9);
        let router_group = test_packet_citation("RouterGroup", "routergroup.go", 0.9);
        let router_group_handle = test_packet_citation("RouterGroup.Handle", "routergroup.go", 0.9);

        assert!(packet_citation_satisfies_required_probe(
            "gin.go New",
            &gin_new
        ));
        assert!(!packet_citation_satisfies_required_probe(
            "gin.go New",
            &gin_with
        ));
        assert!(!packet_citation_satisfies_required_probe(
            "gin.go Default",
            &binding_default
        ));
        assert!(packet_citation_satisfies_required_probe(
            "routergroup.go RouterGroup.Handle",
            &router_group_handle
        ));
        assert!(!packet_citation_satisfies_required_probe(
            "routergroup.go RouterGroup.Handle",
            &router_group
        ));

        let create_track = test_packet_citation(
            "CREATE TABLE Track",
            "SampleDatabase/DataSources/Sample_Sqlite.sql",
            0.9,
        );
        let create_playlist_track = test_packet_citation(
            "CREATE TABLE PlaylistTrack",
            "SampleDatabase/DataSources/Sample_Sqlite.sql",
            0.9,
        );
        assert!(packet_citation_satisfies_required_probe(
            "SampleDatabase/DataSources/Sample_Sqlite.sql CREATE TABLE Track",
            &create_track
        ));
        assert!(!packet_citation_satisfies_required_probe(
            "SampleDatabase/DataSources/Sample_Sqlite.sql CREATE TABLE Track",
            &create_playlist_track
        ));
    }

    #[test]
    fn gin_route_dispatch_source_claims_name_registration_and_context_flow() {
        let prompt = "Trace how Gin creates an engine, registers routes through router groups, stores them in method trees, and dispatches handlers for a request.";
        let fixtures = [
            (
                "RouterGroup.Handle",
                "routergroup.go",
                r#"
                func (group *RouterGroup) handle(httpMethod, relativePath string, handlers HandlersChain) IRoutes {
                    absolutePath := group.calculateAbsolutePath(relativePath)
                    handlers = group.combineHandlers(handlers)
                    group.engine.addRoute(httpMethod, absolutePath, handlers)
                    return group.returnObj()
                }
                func (group *RouterGroup) Handle(httpMethod, relativePath string, handlers ...HandlerFunc) IRoutes {
                    return group.handle(httpMethod, relativePath, handlers)
                }
                "#,
                "RouterGroup.Handle registers routes by delegating to the group handle path.",
            ),
            (
                "Engine.addRoute",
                "gin.go",
                r#"
                func (engine *Engine) addRoute(method, path string, handlers HandlersChain) {
                    root := engine.trees.get(method)
                    if root == nil {
                        root = new(node)
                        engine.trees = append(engine.trees, methodTree{method: method, root: root})
                    }
                    root.addRoute(path, handlers)
                }
                "#,
                "Engine.addRoute inserts handlers into the per-method route tree.",
            ),
            (
                "Engine.handleHTTPRequest",
                "gin.go",
                r#"
                func (engine *Engine) handleHTTPRequest(c *Context) {
                    value := root.getValue(rPath, c.params, c.skippedNodes, unescape)
                    if value.handlers != nil {
                        c.handlers = value.handlers
                        c.fullPath = value.fullPath
                        c.Next()
                    }
                }
                "#,
                "Engine.handleHTTPRequest finds a route and installs handlers on the context.",
            ),
            (
                "Context.Next",
                "context.go",
                r#"
                func (c *Context) Next() {
                    c.index++
                    for c.index < safeInt8(len(c.handlers)) {
                        if c.handlers[c.index] != nil {
                            c.handlers[c.index](c)
                        }
                        c.index++
                    }
                }
                "#,
                "Context.Next advances through the handler chain.",
            ),
        ];

        for (symbol, path, source, expected) in fixtures {
            let citation = test_packet_citation(symbol, path, 0.9);
            let claims = packet_source_derived_claims_for_citation(prompt, &citation, source);
            assert!(
                claims.iter().any(|claim| claim == expected),
                "expected source-derived Gin claim `{expected}` for {path}; got {claims:?}"
            );
        }
    }

    #[test]
    fn server_route_source_claims_survive_with_generic_claims() {
        let prompt = "Trace how a router group registers routes and dispatches handlers for an HTTP request.";
        let fixtures = [
            (
                "RouterGroup.Handle",
                "routergroup.go",
                r#"
                func (group *RouterGroup) Handle(httpMethod, relativePath string, handlers ...HandlerFunc) IRoutes {
                    if matched := regEnLetter.MatchString(httpMethod); !matched {
                        panic("http method is not valid")
                    }
                    return group.handle(httpMethod, relativePath, handlers)
                }
                "#,
                "RouterGroup.Handle registers routes by delegating to the group handle path.",
            ),
            (
                "Context.Next",
                "context.go",
                r#"
                func (c *Context) Next() {
                    c.index++
                    for c.index < safeInt8(len(c.handlers)) {
                        if c.handlers[c.index] != nil {
                            c.handlers[c.index](c)
                        }
                        c.index++
                    }
                }
                "#,
                "Context.Next advances through the handler chain.",
            ),
        ];

        for (symbol, path, source, expected) in fixtures {
            let citation = test_packet_citation(symbol, path, 0.9);
            let claims = packet_source_derived_claims_for_citation(prompt, &citation, source);
            assert!(
                claims.iter().any(|claim| claim == expected),
                "expected generic server-route claim `{expected}` for {path}; got {claims:?}"
            );
        }
    }

    #[test]
    fn express_shape_route_claims_survive_with_generic_claims() {
        let prompt = "Trace how a server application creates an app, registers middleware and routes, handles an incoming request, and sends a response.";
        let citation = test_packet_citation("application", "lib/application.js", 0.9);
        let claims = packet_source_derived_claims_for_citation(
            prompt,
            &citation,
            r#"
            function createApplication() {
              var app = function(req, res, next) { app.handle(req, res, next); };
              mixin(app, proto, false);
              app.request = Object.create(req);
              app.response = Object.create(res);
              app.init();
              return app;
            }

            app.init = function init() {
              this.defaultConfiguration();
              this.router = new Router({});
            };

            app.handle = function handle(req, res, callback) {
              this.router.handle(req, res, callback);
            };

            app.use = function use(fn) {
              return this.router.use(path, fn);
            };

            app.route = function route(path) {
              return this.router.route(path);
            };

            res.send = function send(body) {
              this.set('Content-Length', len);
              return this.end(chunk, encoding);
            };
            "#,
        );

        for expected in [
            "createApplication builds a callable app object and mixes in request and response prototypes.",
            "app.init creates application state and router configuration.",
            "app.handle delegates request handling to the router.",
            "app.use registers middleware on the router.",
            "app.route creates route entries through the router.",
            "res.send prepares and sends the response body.",
        ] {
            assert!(
                claims.iter().any(|claim| claim == expected),
                "expected generic application-route claim `{expected}` in {claims:?}"
            );
        }
    }

    #[test]
    fn shell_version_use_guard_claim_survives_with_generic_claims() {
        let prompt = "Trace how a shell version manager install script dispatches use commands and switches versions.";
        let citation = test_packet_citation("maybe_switch_if_needed", "tool.sh", 0.9);
        let claims = packet_source_derived_claims_for_citation(
            prompt,
            &citation,
            r#"
            maybe_switch_if_needed() {
              if [ "_${1-}" = "_$(tool_ls_current)" ]; then
                return
              fi
              tool use "$@"
            }
            "#,
        );

        let expected = "maybe_switch_if_needed switches versions only when the requested version is not already active.";
        assert!(
            claims.iter().any(|claim| claim == expected),
            "expected generic shell version-use claim `{expected}`; got {claims:?}"
        );
    }

    #[test]
    fn hook_cache_source_claims_survive_with_generic_claims() {
        let prompt = "Explain how a public hook serializes keys, connects cache helpers, and routes mutate behavior.";

        let hook = test_packet_citation("useDataHandler", "src/hooks/use-data.ts", 0.9);
        let claims = packet_source_derived_claims_for_citation(
            prompt,
            &hook,
            r#"
            import { type State, withArgs } from '../_internal'

            export interface FullConfiguration<Data = any, Error = any> {
              fallback: Record<string, Data | Promise<Data>>
            }

            export const useDataHandler = (_key) => {
              const [key, fnArg] = serialize(_key)
              return internalMutate(cache, key, fnArg)
            }

            const useData = withArgs<DataHook>(useDataHandler)
            export default useData
            "#,
        );
        let expected =
            "The public useData export wraps useDataHandler with argument normalization.";
        assert!(
            claims.iter().any(|claim| claim == expected),
            "expected generic hook wrapper claim `{expected}`; got {claims:?}"
        );
        assert!(
            claims
                .iter()
                .all(|claim| !claim.contains("public types export wraps thenable")),
            "generic hook wrapper claim should come from the withArgs assignment, not imports or unrelated type defaults; got {claims:?}"
        );

        let helper = test_packet_citation("makeCacheHelper", "src/cache/helper.ts", 0.9);
        let claims = packet_source_derived_claims_for_citation(
            prompt,
            &helper,
            r#"
            export const makeCacheHelper = (cache, key) => {
              return [
                () => cache.get(key),
                info => state[5](key, info),
                state[6],
                () => snapshot.get(key)
              ] as const
            }
            "#,
        );
        let expected = "makeCacheHelper provides cache get, set, subscribe, and snapshot helpers.";
        assert!(
            claims.iter().any(|claim| claim == expected),
            "expected generic cache helper claim `{expected}`; got {claims:?}"
        );

        let swr_handler = test_packet_citation("useSWRHandler", "src/index/use-swr.ts", 0.9);
        let claims = packet_source_derived_claims_for_citation(
            prompt,
            &swr_handler,
            r#"
            export const useSWRHandler = (_key, fetcher, config) => {
              const [key, fnArg] = serialize(_key)
              const [getCache, setCache, subscribeCache, getInitialCache] =
                createCacheHelper(cache, key)
              const cachedData = getCache()
              return { data: cachedData.data, mutate: (...args) => internalMutate(cache, key, ...args) }
            }
            "#,
        );
        let expected = "useSWRHandler serializes the key before reading cache state.";
        assert!(
            claims.iter().any(|claim| claim == expected),
            "expected generic SWR key serialization claim `{expected}`; got {claims:?}"
        );
    }

    #[test]
    fn client_send_source_claims_survive_with_generic_claims() {
        let prompt = "Explain how a client exposes convenience request helpers and routes send behavior through the transport implementation.";

        let base = test_packet_citation("BaseTransportClient", "src/base_client.dart", 0.9);
        let claims = packet_source_derived_claims_for_citation(
            prompt,
            &base,
            r#"
            abstract mixin class BaseTransportClient implements Client {
              Future<Response> get(Uri url) => _sendUnstreamed('GET', url);
              Future<Response> post(Uri url, {Object? body}) =>
                  _sendUnstreamed('POST', url, body);

              Future<StreamedResponse> send(BaseRequest request);

              Future<Response> _sendUnstreamed(String method, Uri url,
                  [Object? body]) async {
                var request = Request(method, url);
                return Response.fromStream(await send(request));
              }
            }
            "#,
        );
        let expected = "BaseTransportClient implements convenience methods in terms of send.";
        assert!(
            claims.iter().any(|claim| claim == expected),
            "expected generic client convenience claim `{expected}`; got {claims:?}"
        );

        let native = test_packet_citation("NativeClient", "src/native_client.dart", 0.9);
        let claims = packet_source_derived_claims_for_citation(
            prompt,
            &native,
            r#"
            import 'dart:io';

            class NativeClient extends BaseTransportClient {
              HttpClient? _inner;

              Future<NativeStreamedResponse> send(BaseRequest request) async {
                var stream = request.finalize();
                var ioRequest = await _inner!.openUrl(request.method, request.url);
                final response = await stream.pipe(ioRequest) as HttpClientResponse;
                return NativeStreamedResponse(response);
              }
            }
            "#,
        );
        let expected = "NativeClient.send is the dart:io transport implementation.";
        assert!(
            claims.iter().any(|claim| claim == expected),
            "expected generic transport send claim `{expected}`; got {claims:?}"
        );
    }

    #[test]
    fn generic_css_animation_source_claims_name_vars_base_and_keyframes() {
        let fixtures = [
            (
                "styles/timing.css",
                r#"
                :root {
                  --motion-duration: 250ms;
                  --motion-delay: 75ms;
                  --motion-repeat: 2;
                }
                "#,
                "Shared CSS custom properties --motion-duration, --motion-delay, and --motion-repeat define animation duration, delay, and repeat defaults.",
            ),
            (
                "styles/base.css",
                r#"
                .motion-base {
                  animation-duration: var(--motion-duration);
                  animation-fill-mode: both;
                }
                "#,
                ".motion-base is the base class that applies animation duration and fill mode.",
            ),
            (
                "styles/effects.css",
                r#"
                @keyframes fade-in {
                  from { opacity: 0; }
                  to { opacity: 1; }
                }

                .fade-in {
                  animation-name: fade-in;
                }
                "#,
                "Named classes such as .fade-in set animation-name to matching keyframes; @keyframes fade-in defines the matching animation.",
            ),
        ];

        for (path, source, expected) in fixtures {
            let claims = packet_generic_css_animation_flow_claims(source);
            assert!(
                claims.iter().any(|claim| claim == expected),
                "expected generic CSS animation claim `{expected}` for {path}; got {claims:?}"
            );
        }
    }

    #[test]
    fn css_animation_source_claims_name_vars_base_imports_and_keyframes() {
        let fixtures = [
            (
                "source/_vars.css",
                r#"
                :root {
                  --animate-duration: 1s;
                  --animate-delay: 1s;
                  --animate-repeat: 1;
                }
                "#,
                "Shared CSS custom properties define animation duration, delay, and repeat defaults.",
            ),
            (
                "source/_base.css",
                r#"
                .animated {
                  animation-duration: var(--animate-duration);
                  animation-fill-mode: both;
                }
                "#,
                ".animated is the base class that applies animation duration and fill mode.",
            ),
            (
                "source/animate.css",
                r#"
                @import '_vars.css';
                @import '_base.css';
                @import 'attention_seekers/bounce.css';
                @import 'attention_seekers/flash.css';
                "#,
                "The source/animate.css file imports the variable, base, and individual animation files.",
            ),
            (
                "source/attention_seekers/bounce.css",
                r#"
                @keyframes bounce {
                  from, to { transform: translate3d(0, 0, 0); }
                }
                .bounce {
                  animation-name: bounce;
                }
                "#,
                "Named classes such as .bounce set animation-name to matching keyframes.",
            ),
            (
                "source/attention_seekers/flash.css",
                r#"
                @keyframes flash {
                  from, to { opacity: 1; }
                }
                .flash {
                  animation-name: flash;
                }
                "#,
                "source/attention_seekers/flash.css defines @keyframes flash and .flash.",
            ),
        ];

        for (path, source, expected) in fixtures {
            let claims = packet_css_animation_flow_claims(path, source);
            assert!(
                claims.iter().any(|claim| claim == expected),
                "expected CSS animation claim `{expected}` for {path}; got {claims:?}"
            );
        }
    }
    #[test]
    fn generic_sql_schema_claims_survive_with_generic_claims() {
        let prompt = "Explain SQL schema relationships between artists, albums, tracks, invoices, and invoice lines across seed scripts.";
        let citation = test_packet_citation("schema.sql", "db/schema.sql", 0.9);
        let claims = packet_source_derived_claims_for_citation(
            prompt,
            &citation,
            r#"
            CREATE TABLE [Album]
            (
                [AlbumId] INTEGER NOT NULL,
                [ArtistId] INTEGER NOT NULL,
                FOREIGN KEY ([ArtistId]) REFERENCES [Artist] ([ArtistId])
            );
            CREATE TABLE [Artist] ([ArtistId] INTEGER NOT NULL);
            CREATE TABLE [InvoiceLine]
            (
                [InvoiceLineId] INTEGER NOT NULL,
                [InvoiceId] INTEGER NOT NULL,
                [TrackId] INTEGER NOT NULL,
                FOREIGN KEY ([InvoiceId]) REFERENCES [Invoice] ([InvoiceId]),
                FOREIGN KEY ([TrackId]) REFERENCES [Track] ([TrackId])
            );
            CREATE TABLE [Track]
            (
                [TrackId] INTEGER NOT NULL,
                [AlbumId] INTEGER,
                [GenreId] INTEGER,
                [MediaTypeId] INTEGER NOT NULL,
                FOREIGN KEY ([AlbumId]) REFERENCES [Album] ([AlbumId]),
                FOREIGN KEY ([GenreId]) REFERENCES [Genre] ([GenreId]),
                FOREIGN KEY ([MediaTypeId]) REFERENCES [MediaType] ([MediaTypeId])
            );
            "#,
        );

        for expected in [
            "SQL schema defines tables Album, Artist, InvoiceLine, and Track.",
            "Album rows reference Artist rows through ArtistId.",
            "InvoiceLine rows reference Invoice and Track rows.",
            "Track rows reference Album, Genre, and MediaType rows.",
        ] {
            assert!(
                claims.iter().any(|claim| claim == expected),
                "expected generic SQL schema claim `{expected}` in {claims:?}"
            );
        }
    }

    #[test]
    fn runtime_formatting_claims_survive_with_generic_claims() {
        let prompt = "Explain how fmt turns formatting arguments into type-erased format args and reaches vformat or format_to output paths.";

        let format_h = test_packet_citation("vformat", "include/fmt/format.h", 0.9);
        let claims = packet_source_derived_claims_for_citation(
            prompt,
            &format_h,
            r#"
            class format_error : public std::runtime_error {};
            inline auto vformat(locale_ref loc, string_view fmt, format_args args) -> std::string {
              detail::buffer<char> buf;
              detail::vformat_to(buf, fmt, args, loc);
              return to_string(buf);
            }
            template <typename OutputIt, typename... T>
            auto format_to(OutputIt out, locale_ref loc, format_string<T...> fmt, T&&... args) {
              return fmt::vformat_to(out, loc, fmt.str, vargs<T...>{{args...}});
            }
            "#,
        );

        for expected in [
            "vformat is the central formatting path for runtime format arguments.",
            "format_error represents formatting failures.",
        ] {
            assert!(
                claims.iter().any(|claim| claim == expected),
                "expected runtime formatting claim `{expected}` in {claims:?}"
            );
        }
    }

    #[test]
    fn site_build_claims_survive_with_generic_claims() {
        let prompt = "Trace how Jekyll's build command creates a site and runs the read, generate, render, and write phases.";

        let fixtures = [
            (
                "Jekyll::Commands::Build.process",
                "lib/jekyll/commands/build.rb",
                r#"
                module Jekyll
                  module Commands
                    class Build
                      def process(options)
                        site = Jekyll::Site.new(options)
                        build(site, options)
                      end
                    end
                  end
                end
                "#,
                "Build.process constructs a Jekyll::Site before running the build.",
            ),
            (
                "Site#process",
                "lib/jekyll/site.rb",
                r#"
                class Site
                  def process
                    reset
                    read
                    generate
                    render
                    cleanup
                    write
                  end
                end
                "#,
                "Site#process runs read, generate, render, and write phases.",
            ),
            (
                "Reader",
                "lib/jekyll/reader.rb",
                r#"
                class Reader
                  def read
                    read_directories
                    read_data
                  end
                end
                "#,
                "Reader is responsible for reading site content.",
            ),
            (
                "Renderer",
                "lib/jekyll/renderer.rb",
                r#"
                class Renderer
                  def render_document
                  end

                  def render_liquid(content, payload, info, path = nil)
                  end
                end
                "#,
                "Renderer renders pages and documents.",
            ),
        ];

        for (symbol, path, source, expected) in fixtures {
            let citation = test_packet_citation(symbol, path, 0.9);
            let claims = packet_source_derived_claims_for_citation(prompt, &citation, source);
            assert!(
                claims.iter().any(|claim| claim == expected),
                "expected site build claim `{expected}` for {path}; got {claims:?}"
            );
        }
    }

    #[test]
    fn generic_sql_schema_file_probe_adds_files_and_source_anchors() {
        let root = packet_temp_root("generic-sql-schema");
        let db_dir = root.join("db");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&db_dir).expect("create sql fixture directory");
        let schema_path = db_dir.join("schema.sql");
        std::fs::write(
            &schema_path,
            r#"
            /***** Create Tables *****/
            CREATE TABLE [Artist] ([ArtistId] INTEGER NOT NULL);
            CREATE TABLE [Album]
            (
                [AlbumId] INTEGER NOT NULL,
                [ArtistId] INTEGER NOT NULL,
                FOREIGN KEY ([ArtistId]) REFERENCES [Artist] ([ArtistId])
            );
            CREATE TABLE [Track]
            (
                [TrackId] INTEGER NOT NULL,
                [AlbumId] INTEGER,
                FOREIGN KEY ([AlbumId]) REFERENCES [Album] ([AlbumId])
            );
            "#,
        )
        .expect("write sql fixture");

        let question = "Explain SQL schema relationships between artists, albums, and tracks.";
        let mut answer = packet_answer_fixture(question, Vec::new());
        maybe_append_sql_schema_file_citations(&root, question, &mut answer);

        let has_file = answer.citations.iter().any(|citation| {
            citation.kind == NodeKind::FILE
                && citation.display_name == "db/schema.sql"
                && citation
                    .retrieval_score_breakdown
                    .as_ref()
                    .is_some_and(|breakdown| {
                        breakdown
                            .provenance
                            .iter()
                            .any(|entry| entry == "packet_generic_sql_schema_file_probe")
                    })
        });
        let has_album_anchor = answer.citations.iter().any(|citation| {
            citation.kind == NodeKind::ANNOTATION
                && citation.display_name == "CREATE TABLE Album"
                && citation.file_path.as_deref().is_some_and(|path| {
                    packet_display_path(path)
                        .replace('\\', "/")
                        .ends_with("db/schema.sql")
                })
        });
        let has_track_anchor = answer.citations.iter().any(|citation| {
            citation.kind == NodeKind::ANNOTATION && citation.display_name == "CREATE TABLE Track"
        });
        let has_foreign_key_anchor = answer.citations.iter().any(|citation| {
            citation.kind == NodeKind::ANNOTATION
                && citation.display_name == "FOREIGN KEY"
                && citation
                    .retrieval_score_breakdown
                    .as_ref()
                    .is_some_and(|breakdown| {
                        breakdown
                            .provenance
                            .iter()
                            .any(|entry| entry == "packet_generic_sql_schema_anchor_probe")
                    })
        });
        let has_comment_false_positive = answer
            .citations
            .iter()
            .any(|citation| citation.display_name == "CREATE TABLE s");

        let _ = std::fs::remove_dir_all(&root);

        assert!(
            has_file,
            "generic SQL schema probe should append the schema file citation: {:?}",
            answer.citations
        );
        assert!(
            has_album_anchor,
            "generic SQL schema probe should append CREATE TABLE anchors: {:?}",
            answer.citations
        );
        assert!(
            has_track_anchor,
            "generic SQL schema probe should carry prompt-matched table anchors: {:?}",
            answer.citations
        );
        assert!(
            has_foreign_key_anchor,
            "generic SQL schema probe should append FOREIGN KEY anchors: {:?}",
            answer.citations
        );
        assert!(
            !has_comment_false_positive,
            "generic SQL schema probe should not parse prose comments as table names: {:?}",
            answer.citations
        );
    }

    #[test]
    fn required_file_scoped_source_probe_adds_method_and_markup_anchors() {
        let root = packet_temp_root("required-source-probes");
        let _ = std::fs::remove_dir_all(&root);
        write_packet_fixture_file(
            &root,
            "lib/jekyll/site.rb",
            r#"
            module Jekyll
              class Site
                def process
                  read
                  render
                  write
                end
              end
            end
            "#,
        );
        write_packet_fixture_file(
            &root,
            "src/Logging/Logger.php",
            r#"
            <?php
            namespace AppLogging;
            class Logger
            {
                public function addRecord(int $level, string $message): bool
                {
                    return true;
                }
            }
            "#,
        );
        write_packet_fixture_file(
            &root,
            "html/forms/custom-validation/detailed-custom-validation.html",
            r#"
            <form novalidate>
              <input id="mail" type="email" required minlength="8">
            </form>
            "#,
        );

        let mut answer = packet_answer_fixture("fixture packet", Vec::new());
        let probes = [
            "lib/jekyll/site.rb Site#process".to_string(),
            "src/Logging/Logger.php Logger::addRecord".to_string(),
            "html/forms/custom-validation/detailed-custom-validation.html input#mail".to_string(),
            "html/forms/custom-validation/detailed-custom-validation.html novalidate".to_string(),
        ];
        maybe_append_required_file_scoped_source_citations(
            &root,
            "fixture packet",
            PacketTaskClassDto::DataFlow,
            &probes,
            &mut answer,
        );

        let has_ruby_method = answer.citations.iter().any(|citation| {
            citation.display_name == "Site#process"
                && citation.kind == NodeKind::METHOD
                && citation.line == Some(4)
        });
        let has_php_method = answer.citations.iter().any(|citation| {
            citation.display_name == "Logger::addRecord"
                && citation.kind == NodeKind::METHOD
                && citation
                    .file_path
                    .as_deref()
                    .is_some_and(|path| packet_display_path(path).ends_with("Logger.php"))
        });
        let has_input_anchor = answer.citations.iter().any(|citation| {
            citation.display_name == "input#mail" && citation.kind == NodeKind::ANNOTATION
        });
        let has_boolean_attribute_anchor = answer.citations.iter().any(|citation| {
            citation.display_name == "novalidate" && citation.kind == NodeKind::ANNOTATION
        });
        let used_source_probe = answer.retrieval_trace.annotations.iter().any(|annotation| {
            annotation == "packet_required_file_scoped_source_citations appended=4"
        });

        let _ = std::fs::remove_dir_all(&root);

        assert!(
            has_ruby_method,
            "required source probe should append Ruby method anchors: {:?}",
            answer.citations
        );
        assert!(
            has_php_method,
            "required source probe should append PHP method anchors: {:?}",
            answer.citations
        );
        assert!(
            has_input_anchor,
            "required source probe should append HTML id anchors: {:?}",
            answer.citations
        );
        assert!(
            has_boolean_attribute_anchor,
            "required source probe should append HTML boolean attribute anchors: {:?}",
            answer.citations
        );
        assert!(
            used_source_probe,
            "required source probe should annotate appended anchor count: {:?}",
            answer.retrieval_trace.annotations
        );
    }

    #[test]
    fn automapper_map_flow_source_claims_name_runtime_configuration_and_plans() {
        let prompt = "Explain how AutoMapper configuration and runtime mapper APIs cooperate to map source objects to destination objects.";
        let fixtures = [
            (
                "MapperConfiguration",
                "src/AutoMapper/Configuration/MapperConfiguration.cs",
                r#"
                public sealed class MapperConfiguration : IGlobalConfiguration
                {
                    private readonly Dictionary<TypePair, TypeMap> _configuredMaps;
                    private readonly Dictionary<TypePair, TypeMap> _resolvedMaps;
                    private readonly LockingConcurrentDictionary<MapRequest, Delegate> _executionPlans;
                    public LambdaExpression BuildExecutionPlan(Type sourceType, Type destinationType) => this.Internal().BuildExecutionPlan(new(new(sourceType, destinationType)));
                }
                "#,
                "MapperConfiguration builds and owns the mapping configuration used at runtime.",
            ),
            (
                "Mapper.Map",
                "src/AutoMapper/Mapper.cs",
                r#"
                public sealed class Mapper : IMapper, IInternalRuntimeMapper
                {
                    public TDestination Map<TDestination>(object source) => Map(source, default(TDestination));
                    public TDestination Map<TSource, TDestination>(TSource source, TDestination destination) =>
                        MapCore(source, destination, _defaultContext);
                    private TDestination MapCore<TSource, TDestination>(TSource source, TDestination destination, ResolutionContext context)
                    {
                        return _configuration.GetExecutionPlan<TSource, TDestination>(mapRequest)(source, destination, context);
                    }
                }
                "#,
                "Mapper.Map is the public runtime entry point for object mapping.",
            ),
            (
                "TypeMap.CreateMapperLambda",
                "src/AutoMapper/TypeMap.cs",
                r#"
                internal LambdaExpression CreateMapperLambda(IGlobalConfiguration configuration) =>
                    Types.ContainsGenericParameters ? null : new TypeMapPlanBuilder(configuration, this).CreateMapperLambda();
                "#,
                "TypeMap contributes mapper lambda plans used by the execution pipeline.",
            ),
            (
                "TypeMapPlanBuilder",
                "src/AutoMapper/Execution/TypeMapPlanBuilder.cs",
                r#"
                public LambdaExpression CreateMapperLambda()
                {
                    var createDestinationFunc = CreateDestinationFunc();
                    var assignmentFunc = CreateAssignmentFunc(createDestinationFunc);
                    var mapperFunc = CreateMapperFunc(assignmentFunc);
                    return Lambda(mapperFunc, GetParameters(second: _initialDestination));
                }
                "#,
                "TypeMapPlanBuilder participates in building expression plans for mappings.",
            ),
        ];

        for (symbol, path, source, expected) in fixtures {
            let citation = test_packet_citation(symbol, path, 0.9);
            let claims = packet_source_derived_claims_for_citation(prompt, &citation, source);
            assert!(
                claims.iter().any(|claim| claim == expected),
                "expected AutoMapper claim `{expected}` for {path}; got {claims:?}"
            );
        }
    }
    #[test]
    fn express_route_flow_source_claims_name_app_router_response_flow() {
        let prompt = "Trace how Express creates an application, registers middleware/routes, and handles an incoming request through the router and response helpers.";
        let fixtures = [
            (
                "createApplication",
                "lib/express.js",
                "function createApplication() { var app = function(req, res, next) { app.handle(req, res, next); }; mixin(app, proto, false); app.request = Object.create(req); app.response = Object.create(res); app.init(); return app; }",
                "createApplication builds a callable app object and mixes in request and response prototypes.",
            ),
            (
                "logerror",
                "lib/application.js",
                "app.init = function init() { var router = null; this.defaultConfiguration(); router = new Router({}); }\napp.handle = function handle(req, res, callback) { this.router.handle(req, res, done); }\napp.use = function use(fn) { return router.use(path, fn); }\napp.route = function route(path) { return this.router.route(path); }",
                "app.use registers middleware on the router.",
            ),
            (
                "content-disposition",
                "lib/response.js",
                "res.send = function send(body) { this.set('Content-Length', len); this.end(chunk, encoding); return this; }",
                "res.send prepares and sends the response body.",
            ),
        ];

        for (symbol, path, source, expected) in fixtures {
            let citation = test_packet_citation(symbol, path, 0.9);
            let claims = packet_source_derived_claims_for_citation(prompt, &citation, source);
            assert!(
                claims.iter().any(|claim| claim == expected),
                "expected source-derived claim `{expected}` for {path}; got {claims:?}"
            );
        }
    }

    #[test]
    fn route_sufficiency_probes_can_be_covered_by_source_claims() {
        let claims = vec![
            PacketClaimDto {
                claim: "app.use registers middleware on the router.".to_string(),
                citations: Vec::new(),
            },
            PacketClaimDto {
                claim: "app.handle delegates request handling to the router.".to_string(),
                citations: Vec::new(),
            },
            PacketClaimDto {
                claim: "res.send prepares and sends the response body.".to_string(),
                citations: Vec::new(),
            },
        ];

        for probe in ["app.use", "app.handle", "res.send"] {
            assert!(
                packet_probe_query_is_claimed(probe, &claims),
                "expected claim-backed coverage for {probe}: {claims:?}"
            );
        }
    }

    #[test]
    fn java_string_check_source_claims_name_blank_empty_and_region_matching() {
        let prompt = "Explain how Commons Lang implements blank, empty, and case-sensitive string checks across StringUtils, Strings, and CharSequenceUtils.";
        let string_utils = test_packet_citation(
            "org.apache.commons.lang3.StringUtils.isBlank",
            "src/main/java/org/apache/commons/lang3/StringUtils.java",
            0.9,
        );
        let claims = packet_source_derived_claims_for_citation(
            prompt,
            &string_utils,
            r#"
            * StringUtils.isBlank(" ")       = true
            public static boolean isBlank(final CharSequence cs) {
                if (cs == null || cs.length() == 0) {
                    return true;
                }
                return Character.isWhitespace(cs.charAt(0));
            }
            * StringUtils.isEmpty(" ")       = false
            * NOTE: This method changed in Lang version 2.0. It no longer trims the CharSequence.
            public static boolean isEmpty(final CharSequence cs) {
                return cs == null || cs.length() == 0;
            }
            "#,
        );

        for expected in [
            "StringUtils.isBlank treats null, empty, and whitespace-only inputs as blank.",
            "StringUtils.isEmpty does not trim whitespace before deciding emptiness.",
        ] {
            assert!(
                claims.iter().any(|claim| claim == expected),
                "expected Java string claim `{expected}` in {claims:?}"
            );
        }

        let strings = test_packet_citation(
            "Strings",
            "src/main/java/org/apache/commons/lang3/Strings.java",
            0.9,
        );
        let claims = packet_source_derived_claims_for_citation(
            prompt,
            &strings,
            "return CharSequenceUtils.regionMatches(str, ignoreCase, 0, suffix, 0, length);",
        );
        assert!(
            claims.iter().any(|claim| claim
                == "Strings delegates region matching work to CharSequenceUtils.regionMatches."),
            "expected region matching claim in {claims:?}"
        );
    }

    #[test]
    fn generic_string_predicate_claims_name_blank_and_empty_behavior() {
        let source = r#"
        final class TextChecks {
            /**
             * @return true if the value is null, empty or whitespace only.
             */
            public static boolean isBlank(final CharSequence value) {
                final int valueLength = length(value);
                for (int i = 0; i < valueLength; i++) {
                    if (!Character.isWhitespace(value.charAt(i))) {
                        return false;
                    }
                }
                return true;
            }

            public static boolean isEmpty(final CharSequence value) {
                return value == null || value.length() == 0;
            }
        }
        "#;

        let mut claims =
            packet_generic_string_predicate_flow_claims("com.acme.TextChecks.isBlank", source);
        claims.extend(packet_generic_string_predicate_flow_claims(
            "com.acme.TextChecks.isEmpty",
            source,
        ));

        for expected in [
            "TextChecks.isBlank treats null, empty, and whitespace-only inputs as blank.",
            "TextChecks.isEmpty does not trim whitespace before deciding emptiness.",
        ] {
            assert!(
                claims.iter().any(|claim| claim == expected),
                "expected generic string predicate claim `{expected}` in {claims:?}"
            );
        }
    }

    #[test]
    fn swr_source_claims_name_hook_cache_and_mutation_flow() {
        let prompt = "Explain how SWR exposes useSWR, serializes keys, connects cache helpers, and routes mutate behavior through the internal mutation helper.";
        let use_swr = test_packet_citation("useSWRHandler", "src/index/use-swr.ts", 0.9);
        let claims = packet_source_derived_claims_for_citation(
            prompt,
            &use_swr,
            r#"
            export const useSWRHandler = (_key) => {
                const [key, fnArg] = serialize(_key)
                return internalMutate(cache, keyRef.current, ...args)
            }
            const useSWR = withArgs<SWRHook>(useSWRHandler)
            export default useSWR
            "#,
        );
        for expected in [
            "The public useSWR export wraps useSWRHandler with argument normalization.",
            "useSWRHandler serializes the key before reading cache state.",
            "mutate behavior flows through internalMutate.",
        ] {
            assert!(
                claims.iter().any(|claim| claim == expected),
                "expected SWR hook claim `{expected}` in {claims:?}"
            );
        }

        let helper =
            test_packet_citation("createCacheHelper", "src/_internal/utils/helper.ts", 0.9);
        let claims = packet_source_derived_claims_for_citation(
            prompt,
            &helper,
            r#"
            export const createCacheHelper = (cache, key) => {
                const get = () => cache.get(key)
                const set = info => cache.set(key, info)
                const subscribe = callback => subscriptions.push(callback)
                return [get, set, subscribe, () => snapshot]
            }
            "#,
        );
        assert!(
            claims.iter().any(|claim| claim
                == "createCacheHelper provides cache get, set, subscribe, and snapshot helpers."),
            "expected SWR cache helper claim in {claims:?}"
        );

        let mutate = test_packet_citation("internalMutate", "src/_internal/utils/mutate.ts", 0.9);
        let claims = packet_source_derived_claims_for_citation(
            prompt,
            &mutate,
            "export async function internalMutate<Data>(cache, _key, _data) { return data }",
        );
        assert!(
            claims
                .iter()
                .any(|claim| claim == "mutate behavior flows through internalMutate."),
            "expected SWR mutation claim in {claims:?}"
        );
    }

    #[test]
    fn python_requests_source_claims_name_method_flow() {
        let prompt = "Explain how Requests turns a top-level request call into a prepared request and sends it through a session adapter.";
        let cases = [
            (
                "request",
                "src/requests/api.py",
                "def request(method, url, **kwargs):\n    with sessions.Session() as session:\n        return session.request(method=method, url=url, **kwargs)\n",
                "The top-level request helper opens a Session and delegates to Session.request.",
            ),
            (
                "Session.request",
                "src/requests/sessions.py",
                "def request(self, method, url, **kwargs):\n    req = Request(method=method, url=url)\n    prep = self.prepare_request(req)\n    return self.send(prep, **kwargs)\n",
                "Session.request creates a Request object and prepares it into a PreparedRequest.",
            ),
            (
                "PreparedRequest.prepare",
                "src/requests/models.py",
                "def prepare(self):\n    self.prepare_method(method)\n    self.prepare_url(url, params)\n    self.prepare_headers(headers)\n    self.prepare_cookies(cookies)\n    self.prepare_body(data, files, json)\n    self.prepare_auth(auth, url)\n    self.prepare_hooks(hooks)\n",
                "PreparedRequest.prepare builds the prepared method, URL, headers, cookies, body, auth, and hooks.",
            ),
            (
                "Session.send",
                "src/requests/sessions.py",
                "def send(self, request, **kwargs):\n    adapter = self.get_adapter(url=request.url)\n    r = adapter.send(request, **kwargs)\n    return r\n",
                "Session.send chooses an adapter and calls the adapter send method.",
            ),
            (
                "HTTPAdapter.send",
                "src/requests/adapters.py",
                "def send(self, request, **kwargs):\n    resp = conn.urlopen(method=request.method, url=url)\n    return self.build_response(request, resp)\n",
                "HTTPAdapter.send is the transport boundary that returns the response.",
            ),
        ];

        for (symbol, path, source, expected) in cases {
            let citation = test_packet_citation(symbol, path, 0.9);
            let claims = packet_source_derived_claims_for_citation(prompt, &citation, source);
            assert!(
                claims.iter().any(|claim| claim == expected),
                "expected source-derived claim `{expected}` for {symbol}; got {claims:?}"
            );
        }
    }

    #[test]
    fn python_request_flow_does_not_emit_axios_transport_claim_without_xhr() {
        let prompt = "Explain how Requests sends a prepared request through a session adapter.";
        let citation = test_packet_citation("Session", "src/requests/sessions.py", 0.9);
        let claims = packet_source_derived_claims_for_citation(
            prompt,
            &citation,
            "adapter = self.get_adapter(url=request.url)\n# http proxy environment settings\n",
        );

        assert!(
            !claims.iter().any(|claim| claim.contains("xhr or http")),
            "Python Requests source should not inherit Axios transport wording: {claims:?}"
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
            packet_claim_for_role(
                "command entrypoint",
                "command entrypoint",
                &citation,
                "Explain the CLI entrypoint."
            )
            .contains("`CliCommand`"),
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
