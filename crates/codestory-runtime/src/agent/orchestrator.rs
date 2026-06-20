use crate::agent::citation::{evidence_edge_ids_for_node, to_citation_from_hit};
use crate::agent::packet_batch::{
    PacketLatencyBudget, packet_anchor_probe_queries, run_packet_anchor_expansion,
    run_packet_planned_subqueries,
};
#[cfg(test)]
use crate::agent::packet_batch::{
    packet_anchor_hit_is_relevant, packet_anchor_probe_limit_for_budget,
};
#[cfg(test)]
use crate::agent::packet_budget::{
    apply_packet_budget, next_deeper_packet_command, packet_budget_usage,
    truncate_answer_markdown_to_byte_cap,
};
use crate::agent::packet_budget::{
    apply_packet_budget_with_extra, enforce_packet_output_budget, packet_budget_limits,
};
#[cfg(test)]
use crate::agent::packet_capping::{
    cap_citations, cap_packet_citations, promote_focus_neighborhood_citations,
    promote_required_probe_citations,
};
#[cfg(test)]
use crate::agent::packet_claim_profiles::{
    packet_generic_css_animation_flow_claims, packet_generic_string_predicate_flow_claims,
    packet_source_derived_claims_for_citation,
};
#[cfg(test)]
use crate::agent::packet_claims::packet_claim_for_role as build_packet_claim_for_role;
use crate::agent::packet_claims::{packet_flow_claims_markdown, packet_supported_claims};
use crate::agent::packet_evidence::decorate_citation_from_hit;
use crate::agent::packet_evidence_roles::{
    PacketEvidenceRole, packet_claim_key_for_citation, packet_evidence_role,
};
#[cfg(test)]
use crate::agent::packet_plan::{
    build_packet_plan, packet_concept_queries, packet_symbol_probe_queries,
};
use crate::agent::packet_plan::{
    build_packet_plan_with_extra, packet_plan_annotation, packet_rank_terms,
    packet_request_extra_probes,
};
#[cfg(test)]
use crate::agent::packet_required_probes::packet_sufficiency_required_probe_queries;
use crate::agent::packet_required_probes::{
    PacketFileScopedSymbolProbe, packet_file_scoped_symbol_probe_parts,
    packet_probe_file_name_matches, packet_probe_query_is_cited,
    packet_sufficiency_required_probe_queries_with_extra,
};
#[cfg(test)]
use crate::agent::packet_scoring::packet_citation_key;
use crate::agent::packet_scoring::{
    normalize_identifier, packet_citation_rank, packet_display_path,
};
use crate::agent::packet_source_patterns::packet_sql_identifier_after;
use crate::agent::packet_sufficiency::build_packet_sufficiency_with_extra;
#[cfg(test)]
use crate::agent::packet_sufficiency::{
    PACKET_MARKDOWN_TRUNCATION_SUFFIX, quote_packet_command_value,
};
#[cfg(test)]
use crate::agent::packet_sufficiency::{
    build_packet_sufficiency, packet_budget_exceeded_hard_output_cap,
    packet_claim_can_satisfy_sufficiency, packet_claim_family, packet_supported_claim_family_count,
    packet_targeted_follow_up_queries,
};
use crate::agent::packet_terms::{
    packet_probe_terms, packet_terms_have_any, packet_terms_indicate_buffered_io_flow,
    packet_terms_indicate_client_send_flow, packet_terms_indicate_event_loop_command_flow,
    packet_terms_indicate_form_validation_flow, packet_terms_indicate_hook_cache_flow,
    packet_terms_indicate_mapper_configuration_plan_flow,
    packet_terms_indicate_runtime_formatting_flow,
    packet_terms_indicate_server_route_dispatch_flow, packet_terms_indicate_sql_schema_flow,
    packet_terms_indicate_stylesheet_animation_flow,
    packet_terms_indicate_url_session_request_flow, prompt_search_terms,
};
use crate::agent::profiles::{ResolvedProfile, TrailPlan, resolve_profile};
use crate::agent::retrieval_primary::{
    RETRIEVAL_VERSION_SIDECAR, SidecarPrimarySearchOutcome, maybe_run_retrieval_shadow,
    sidecar_retrieval_blocks_nucleo_supplement, sidecar_retrieval_primary_enabled,
    sidecar_retrieval_unavailable_error, try_sidecar_primary_search,
};
use crate::agent::trace::{TraceRecorder, field};
use crate::agent::trace_export;
use crate::{
    AppController, FocusedSourceContext, HybridSearchScoredHit, clamp_u128_to_u32,
    fallback_mermaid as diagnostic_mermaid, hybrid_retrieval_enabled, mermaid_flowchart,
    mermaid_gantt, mermaid_sequence, query_mentions_non_primary_source,
};
use codestory_contracts::api::{
    AgentAnswerDto, AgentAskRequest, AgentCitationDto, AgentCustomRetrievalConfigDto,
    AgentHybridWeightsDto, AgentPacketDto, AgentPacketRequestDto, AgentResponseBlockDto,
    AgentResponseModeDto, AgentResponseSectionDto, AgentRetrievalPolicyModeDto,
    AgentRetrievalPresetDto, AgentRetrievalProfileSelectionDto, AgentRetrievalStepKindDto,
    ApiError, GraphArtifactDto, GraphRequest, GraphResponse, GroundingBudgetDto, IndexFreshnessDto,
    IndexFreshnessStatusDto, NodeDetailsDto, NodeDetailsRequest, NodeId, NodeKind,
    NodeOccurrencesRequest, PacketBudgetLimitsDto, PacketBudgetModeDto, PacketPlanDto,
    PacketTaskClassDto, RetrievalScoreBreakdownDto, SearchHit, SearchHitOrigin, SearchRepoTextMode,
    SearchRequest, TrailConfigDto, TrailFilterOptionsDto,
};
#[cfg(test)]
use codestory_contracts::api::{
    AgentRetrievalStepDto, AgentRetrievalStepStatusDto, EdgeId, PacketBudgetDto,
    PacketBudgetUsageDto, PacketClaimDto, PacketPlanQueryDto, PacketSidecarQueryDiagnosticDto,
    PacketSufficiencyDto, PacketSufficiencyStatusDto, SearchMatchQualityDto,
};
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet, VecDeque};
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
const GRAPH_ARTIFACT_BUNDLE_BYTE_CAP: usize = 512 * 1024;
const RETRIEVAL_VERSION_HYBRID: &str = "hybrid-v1";
const RETRIEVAL_VERSION_SIDECAR_BLOCKED: &str = "sidecar-blocked-v1";
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
    let phase_started = Instant::now();
    maybe_append_sql_schema_file_citations(&project_root, &question, &mut answer);
    maybe_append_generic_source_shape_citations(&project_root, &question, &mut answer);
    let file_scoped_source_probes =
        packet_file_scoped_source_probe_inputs_from_plan(&plan, &extra_probes);
    maybe_append_required_file_scoped_source_citations(
        &project_root,
        &question,
        plan.task_class,
        &file_scoped_source_probes,
        &mut answer,
    );
    append_packet_non_trace_phase(&mut answer, "pre_rank_citations", phase_started);
    let phase_started = Instant::now();
    packet_latency.apply_to_trace(&mut answer);
    append_packet_non_trace_phase(&mut answer, "trace_apply", phase_started);

    let phase_started = Instant::now();
    rank_packet_evidence(&question, &mut answer);
    maybe_annotate_packet_candidate_window(&question, &limits, &mut answer);
    append_packet_non_trace_phase(&mut answer, "rank_and_window", phase_started);

    let phase_started = Instant::now();
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
    append_packet_step_trace_annotation(&mut answer);
    append_packet_non_trace_phase(&mut answer, "shadow_and_trace", phase_started);

    let sufficiency_extra_probes = packet_plan_sufficiency_extra_probes(&plan, &extra_probes);
    let phase_started = Instant::now();
    let budget = apply_packet_budget_with_extra(
        &project_root,
        &question,
        plan.task_class,
        req.budget,
        limits.clone(),
        &mut answer,
        &sufficiency_extra_probes,
    );
    append_packet_non_trace_phase(&mut answer, "budget", phase_started);
    let phase_started = Instant::now();
    append_packet_evidence_sections(&mut answer, plan.task_class, &limits);
    append_packet_non_trace_phase(&mut answer, "evidence_sections", phase_started);
    let phase_started = Instant::now();
    let sufficiency = build_packet_sufficiency_with_extra(
        &project_root,
        &question,
        plan.task_class,
        &answer,
        &budget,
        &sufficiency_extra_probes,
    );
    append_packet_non_trace_phase(&mut answer, "sufficiency", phase_started);
    let phase_started = Instant::now();
    let retrieval_trace_summary = trace_export::packet_retrieval_trace_summary(&answer);
    append_packet_non_trace_phase(&mut answer, "trace_summary", phase_started);

    let phase_started = Instant::now();
    let mut packet = AgentPacketDto {
        packet_id: answer.answer_id.clone(),
        question,
        task_class: Some(plan.task_class),
        plan,
        answer,
        budget,
        sufficiency,
        retrieval_trace_summary,
    };
    append_packet_non_trace_phase(&mut packet.answer, "packet_dto", phase_started);
    let phase_started = Instant::now();
    enforce_packet_output_budget(&project_root, &mut packet);
    append_packet_non_trace_phase(&mut packet.answer, "output_budget", phase_started);
    enforce_packet_output_budget(&project_root, &mut packet);

    if let Some(diagnostic) = trace_export::write_packet_step_trace_from_env(&packet.answer) {
        packet.answer.retrieval_trace.annotations.push(diagnostic);
        let phase_started = Instant::now();
        enforce_packet_output_budget(&project_root, &mut packet);
        append_packet_non_trace_phase(
            &mut packet.answer,
            "trace_artifact_output_budget",
            phase_started,
        );
        enforce_packet_output_budget(&project_root, &mut packet);
    }

    Ok(packet)
}

fn append_packet_non_trace_phase(answer: &mut AgentAnswerDto, label: &str, started_at: Instant) {
    answer
        .retrieval_trace
        .annotations
        .push(packet_non_trace_phase_annotation(
            label,
            clamp_u128_to_u32(started_at.elapsed().as_millis()),
        ));
}

fn packet_non_trace_phase_annotation(label: &str, duration_ms: u32) -> String {
    format!("packet_non_trace_phase label={label} duration_ms={duration_ms}")
}

fn packet_plan_sufficiency_extra_probes(
    plan: &PacketPlanDto,
    explicit_extra_probes: &[String],
) -> Vec<String> {
    let mut probes = Vec::new();
    for probe in explicit_extra_probes {
        push_packet_sufficiency_extra_probe(&mut probes, probe);
    }
    for query in &plan.queries {
        if packet_plan_query_can_gate_sufficiency(&query.query)
            || packet_file_scoped_symbol_probe_parts(&query.query).is_some()
        {
            push_packet_sufficiency_extra_probe(&mut probes, &query.query);
        }
    }
    probes
}

fn packet_plan_query_can_gate_sufficiency(query: &str) -> bool {
    matches!(
        normalize_identifier(query).as_str(),
        "serialize"
            | "cachehelper"
            | "middleware"
            | "appinitialization"
            | "middlewareregistration"
            | "routeregistration"
            | "handlerprocessing"
            | "handlerdispatch"
            | "requesthandler"
            | "responsesend"
            | "routetreeaddroute"
            | "routergrouphandleroute"
            | "enginerequesthandler"
            | "contextnexthandlerchain"
            | "enginecreationrouterstate"
            | "formvalidation"
            | "constraintvalidation"
            | "htmlconstraint"
            | "pattern"
            | "javascriptvalidation"
            | "customvalidation"
            | "customvalidationflow"
            | "formvalidationbypass"
            | "validitystate"
            | "mappingconfiguration"
            | "typemap"
            | "mappingplan"
            | "bufferedsource"
            | "bufferedsink"
            | "sourcebuffer"
            | "sinkbuffer"
            | "sourcereadbuffer"
            | "sinkwritebuffer"
            | "clientsend"
            | "requestresponse"
    )
}

fn push_packet_sufficiency_extra_probe(probes: &mut Vec<String>, probe: &str) {
    let probe = probe.trim();
    if probe.len() < 3 {
        return;
    }
    if !probes
        .iter()
        .any(|existing| existing.eq_ignore_ascii_case(probe))
    {
        probes.push(probe.to_string());
    }
}

fn append_packet_step_trace_annotation(answer: &mut AgentAnswerDto) {
    answer.retrieval_trace.annotations.push(format!(
        "packet_step_trace search_total_ms={} step_count={}",
        trace_export::search_step_total_ms(answer),
        answer.retrieval_trace.steps.len()
    ));
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

fn rank_packet_evidence(question: &str, answer: &mut AgentAnswerDto) {
    let terms = packet_rank_terms(question);
    let prefer_primary_sources = !query_mentions_non_primary_source(question);
    answer.citations.sort_by(|left, right| {
        packet_citation_rank(right, &terms, prefer_primary_sources)
            .partial_cmp(&packet_citation_rank(left, &terms, prefer_primary_sources))
            .unwrap_or(Ordering::Equal)
    });
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
    let role_label = role.map(PacketEvidenceRole::as_str).unwrap_or("-");
    format!(
        "#{}{} rank={:.3} score={:.3} claim={} role={} kind={:?} name=`{}` path={} line={}",
        index + 1,
        if matches_filter { "*" } else { "" },
        packet_citation_rank(citation, rank_terms, prefer_primary_sources),
        citation.score,
        claim,
        role_label,
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
    let role = packet_evidence_role(citation)
        .map(PacketEvidenceRole::as_str)
        .unwrap_or("source evidence");
    format!(
        "- `{}` ({:?}) - `{}`{} - {} - score {:.3}",
        citation.display_name, citation.kind, path, line, role, citation.score
    )
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
    const SYNTHETIC_SQL_SCORE_CAP: f32 = 20.0;
    for candidate in candidates.into_iter().take(12) {
        let path_string = candidate.path.to_string_lossy().to_string();
        let file_already_present = answer.citations.iter().any(|existing| {
            existing.file_path.as_deref().is_some_and(|existing_path| {
                packet_display_path(existing_path) == packet_display_path(&path_string)
            })
        });
        if !file_already_present {
            let raw_score = candidate.score + 5.0;
            let score = raw_score.min(SYNTHETIC_SQL_SCORE_CAP);
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
                    tier_cap: Some(SYNTHETIC_SQL_SCORE_CAP),
                    boosts: Vec::new(),
                    dampening: vec![format!(
                        "synthetic SQL source scan capped from {raw_score:.3}"
                    )],
                    final_rank_reason: Some("synthetic SQL source scan".to_string()),
                    provenance: vec!["packet_generic_sql_schema_file_probe".to_string()],
                }),
                evidence_tier: Some(
                    codestory_contracts::api::PacketEvidenceTierDto::SyntheticSourceScan,
                ),
                evidence_producer: Some("packet_generic_sql_schema_file_probe".to_string()),
                resolution_status: Some(
                    codestory_contracts::api::PacketEvidenceResolutionDto::SourceRangeOnly,
                ),
                loss_reason: None,
                coverage_role: Some("sql schema scripts".to_string()),
                eligible_for_sufficiency: Some(false),
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
            let raw_score = candidate.score + anchor.score;
            let score = raw_score.min(SYNTHETIC_SQL_SCORE_CAP);
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
                    tier_cap: Some(SYNTHETIC_SQL_SCORE_CAP),
                    boosts: Vec::new(),
                    dampening: vec![format!(
                        "synthetic SQL source scan capped from {raw_score:.3}"
                    )],
                    final_rank_reason: Some("synthetic SQL source scan".to_string()),
                    provenance: vec!["packet_generic_sql_schema_anchor_probe".to_string()],
                }),
                evidence_tier: Some(
                    codestory_contracts::api::PacketEvidenceTierDto::SyntheticSourceScan,
                ),
                evidence_producer: Some("packet_generic_sql_schema_anchor_probe".to_string()),
                resolution_status: Some(
                    codestory_contracts::api::PacketEvidenceResolutionDto::SourceRangeOnly,
                ),
                loss_reason: None,
                coverage_role: Some("sql schema anchor".to_string()),
                eligible_for_sufficiency: Some(false),
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

struct PacketGenericSourceShapeCandidate {
    path: std::path::PathBuf,
    display_name: String,
    kind: NodeKind,
    line: u32,
    score: f32,
    coverage_role: String,
    producer: String,
    eligible_for_sufficiency: bool,
}

fn maybe_append_generic_source_shape_citations(
    project_root: &Path,
    question: &str,
    answer: &mut AgentAnswerDto,
) {
    let terms = packet_probe_terms(question);
    let route_flow = packet_terms_indicate_server_route_dispatch_flow(&terms);
    let mapper_flow = packet_terms_indicate_mapper_configuration_plan_flow(&terms);
    let client_send_flow = packet_terms_indicate_client_send_flow(&terms);
    let buffered_io_flow = packet_terms_indicate_buffered_io_flow(&terms);
    let url_session_request_flow = packet_terms_indicate_url_session_request_flow(&terms);
    let hook_cache_flow = packet_terms_indicate_hook_cache_flow(&terms);
    let command_flow = packet_terms_indicate_event_loop_command_flow(&terms);
    let form_validation_flow = packet_terms_indicate_form_validation_flow(&terms);
    let formatting_flow = packet_terms_indicate_runtime_formatting_flow(&terms);
    let css_animation_flow = packet_terms_indicate_stylesheet_animation_flow(&terms)
        || (packet_terms_have_any(&terms, &["animation", "animations", "animate"])
            && packet_terms_have_any(&terms, &["variable", "variables", "keyframe", "keyframes"]));
    if !route_flow
        && !mapper_flow
        && !client_send_flow
        && !buffered_io_flow
        && !url_session_request_flow
        && !hook_cache_flow
        && !command_flow
        && !form_validation_flow
        && !formatting_flow
        && !css_animation_flow
    {
        return;
    }

    let mut candidates = Vec::new();
    collect_generic_source_shape_candidates(
        project_root,
        project_root,
        route_flow,
        mapper_flow,
        client_send_flow,
        buffered_io_flow,
        url_session_request_flow,
        hook_cache_flow,
        command_flow,
        form_validation_flow,
        formatting_flow,
        css_animation_flow,
        &mut candidates,
    );
    if url_session_request_flow {
        collect_cited_request_validation_shape_candidates(answer, &mut candidates);
    }
    candidates.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(Ordering::Equal)
            .then_with(|| left.display_name.cmp(&right.display_name))
    });

    let mut appended = 0usize;
    let mut skipped_existing = 0usize;
    for candidate in candidates.into_iter().take(24) {
        if appended >= 16 {
            break;
        }
        let path_string = candidate.path.to_string_lossy().to_string();
        let score = candidate.score.min(40.0);
        if let Some(existing) = answer.citations.iter_mut().find(|existing| {
            existing.display_name == candidate.display_name
                && existing.file_path.as_deref().is_some_and(|existing_path| {
                    packet_display_path(existing_path) == packet_display_path(&path_string)
                })
        }) {
            skipped_existing = skipped_existing.saturating_add(1);
            if existing.score < score {
                existing.score = score;
            }
            if existing.coverage_role.is_none() {
                existing.coverage_role = Some(candidate.coverage_role);
            }
            if let Some(breakdown) = existing.retrieval_score_breakdown.as_mut()
                && !breakdown
                    .boosts
                    .iter()
                    .any(|boost| boost == "generic source-shape duplicate boost")
            {
                breakdown
                    .boosts
                    .push("generic source-shape duplicate boost".to_string());
            }
            continue;
        }
        answer.citations.push(AgentCitationDto {
            node_id: NodeId(format!(
                "packet::generic_source_shape::{}::{}::{}",
                candidate.producer, candidate.display_name, candidate.line
            )),
            display_name: candidate.display_name,
            kind: candidate.kind,
            file_path: Some(path_string),
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
                tier_cap: Some(40.0),
                boosts: Vec::new(),
                dampening: Vec::new(),
                final_rank_reason: Some("generic source-shape scan".to_string()),
                provenance: vec![candidate.producer.clone()],
            }),
            evidence_tier: Some(
                codestory_contracts::api::PacketEvidenceTierDto::SyntheticSourceScan,
            ),
            evidence_producer: Some(candidate.producer),
            resolution_status: Some(
                codestory_contracts::api::PacketEvidenceResolutionDto::SourceRangeOnly,
            ),
            loss_reason: None,
            coverage_role: Some(candidate.coverage_role),
            eligible_for_sufficiency: Some(candidate.eligible_for_sufficiency),
        });
        appended = appended.saturating_add(1);
    }

    if appended > 0 || skipped_existing > 0 {
        answer.retrieval_trace.annotations.push(format!(
            "packet_generic_source_shape_citations appended={appended} skipped_existing={skipped_existing}"
        ));
    }
}

fn collect_generic_source_shape_candidates(
    project_root: &Path,
    dir: &Path,
    route_flow: bool,
    mapper_flow: bool,
    client_send_flow: bool,
    buffered_io_flow: bool,
    url_session_request_flow: bool,
    hook_cache_flow: bool,
    command_flow: bool,
    form_validation_flow: bool,
    formatting_flow: bool,
    css_animation_flow: bool,
    candidates: &mut Vec<PacketGenericSourceShapeCandidate>,
) {
    if candidates.len() >= 96 {
        return;
    }
    let Ok(read_dir) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries = read_dir.flatten().collect::<Vec<_>>();
    entries.sort_by(|left, right| {
        let left_is_dir = left.path().is_dir();
        let right_is_dir = right.path().is_dir();
        left_is_dir
            .cmp(&right_is_dir)
            .then_with(|| left.file_name().cmp(&right.file_name()))
    });
    for entry in entries {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if path.is_dir() {
            if !packet_source_probe_skip_dir(&name) {
                collect_generic_source_shape_candidates(
                    project_root,
                    &path,
                    route_flow,
                    mapper_flow,
                    client_send_flow,
                    buffered_io_flow,
                    url_session_request_flow,
                    hook_cache_flow,
                    command_flow,
                    form_validation_flow,
                    formatting_flow,
                    css_animation_flow,
                    candidates,
                );
            }
            continue;
        }
        if !packet_generic_source_shape_candidate_path(project_root, &path) {
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
        if route_flow {
            collect_route_receiver_assignment_candidates(&path, &source, candidates);
        }
        if mapper_flow {
            collect_csharp_mapper_shape_candidates(&path, &source, candidates);
        }
        if client_send_flow {
            collect_client_send_shape_candidates(&path, &source, candidates);
        }
        if buffered_io_flow {
            collect_buffered_io_shape_candidates(&path, &source, candidates);
        }
        if url_session_request_flow {
            collect_url_session_request_shape_candidates(&path, &source, candidates);
        }
        if hook_cache_flow {
            collect_hook_cache_shape_candidates(&path, &source, candidates);
        }
        if command_flow {
            collect_event_loop_command_shape_candidates(&path, &source, candidates);
        }
        if form_validation_flow {
            collect_form_validation_shape_candidates(&path, &source, candidates);
        }
        if formatting_flow {
            collect_runtime_formatting_shape_candidates(&path, &source, candidates);
        }
        if css_animation_flow {
            collect_css_animation_variable_candidates(&path, &source, candidates);
        }
    }
}

fn packet_generic_source_shape_candidate_path(project_root: &Path, path: &Path) -> bool {
    let relative = path
        .strip_prefix(project_root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase();
    if relative.contains("/test/")
        || relative.contains("/tests/")
        || relative.starts_with("test/")
        || relative.starts_with("tests/")
        || relative.contains("/example")
        || relative.starts_with("example")
        || relative.contains("/docs/")
        || relative.starts_with("docs/")
        || relative.contains("docssource/")
        || relative.contains("/vendor/")
        || relative.starts_with("vendor/")
        || relative.contains("/third_party/")
        || relative.starts_with("third_party/")
        || relative.contains("/deps/")
        || relative.starts_with("deps/")
    {
        return false;
    }
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "js" | "mjs"
                    | "cjs"
                    | "ts"
                    | "html"
                    | "htm"
                    | "css"
                    | "c"
                    | "h"
                    | "hpp"
                    | "hh"
                    | "cc"
                    | "cpp"
                    | "cxx"
                    | "cs"
                    | "dart"
                    | "go"
                    | "java"
                    | "kt"
                    | "rs"
                    | "swift"
            )
        })
        .unwrap_or(false)
}

fn collect_route_receiver_assignment_candidates(
    path: &Path,
    source: &str,
    candidates: &mut Vec<PacketGenericSourceShapeCandidate>,
) {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase())
        .unwrap_or_default();
    if !matches!(extension.as_str(), "js" | "mjs" | "cjs" | "ts") {
        return;
    }
    let source_lower = source.to_ascii_lowercase();
    for (index, line) in source.lines().enumerate() {
        let Some((receiver, method)) = packet_js_receiver_function_assignment(line) else {
            continue;
        };
        let normalized_method = normalize_identifier(&method);
        let route_method = matches!(
            normalized_method.as_str(),
            "init" | "handle" | "use" | "route" | "send" | "json" | "end" | "respond"
        );
        if !route_method {
            continue;
        }
        let method_context = match normalized_method.as_str() {
            "init" => source_lower.contains("configuration") || source_lower.contains("router"),
            "handle" | "use" | "route" => source_lower.contains("router"),
            "send" | "json" | "end" | "respond" => {
                source_lower.contains("content-type")
                    || source_lower.contains("content-length")
                    || source_lower.contains(".end(")
                    || source_lower.contains(".write(")
            }
            _ => false,
        };
        if !method_context {
            continue;
        }
        let mut score = 90.0;
        if matches!(
            normalized_method.as_str(),
            "handle" | "use" | "route" | "send"
        ) {
            score += 4.0;
        }
        if normalized_method == "init" {
            score += 2.0;
        }
        candidates.push(PacketGenericSourceShapeCandidate {
            path: path.to_path_buf(),
            display_name: format!("{receiver}.{method}"),
            kind: NodeKind::METHOD,
            line: index.saturating_add(1).try_into().unwrap_or(u32::MAX),
            score,
            coverage_role: "receiver method assignment".to_string(),
            producer: "packet_generic_receiver_method_source_probe".to_string(),
            eligible_for_sufficiency: true,
        });
    }
}

fn packet_js_receiver_function_assignment(line: &str) -> Option<(String, String)> {
    let compact = line.trim();
    let (left, right) = compact.split_once('=')?;
    let right = right.trim_start();
    if !right.starts_with("function") {
        return None;
    }
    let (receiver, method) = left.trim().rsplit_once('.')?;
    let receiver = receiver
        .rsplit(|ch: char| !packet_source_identifier_char(ch))
        .next()
        .unwrap_or(receiver)
        .trim();
    let method = method.trim();
    if receiver.is_empty() || method.is_empty() {
        return None;
    }
    if !receiver.chars().all(packet_source_identifier_char)
        || !method.chars().all(packet_source_identifier_char)
    {
        return None;
    }
    Some((receiver.to_string(), method.to_string()))
}

fn collect_client_send_shape_candidates(
    path: &Path,
    source: &str,
    candidates: &mut Vec<PacketGenericSourceShapeCandidate>,
) {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase())
        .unwrap_or_default();
    if extension != "dart" {
        return;
    }

    let normalized_source = normalize_identifier(source);
    let source_lower = source.to_ascii_lowercase();
    if source_lower.contains("_withclient")
        && source_lower.contains("client()")
        && source_lower.contains("client.")
        && packet_source_shape_has_any(source, &["get(", "post(", "put(", "patch(", "delete("])
    {
        candidates.push(PacketGenericSourceShapeCandidate {
            path: path.to_path_buf(),
            display_name: "Top-level HTTP helpers".to_string(),
            kind: NodeKind::FUNCTION,
            line: packet_first_line_containing(source, &["Future<Response>", " get("]).unwrap_or(1),
            score: 116.0,
            coverage_role: "client public facade".to_string(),
            producer: "packet_generic_client_send_source_probe".to_string(),
            eligible_for_sufficiency: true,
        });
    }

    if normalized_source.contains("interfaceclassclient")
        && normalized_source.contains("futureresponse")
        && normalized_source.contains("futurestreamedresponsesend")
        && normalized_source.contains("request")
    {
        candidates.push(PacketGenericSourceShapeCandidate {
            path: path.to_path_buf(),
            display_name: "Client interface helpers".to_string(),
            kind: NodeKind::METHOD,
            line: packet_first_line_containing(source, &["Future<Response>", " get("]).unwrap_or(1),
            score: 114.0,
            coverage_role: "client interface helpers".to_string(),
            producer: "packet_generic_client_send_source_probe".to_string(),
            eligible_for_sufficiency: true,
        });
    }

    if normalized_source.contains("classrequestextends")
        && normalized_source.contains("bytestreamfinalize")
        && normalized_source.contains("frombytesbodybytes")
    {
        candidates.push(PacketGenericSourceShapeCandidate {
            path: path.to_path_buf(),
            display_name: "Request.finalize".to_string(),
            kind: NodeKind::METHOD,
            line: packet_first_line_containing(source, &["ByteStream", " finalize("]).unwrap_or(1),
            score: 112.0,
            coverage_role: "client request finalization".to_string(),
            producer: "packet_generic_client_send_source_probe".to_string(),
            eligible_for_sufficiency: true,
        });
    }

    if normalized_source.contains("classresponseextendsbaseresponse")
        && normalized_source.contains("fromstreamstreamedresponseresponse")
        && normalized_source.contains("responsestreamtobytes")
    {
        candidates.push(PacketGenericSourceShapeCandidate {
            path: path.to_path_buf(),
            display_name: "Response.fromStream".to_string(),
            kind: NodeKind::METHOD,
            line: packet_first_line_containing(source, &["fromStream", "StreamedResponse"])
                .unwrap_or(1),
            score: 112.0,
            coverage_role: "client response materialization".to_string(),
            producer: "packet_generic_client_send_source_probe".to_string(),
            eligible_for_sufficiency: true,
        });
    }

    if source_lower.contains("dart:io")
        && source_lower.contains("httpclient")
        && source_lower.contains("future<streamedresponse>")
        && source_lower.contains(" send(")
        && source_lower.contains("request.finalize")
        && normalized_source.contains("openurl")
    {
        candidates.push(PacketGenericSourceShapeCandidate {
            path: path.to_path_buf(),
            display_name: "Transport send".to_string(),
            kind: NodeKind::METHOD,
            line: packet_first_line_containing(source, &["Future<StreamedResponse>", " send("])
                .unwrap_or(1),
            score: 112.0,
            coverage_role: "client transport send".to_string(),
            producer: "packet_generic_client_send_source_probe".to_string(),
            eligible_for_sufficiency: true,
        });
    }
}

fn collect_hook_cache_shape_candidates(
    path: &Path,
    source: &str,
    candidates: &mut Vec<PacketGenericSourceShapeCandidate>,
) {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase())
        .unwrap_or_default();
    if !matches!(extension.as_str(), "js" | "mjs" | "cjs" | "ts") {
        return;
    }

    let source_lower = source.to_ascii_lowercase();
    let normalized_source = normalize_identifier(source);
    let cache_helper_call = packet_source_shape_has_cache_helper_call(&normalized_source);
    let hook_return_call = packet_source_shape_has_hook_return_call(&normalized_source);

    if source_lower.contains("serialize(_key)")
        && cache_helper_call
        && normalized_source.contains("mutate")
        && let Some((display_name, line)) = packet_first_exported_name_near(source, &["handler"])
    {
        candidates.push(PacketGenericSourceShapeCandidate {
            path: path.to_path_buf(),
            display_name,
            kind: NodeKind::FUNCTION,
            line,
            score: 118.0,
            coverage_role: "hook_key_serialization".to_string(),
            producer: "packet_generic_hook_cache_source_probe".to_string(),
            eligible_for_sufficiency: true,
        });
    }

    if normalized_source.contains("stablehash")
        && normalized_source.contains("returnkeyargs")
        && let Some((display_name, line)) =
            packet_first_exported_name_near(source, &["serialize", "key"])
    {
        candidates.push(PacketGenericSourceShapeCandidate {
            path: path.to_path_buf(),
            display_name,
            kind: NodeKind::FUNCTION,
            line,
            score: 116.0,
            coverage_role: "hook_key_serialization".to_string(),
            producer: "packet_generic_hook_cache_source_probe".to_string(),
            eligible_for_sufficiency: true,
        });
    }

    if source_lower.contains("cache.get(key)")
        && source_lower.contains("return [")
        && (source_lower.contains("cache.set(key")
            || source_lower.contains("state[5]")
            || source_lower.contains("setter"))
        && (source_lower.contains("state[6]")
            || source_lower.contains("subscribe")
            || source_lower.contains("subscriber"))
        && (source_lower.contains("snapshot")
            || source_lower.contains("initial_cache")
            || source_lower.contains("initial cache"))
        && let Some((display_name, line)) =
            packet_first_exported_name_near(source, &["cache", "helper"])
    {
        candidates.push(PacketGenericSourceShapeCandidate {
            path: path.to_path_buf(),
            display_name,
            kind: NodeKind::FUNCTION,
            line,
            score: 116.0,
            coverage_role: "hook_cache_helper".to_string(),
            producer: "packet_generic_hook_cache_source_probe".to_string(),
            eligible_for_sufficiency: true,
        });
    }

    if normalized_source.contains("exportasyncfunction")
        && normalized_source.contains("serialize")
        && cache_helper_call
        && normalized_source.contains("mutatebykey")
        && let Some((display_name, line)) =
            packet_first_exported_name_near(source, &["mutate", "mutation", "mutat"])
    {
        candidates.push(PacketGenericSourceShapeCandidate {
            path: path.to_path_buf(),
            display_name,
            kind: NodeKind::FUNCTION,
            line,
            score: 115.0,
            coverage_role: "hook_mutation_flow".to_string(),
            producer: "packet_generic_hook_cache_source_probe".to_string(),
            eligible_for_sufficiency: true,
        });
    }

    if normalized_source.contains("middleware")
        && normalized_source.contains("hook")
        && normalized_source.contains("configuse")
        && hook_return_call
        && let Some((display_name, line)) = packet_first_exported_name_near(source, &["middleware"])
    {
        candidates.push(PacketGenericSourceShapeCandidate {
            path: path.to_path_buf(),
            display_name,
            kind: NodeKind::FUNCTION,
            line,
            score: 110.0,
            coverage_role: "hook_middleware_composition".to_string(),
            producer: "packet_generic_hook_cache_source_probe".to_string(),
            eligible_for_sufficiency: true,
        });
    }
}

fn collect_event_loop_command_shape_candidates(
    path: &Path,
    source: &str,
    candidates: &mut Vec<PacketGenericSourceShapeCandidate>,
) {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase())
        .unwrap_or_default();
    if extension != "c" {
        return;
    }
    let normalized_source = normalize_identifier(source);
    let has_server = normalized_source.contains("server");
    let has_command = normalized_source.contains("command");
    let has_event_loop = normalized_source.contains("eventloop")
        || normalized_source.contains("event_loop")
        || (normalized_source.contains("event") && normalized_source.contains("loop"));
    let has_client_input = normalized_source.contains("client")
        && (normalized_source.contains("input")
            || normalized_source.contains("buffer")
            || normalized_source.contains("read"));
    let functions = packet_c_function_names(source);

    if has_server
        && has_event_loop
        && let Some((name, line)) =
            packet_best_c_function_with_words(&functions, &["init", "server"], None)
                .or_else(|| packet_c_function_exact(&functions, "main"))
    {
        push_command_shape_candidate(
            path,
            candidates,
            name,
            line,
            "command_server_bootstrap",
            124.0,
        );
    }
    if has_event_loop
        && let Some((name, line)) = packet_c_function_ending_with(&functions, "Main", "main")
            .or_else(|| packet_best_c_function_with_words(&functions, &["event", "loop"], None))
    {
        push_command_shape_candidate(path, candidates, name, line, "command_event_loop", 128.0);
    }
    if has_command
        && has_client_input
        && let Some((name, line)) =
            packet_best_c_function_with_words(&functions, &["read", "client"], Some("read"))
                .or_else(|| {
                    packet_best_c_function_with_words(&functions, &["client", "input"], None)
                })
    {
        push_command_shape_candidate(path, candidates, name, line, "command_network_input", 130.0);
    }
    if has_command {
        if let Some((name, line)) =
            packet_best_c_function_with_words(&functions, &["process", "command"], None)
        {
            push_command_shape_candidate(path, candidates, name, line, "command_dispatch", 126.0);
        }
        if let Some((name, line)) = packet_c_function_exact(&functions, "call") {
            push_command_shape_candidate(path, candidates, name, line, "command_dispatch", 125.0);
        }
    }
}

fn push_command_shape_candidate(
    path: &Path,
    candidates: &mut Vec<PacketGenericSourceShapeCandidate>,
    display_name: String,
    line: u32,
    coverage_role: &str,
    score: f32,
) {
    candidates.push(PacketGenericSourceShapeCandidate {
        path: path.to_path_buf(),
        line,
        display_name,
        kind: NodeKind::FUNCTION,
        score,
        coverage_role: coverage_role.to_string(),
        producer: "packet_generic_command_source_probe".to_string(),
        eligible_for_sufficiency: true,
    });
}

fn packet_c_function_names(source: &str) -> Vec<(String, u32)> {
    source
        .lines()
        .enumerate()
        .filter_map(|(index, line)| {
            let name = packet_c_function_name_from_line(line)?;
            Some((name, index.saturating_add(1).try_into().unwrap_or(u32::MAX)))
        })
        .collect()
}

fn packet_c_function_name_from_line(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if trimmed.starts_with('#')
        || trimmed.ends_with(';')
        || trimmed.contains("typedef")
        || !trimmed.contains('(')
        || !trimmed.contains('{')
    {
        return None;
    }
    let before_paren = trimmed.split_once('(')?.0.trim_end();
    let name = before_paren
        .rsplit(|ch: char| !packet_source_identifier_char(ch))
        .next()?
        .trim();
    if name.is_empty()
        || matches!(
            name,
            "if" | "for" | "while" | "switch" | "return" | "sizeof"
        )
    {
        return None;
    }
    Some(name.to_string())
}

fn packet_best_c_function_with_words(
    functions: &[(String, u32)],
    words: &[&str],
    preferred_prefix: Option<&str>,
) -> Option<(String, u32)> {
    functions
        .iter()
        .filter(|(name, _)| {
            let normalized = normalize_identifier(name);
            words.iter().all(|word| normalized.contains(word))
        })
        .min_by_key(|(name, line)| {
            let normalized = normalize_identifier(name);
            let prefix_miss = preferred_prefix
                .map(|prefix| !normalized.starts_with(prefix))
                .unwrap_or(false);
            (prefix_miss, name.len(), *line)
        })
        .cloned()
}

fn packet_c_function_exact(functions: &[(String, u32)], expected: &str) -> Option<(String, u32)> {
    functions
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(expected))
        .cloned()
}

fn packet_c_function_ending_with(
    functions: &[(String, u32)],
    suffix: &str,
    excluded: &str,
) -> Option<(String, u32)> {
    functions
        .iter()
        .filter(|(name, _)| !name.eq_ignore_ascii_case(excluded) && name.ends_with(suffix))
        .min_by_key(|(name, line)| (name.len(), *line))
        .cloned()
}

fn collect_form_validation_shape_candidates(
    path: &Path,
    source: &str,
    candidates: &mut Vec<PacketGenericSourceShapeCandidate>,
) {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase())
        .unwrap_or_default();
    if !matches!(extension.as_str(), "html" | "htm") {
        return;
    }
    let source_lower = source.to_ascii_lowercase();
    if !source_lower.contains("<form") || !source_lower.contains("<input") {
        return;
    }
    let has_native_constraints = source_lower.contains("required")
        && source_lower.contains("pattern")
        && source_lower.contains("min=")
        && source_lower.contains("max=");
    if has_native_constraints {
        candidates.push(PacketGenericSourceShapeCandidate {
            path: path.to_path_buf(),
            display_name: "Native form constraints".to_string(),
            kind: NodeKind::ANNOTATION,
            line: packet_source_line_containing(source, "pattern").unwrap_or(1),
            score: 132.0,
            coverage_role: "form_native_constraints".to_string(),
            producer: "packet_generic_form_validation_source_probe".to_string(),
            eligible_for_sufficiency: true,
        });
    }
    if source_lower.contains("pattern=") {
        candidates.push(PacketGenericSourceShapeCandidate {
            path: path.to_path_buf(),
            display_name: "pattern".to_string(),
            kind: NodeKind::ANNOTATION,
            line: packet_source_line_containing(source, "pattern").unwrap_or(1),
            score: 130.0,
            coverage_role: "form_pattern_constraint".to_string(),
            producer: "packet_generic_form_validation_source_probe".to_string(),
            eligible_for_sufficiency: true,
        });
    }
    if let Some((attribute, line)) = packet_first_form_validation_bypass_attribute(source) {
        candidates.push(PacketGenericSourceShapeCandidate {
            path: path.to_path_buf(),
            display_name: attribute,
            kind: NodeKind::ANNOTATION,
            line,
            score: 129.0,
            coverage_role: "form_validation_bypass".to_string(),
            producer: "packet_generic_form_validation_source_probe".to_string(),
            eligible_for_sufficiency: true,
        });
    }
    if let Some(input_id) = packet_first_html_input_id(source) {
        let score = if packet_first_form_validation_bypass_attribute(source).is_some()
            || source_lower.contains("validity")
        {
            132.0
        } else {
            128.0
        };
        candidates.push(PacketGenericSourceShapeCandidate {
            path: path.to_path_buf(),
            display_name: format!("input#{input_id}"),
            kind: NodeKind::ANNOTATION,
            line: packet_source_line_containing(source, &format!("id=\"{input_id}\""))
                .or_else(|| packet_source_line_containing(source, &format!("id='{input_id}'")))
                .unwrap_or(1),
            score,
            coverage_role: "form_custom_input".to_string(),
            producer: "packet_generic_form_validation_source_probe".to_string(),
            eligible_for_sufficiency: true,
        });
    }
    if let Some((function, line)) = packet_first_form_validation_error_function(source) {
        candidates.push(PacketGenericSourceShapeCandidate {
            path: path.to_path_buf(),
            display_name: function,
            kind: NodeKind::FUNCTION,
            line,
            score: 134.0,
            coverage_role: "form_custom_error_rendering".to_string(),
            producer: "packet_generic_form_validation_source_probe".to_string(),
            eligible_for_sufficiency: true,
        });
    }
}

fn collect_buffered_io_shape_candidates(
    path: &Path,
    source: &str,
    candidates: &mut Vec<PacketGenericSourceShapeCandidate>,
) {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase())
        .unwrap_or_default();
    if !matches!(extension.as_str(), "kt" | "java" | "swift" | "go" | "rs") {
        return;
    }
    let buffered_type_names = packet_buffered_io_type_names(source);
    for (display_name, role, score, line) in &buffered_type_names {
        candidates.push(PacketGenericSourceShapeCandidate {
            path: path.to_path_buf(),
            display_name: display_name.clone(),
            kind: NodeKind::CLASS,
            line: *line,
            score: *score,
            coverage_role: (*role).to_string(),
            producer: "packet_generic_buffered_io_source_probe".to_string(),
            eligible_for_sufficiency: true,
        });
    }
    let source_lower = source.to_ascii_lowercase();
    if source_lower.contains("buffer()") && !buffered_type_names.is_empty() {
        candidates.push(PacketGenericSourceShapeCandidate {
            path: path.to_path_buf(),
            display_name: "buffer".to_string(),
            kind: NodeKind::FUNCTION,
            line: packet_source_line_containing(source, "buffer()").unwrap_or(1),
            score: 134.0,
            coverage_role: "buffered_wrapper_helper".to_string(),
            producer: "packet_generic_buffered_io_source_probe".to_string(),
            eligible_for_sufficiency: true,
        });
    }
}

fn packet_buffered_io_type_names(source: &str) -> Vec<(String, &'static str, f32, u32)> {
    let mut names = Vec::new();
    for (index, line) in source.lines().enumerate() {
        let Some(name) = packet_declared_type_name(line) else {
            continue;
        };
        let normalized = name.to_ascii_lowercase();
        if !normalized.contains("buffered") {
            continue;
        }
        let Some((role, score)) = packet_buffered_io_role(&normalized) else {
            continue;
        };
        names.push((
            name,
            role,
            score,
            index.saturating_add(1).try_into().unwrap_or(u32::MAX),
        ));
    }
    names
}

fn packet_declared_type_name(line: &str) -> Option<String> {
    let mut words = line
        .split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')
        .filter(|word| !word.is_empty());
    while let Some(word) = words.next() {
        if matches!(word, "class" | "struct" | "interface" | "object") {
            return words.next().map(|name| name.to_string());
        }
    }
    None
}

fn packet_buffered_io_role(normalized_name: &str) -> Option<(&'static str, f32)> {
    if normalized_name.ends_with("source") {
        return Some(("buffered_source_impl", 136.0));
    }
    if normalized_name.ends_with("sink") {
        return Some(("buffered_sink_impl", 135.0));
    }
    None
}

fn collect_url_session_request_shape_candidates(
    path: &Path,
    source: &str,
    candidates: &mut Vec<PacketGenericSourceShapeCandidate>,
) {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase())
        .unwrap_or_default();
    if extension != "swift" {
        return;
    }
    let source_lower = source.to_ascii_lowercase();
    let type_name = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.is_empty())
        .unwrap_or("Request");
    if source_lower.contains("func resume")
        && (source_lower.contains("task.resume") || source_lower.contains("task?.resume"))
    {
        candidates.push(PacketGenericSourceShapeCandidate {
            path: path.to_path_buf(),
            display_name: format!("{type_name}.resume"),
            kind: NodeKind::METHOD,
            line: packet_source_line_containing(source, "func resume").unwrap_or(1),
            score: 136.0,
            coverage_role: "request_resume_dispatch".to_string(),
            producer: "packet_generic_url_session_request_source_probe".to_string(),
            eligible_for_sufficiency: true,
        });
    }
    if source_lower.contains("func validate")
        && (source_lower.contains("validators") || source_lower.contains("validation"))
    {
        let score = if type_name.to_ascii_lowercase().contains("data") {
            138.0
        } else {
            135.0
        };
        candidates.push(PacketGenericSourceShapeCandidate {
            path: path.to_path_buf(),
            display_name: format!("{type_name}.validate"),
            kind: NodeKind::METHOD,
            line: packet_source_line_containing(source, "func validate").unwrap_or(1),
            score,
            coverage_role: "request_validation_pipeline".to_string(),
            producer: "packet_generic_url_session_request_source_probe".to_string(),
            eligible_for_sufficiency: true,
        });
    }
    if source_lower.contains("urlsession")
        && source_lower.contains("func urlsession")
        && (source_lower.contains("didreceive") || source_lower.contains("didcomplete"))
    {
        candidates.push(PacketGenericSourceShapeCandidate {
            path: path.to_path_buf(),
            display_name: format!("{type_name}.urlSession"),
            kind: NodeKind::METHOD,
            line: packet_source_line_containing(source, "func urlSession").unwrap_or(1),
            score: 137.0,
            coverage_role: "session_callbacks".to_string(),
            producer: "packet_generic_url_session_request_source_probe".to_string(),
            eligible_for_sufficiency: true,
        });
    }
}

fn collect_cited_request_validation_shape_candidates(
    answer: &AgentAnswerDto,
    candidates: &mut Vec<PacketGenericSourceShapeCandidate>,
) {
    for citation in &answer.citations {
        let Some(path) = citation.file_path.as_deref().map(Path::new) else {
            continue;
        };
        if path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_none_or(|extension| !extension.eq_ignore_ascii_case("swift"))
        {
            continue;
        }
        let Some(type_name) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        if normalize_identifier(type_name) != normalize_identifier(&citation.display_name) {
            continue;
        }
        let Ok(source) = std::fs::read_to_string(path) else {
            continue;
        };
        if !packet_swift_request_validation_source(&source) {
            continue;
        }
        candidates.push(PacketGenericSourceShapeCandidate {
            path: path.to_path_buf(),
            display_name: format!("{}.validate", citation.display_name),
            kind: NodeKind::METHOD,
            line: packet_source_line_containing(&source, "func validate").unwrap_or(1),
            score: 140.0,
            coverage_role: "request_validation_pipeline".to_string(),
            producer: "packet_generic_url_session_request_source_probe".to_string(),
            eligible_for_sufficiency: true,
        });
    }
}

fn packet_swift_request_validation_source(source: &str) -> bool {
    let source_lower = source.to_ascii_lowercase();
    source_lower.contains("func validate")
        && source_lower.contains("request")
        && (source_lower.contains("validators") || source_lower.contains("validation"))
}

fn packet_first_form_validation_bypass_attribute(source: &str) -> Option<(String, u32)> {
    for (index, line) in source.lines().enumerate() {
        let lower = line.to_ascii_lowercase();
        if !lower.contains("<form") {
            continue;
        }
        for attribute in line.split_ascii_whitespace() {
            let attribute = attribute
                .trim_matches(|ch: char| ch == '<' || ch == '>' || ch == '/')
                .split('=')
                .next()
                .unwrap_or_default();
            let normalized = attribute.to_ascii_lowercase();
            if normalized.starts_with("no") && normalized.ends_with("validate") {
                return Some((
                    attribute.to_string(),
                    index.saturating_add(1).try_into().unwrap_or(u32::MAX),
                ));
            }
        }
    }
    None
}

fn packet_first_html_input_id(source: &str) -> Option<String> {
    for line in source.lines() {
        let lower = line.to_ascii_lowercase();
        if !lower.contains("<input") || !lower.contains("id=") {
            continue;
        }
        for quote in ['"', '\''] {
            let marker = format!("id={quote}");
            let Some(start) = lower.find(&marker) else {
                continue;
            };
            let value_start = start + marker.len();
            let value = &line[value_start..];
            let Some(end) = value.find(quote) else {
                continue;
            };
            let id = value[..end].trim();
            if !id.is_empty() {
                return Some(id.to_string());
            }
        }
    }
    None
}

fn packet_first_form_validation_error_function(source: &str) -> Option<(String, u32)> {
    let source_lower = source.to_ascii_lowercase();
    if !(source_lower.contains("validity.valuemissing")
        || source_lower.contains("validity.typemismatch")
        || source_lower.contains("validity.tooshort"))
    {
        return None;
    }
    source.lines().enumerate().find_map(|(index, line)| {
        let trimmed = line.trim();
        let name = trimmed.strip_prefix("function ")?.split_once('(')?.0.trim();
        if name.is_empty() || !name.chars().all(packet_source_identifier_char) {
            return None;
        }
        let normalized = normalize_identifier(name);
        (normalized.contains("error") || normalized.contains("message")).then_some((
            name.to_string(),
            index.saturating_add(1).try_into().unwrap_or(u32::MAX),
        ))
    })
}

fn packet_source_line_containing(source: &str, needle: &str) -> Option<u32> {
    source
        .lines()
        .position(|line| line.to_ascii_lowercase().contains(needle))
        .map(|index| index.saturating_add(1).try_into().unwrap_or(u32::MAX))
}

fn packet_source_shape_has_cache_helper_call(normalized_source: &str) -> bool {
    normalized_source.contains("cachehelper") && normalized_source.contains("create")
}

fn packet_source_shape_has_hook_return_call(normalized_source: &str) -> bool {
    normalized_source.contains("returnuse") && normalized_source.contains("hook")
}

fn packet_source_shape_has_any(source: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| source.contains(needle))
}

fn packet_first_exported_name_near(
    source: &str,
    normalized_needles: &[&str],
) -> Option<(String, u32)> {
    source.lines().enumerate().find_map(|(index, line)| {
        let name = packet_exported_name_from_line(line)?;
        let normalized = normalize_identifier(&name);
        normalized_needles
            .iter()
            .any(|needle| normalized.contains(needle))
            .then(|| (name, index.saturating_add(1).try_into().unwrap_or(u32::MAX)))
    })
}

fn packet_exported_name_from_line(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    let remainder = trimmed
        .strip_prefix("export async function ")
        .or_else(|| trimmed.strip_prefix("export function "))
        .or_else(|| trimmed.strip_prefix("export const "))?;
    let name = remainder
        .chars()
        .take_while(|ch| packet_source_identifier_char(*ch))
        .collect::<String>();
    (!name.is_empty()).then_some(name)
}

fn packet_first_line_containing(source: &str, needles: &[&str]) -> Option<u32> {
    source
        .lines()
        .enumerate()
        .find(|(_, line)| needles.iter().all(|needle| line.contains(needle)))
        .and_then(|(index, _)| index.saturating_add(1).try_into().ok())
}

fn collect_csharp_mapper_shape_candidates(
    path: &Path,
    source: &str,
    candidates: &mut Vec<PacketGenericSourceShapeCandidate>,
) {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase())
        .unwrap_or_default();
    if extension != "cs" {
        return;
    }
    let normalized_source = normalize_identifier(source);
    let source_lower = source.to_ascii_lowercase();
    if !(normalized_source.contains("mapper")
        || (normalized_source.contains("map") && normalized_source.contains("destination")))
    {
        return;
    }
    let normalized_path = path
        .to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase();
    let internal_mapper_strategy_path = normalized_path.contains("/mappers/")
        || normalized_path.contains("/mapperstrateg")
        || normalized_path.contains("/mappingstrateg");

    if packet_csharp_source_has_public_mapper_api(&normalized_source) {
        let mut public_mapper_interfaces = Vec::new();
        if !internal_mapper_strategy_path {
            for (name, line) in packet_csharp_declared_type_names(source, "interface") {
                let normalized_name = normalize_identifier(&name);
                if packet_csharp_public_mapper_api_name(&normalized_name)
                    && !normalized_name.contains("internal")
                {
                    public_mapper_interfaces.push((name, line));
                }
            }
        }
        let runtime_mapper_method =
            packet_csharp_runtime_mapper_method_candidate(source, &normalized_source);
        let grouped_facade = packet_csharp_public_mapper_api_facade_group(
            &public_mapper_interfaces,
            runtime_mapper_method.as_ref(),
        );
        if let Some((display_name, line)) = grouped_facade {
            candidates.push(PacketGenericSourceShapeCandidate {
                path: path.to_path_buf(),
                display_name,
                kind: NodeKind::METHOD,
                line,
                score: 112.0,
                coverage_role: "mapper public api".to_string(),
                producer: "packet_generic_csharp_mapper_source_probe".to_string(),
                eligible_for_sufficiency: false,
            });
        } else {
            for (name, line) in public_mapper_interfaces {
                candidates.push(PacketGenericSourceShapeCandidate {
                    path: path.to_path_buf(),
                    display_name: name,
                    kind: NodeKind::INTERFACE,
                    line,
                    score: 108.0,
                    coverage_role: "mapper public api".to_string(),
                    producer: "packet_generic_csharp_mapper_source_probe".to_string(),
                    eligible_for_sufficiency: false,
                });
            }
            if let Some((owner, method, line)) = runtime_mapper_method.as_ref() {
                candidates.push(PacketGenericSourceShapeCandidate {
                    path: path.to_path_buf(),
                    display_name: format!("{owner}.{method}"),
                    kind: NodeKind::METHOD,
                    line: *line,
                    score: 109.0,
                    coverage_role: "mapper public api".to_string(),
                    producer: "packet_generic_csharp_mapper_source_probe".to_string(),
                    eligible_for_sufficiency: false,
                });
            }
        }
    }

    if packet_csharp_source_has_mapping_configuration_owner(&normalized_source) {
        for (name, line) in packet_csharp_declared_type_names(source, "class") {
            let normalized_name = normalize_identifier(&name);
            if normalized_name.contains("configuration")
                && (normalized_name.contains("mapper") || normalized_name.contains("mapping"))
            {
                candidates.push(PacketGenericSourceShapeCandidate {
                    path: path.to_path_buf(),
                    display_name: name,
                    kind: NodeKind::CLASS,
                    line,
                    score: 99.0,
                    coverage_role: "mapper configuration".to_string(),
                    producer: "packet_generic_csharp_mapper_source_probe".to_string(),
                    eligible_for_sufficiency: false,
                });
            }
        }
    }

    if packet_csharp_source_has_type_map_lambda_plan(&normalized_source) {
        let type_owner = packet_csharp_declared_type_names(source, "class")
            .into_iter()
            .find(|(name, _)| {
                let normalized = normalize_identifier(name);
                normalized.contains("map")
                    && (normalized.contains("type")
                        || normalized_source.contains("sourcetype")
                        || normalized_source.contains("destinationtype"))
            });
        if let Some((owner, _)) = type_owner {
            for (method, line) in packet_csharp_method_names(source) {
                let normalized_method = normalize_identifier(&method);
                if normalized_method.contains("lambda")
                    && (normalized_method.contains("map") || source_lower.contains("mapexpression"))
                {
                    candidates.push(PacketGenericSourceShapeCandidate {
                        path: path.to_path_buf(),
                        display_name: format!("{owner}.{method}"),
                        kind: NodeKind::METHOD,
                        line,
                        score: 99.0,
                        coverage_role: "mapping execution plan".to_string(),
                        producer: "packet_generic_csharp_mapper_source_probe".to_string(),
                        eligible_for_sufficiency: false,
                    });
                }
            }
        }
    }
}

fn packet_csharp_source_has_public_mapper_api(normalized_source: &str) -> bool {
    normalized_source.contains("interface")
        && normalized_source.contains("map")
        && normalized_source.contains("source")
        && normalized_source.contains("destination")
        && (normalized_source.contains("mapper") || normalized_source.contains("mapping"))
}

fn packet_csharp_public_mapper_api_name(normalized_name: &str) -> bool {
    normalized_name.contains("mapper")
        && ![
            "action",
            "configuration",
            "convention",
            "destinationname",
            "expression",
            "member",
            "operation",
            "options",
            "projection",
            "source",
        ]
        .iter()
        .any(|needle| normalized_name.contains(needle))
}

fn packet_csharp_runtime_mapper_method_candidate(
    source: &str,
    normalized_source: &str,
) -> Option<(String, String, u32)> {
    if !(normalized_source.contains("class")
        && normalized_source.contains("mapper")
        && normalized_source.contains("mapcore")
        && normalized_source.contains("getexecutionplan"))
    {
        return None;
    }
    let owner = packet_csharp_declared_type_names(source, "class")
        .into_iter()
        .find_map(|(name, _)| {
            packet_csharp_public_mapper_api_name(&normalize_identifier(&name)).then_some(name)
        })?;
    packet_csharp_method_names(source)
        .into_iter()
        .find(|(method, _)| normalize_identifier(method) == "map")
        .map(|(method, line)| (owner, method, line))
}

fn packet_csharp_public_mapper_api_facade_group(
    interfaces: &[(String, u32)],
    runtime_method: Option<&(String, String, u32)>,
) -> Option<(String, u32)> {
    let mut names = Vec::new();
    let mut line = u32::MAX;
    for (name, name_line) in interfaces {
        if !names.iter().any(|existing| existing == name) {
            names.push(name.clone());
            line = line.min(*name_line);
        }
    }
    if let Some((owner, method, method_line)) = runtime_method {
        let display_name = format!("{owner}.{method}");
        if !names.iter().any(|existing| existing == &display_name) {
            names.push(display_name);
            line = line.min(*method_line);
        }
    }
    if names.len() < 2 {
        return None;
    }
    Some((format!("public mapper API: {}", names.join(", ")), line))
}

fn packet_csharp_source_has_mapping_configuration_owner(normalized_source: &str) -> bool {
    normalized_source.contains("configuration")
        && (normalized_source.contains("configuredmaps")
            || normalized_source.contains("resolvedmaps")
            || normalized_source.contains("typemaps")
            || normalized_source.contains("executionplans"))
        && (normalized_source.contains("buildexecutionplan")
            || normalized_source.contains("createmapper")
            || normalized_source.contains("compilemappings"))
}

fn packet_csharp_source_has_type_map_lambda_plan(normalized_source: &str) -> bool {
    normalized_source.contains("lambda")
        && normalized_source.contains("map")
        && (normalized_source.contains("sourcetype") || normalized_source.contains("source"))
        && (normalized_source.contains("destinationtype")
            || normalized_source.contains("destination"))
        && (normalized_source.contains("planbuilder")
            || normalized_source.contains("mapexpression")
            || normalized_source.contains("expression"))
}

fn packet_csharp_declared_type_names(source: &str, keyword: &str) -> Vec<(String, u32)> {
    source
        .lines()
        .enumerate()
        .filter_map(|(index, line)| {
            if packet_source_line_is_comment_like(line) {
                return None;
            }
            packet_text_after_keyword(line, keyword).and_then(|after| {
                packet_identifier_tokens(after)
                    .into_iter()
                    .find(|token| !packet_csharp_modifier_token(token))
                    .map(|token| {
                        (
                            token,
                            index.saturating_add(1).try_into().unwrap_or(u32::MAX),
                        )
                    })
            })
        })
        .collect()
}

fn packet_csharp_method_names(source: &str) -> Vec<(String, u32)> {
    source
        .lines()
        .enumerate()
        .filter_map(|(index, line)| {
            if packet_source_line_is_comment_like(line) || !line.contains('(') {
                return None;
            }
            let before_paren = line.split_once('(')?.0;
            let method_prefix = before_paren
                .rfind('<')
                .map(|generic_start| &before_paren[..generic_start])
                .unwrap_or(before_paren);
            let name = packet_identifier_tokens(method_prefix)
                .into_iter()
                .rev()
                .find(|token| !packet_csharp_modifier_token(token))?;
            Some((name, index.saturating_add(1).try_into().unwrap_or(u32::MAX)))
        })
        .collect()
}

fn packet_csharp_modifier_token(token: &str) -> bool {
    matches!(
        normalize_identifier(token).as_str(),
        "public"
            | "private"
            | "protected"
            | "internal"
            | "static"
            | "sealed"
            | "abstract"
            | "partial"
            | "readonly"
            | "virtual"
            | "override"
            | "async"
            | "new"
            | "where"
            | "return"
    )
}

fn collect_runtime_formatting_shape_candidates(
    path: &Path,
    source: &str,
    candidates: &mut Vec<PacketGenericSourceShapeCandidate>,
) {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase())
        .unwrap_or_default();
    if !matches!(
        extension.as_str(),
        "h" | "hpp" | "hh" | "cc" | "cpp" | "cxx"
    ) {
        return;
    }
    for (index, line) in source.lines().enumerate() {
        if packet_source_line_is_comment_like(line) {
            continue;
        }
        let Some((name, kind)) = packet_cpp_declared_type_name(line) else {
            continue;
        };
        let normalized = normalize_identifier(&name);
        let argument_store_shape = normalized.contains("format")
            && (normalized.contains("arg") || normalized.contains("argument"))
            && normalized.contains("store");
        let failure_type_shape = normalized.contains("format")
            && (normalized.contains("error") || normalized.contains("failure"));
        if !argument_store_shape && !failure_type_shape {
            continue;
        }
        let score = if argument_store_shape { 94.0 } else { 92.0 };
        candidates.push(PacketGenericSourceShapeCandidate {
            path: path.to_path_buf(),
            display_name: name,
            kind,
            line: index.saturating_add(1).try_into().unwrap_or(u32::MAX),
            score,
            coverage_role: if argument_store_shape {
                "runtime format argument store".to_string()
            } else {
                "runtime formatting failure type".to_string()
            },
            producer: "packet_generic_runtime_formatting_source_probe".to_string(),
            eligible_for_sufficiency: true,
        });
    }
}

fn packet_cpp_declared_type_name(line: &str) -> Option<(String, NodeKind)> {
    for (keyword, kind) in [
        ("class", NodeKind::CLASS),
        ("struct", NodeKind::STRUCT),
        ("using", NodeKind::TYPEDEF),
    ] {
        let Some(after) = packet_text_after_keyword(line, keyword) else {
            continue;
        };
        for token in packet_identifier_tokens(after) {
            let normalized = normalize_identifier(&token);
            if normalized.is_empty()
                || matches!(
                    normalized.as_str(),
                    "typename" | "template" | "public" | "private" | "protected" | "default"
                )
                || token.chars().all(|ch| ch.is_ascii_uppercase() || ch == '_')
            {
                continue;
            }
            return Some((token, kind));
        }
    }
    None
}

fn packet_text_after_keyword<'a>(line: &'a str, keyword: &str) -> Option<&'a str> {
    let lower = line.to_ascii_lowercase();
    let index = lower.find(keyword)?;
    let before = lower[..index].chars().last();
    let after_index = index + keyword.len();
    let after = lower[after_index..].chars().next();
    if before.is_some_and(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        || after.is_some_and(|ch| ch.is_ascii_alphanumeric() || ch == '_')
    {
        return None;
    }
    Some(&line[after_index..])
}

fn packet_identifier_tokens(input: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut token = String::new();
    for ch in input.chars() {
        if packet_source_identifier_char(ch) {
            token.push(ch);
        } else if !token.is_empty() {
            tokens.push(std::mem::take(&mut token));
        }
    }
    if !token.is_empty() {
        tokens.push(token);
    }
    tokens
}

fn collect_css_animation_variable_candidates(
    path: &Path,
    source: &str,
    candidates: &mut Vec<PacketGenericSourceShapeCandidate>,
) {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase())
        .unwrap_or_default();
    if extension != "css" {
        return;
    }
    let lower = source.to_ascii_lowercase();
    if !lower.contains(":root") || !lower.contains("--") {
        return;
    }
    let custom_properties = packet_css_custom_properties(source);
    let animation_properties = custom_properties
        .into_iter()
        .filter(|property| {
            let normalized = normalize_identifier(property);
            normalized.contains("animation")
                || normalized.contains("animate")
                || normalized.contains("duration")
                || normalized.contains("delay")
                || normalized.contains("repeat")
        })
        .collect::<Vec<_>>();
    if animation_properties.is_empty() {
        return;
    }
    let display_name = animation_properties
        .first()
        .cloned()
        .unwrap_or_else(|| "css custom property".to_string());
    candidates.push(PacketGenericSourceShapeCandidate {
        path: path.to_path_buf(),
        display_name,
        kind: NodeKind::CONSTANT,
        line: packet_first_css_custom_property_line(source).unwrap_or(1),
        score: 96.0 + animation_properties.len().min(4) as f32,
        coverage_role: "css animation variables".to_string(),
        producer: "packet_generic_css_variable_source_probe".to_string(),
        eligible_for_sufficiency: true,
    });
}

fn packet_css_custom_properties(source: &str) -> Vec<String> {
    let mut properties = Vec::new();
    for line in source.lines() {
        let trimmed = line.trim();
        let Some(start) = trimmed.find("--") else {
            continue;
        };
        let name = trimmed[start..]
            .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '-' || ch == '_'))
            .next()
            .unwrap_or_default();
        if name.len() > 2 && !properties.iter().any(|existing| existing == name) {
            properties.push(name.to_string());
        }
    }
    properties
}

fn packet_first_css_custom_property_line(source: &str) -> Option<u32> {
    source
        .lines()
        .position(|line| line.contains("--"))
        .map(|index| index.saturating_add(1).try_into().unwrap_or(u32::MAX))
}

fn packet_source_identifier_char(ch: char) -> bool {
    ch == '_' || ch == '$' || ch.is_ascii_alphanumeric()
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
    let mut file_scoped = 0usize;
    let mut already_cited = 0usize;
    let mut no_path = 0usize;
    let mut too_large = 0usize;
    let mut read_failed = 0usize;
    let mut no_anchor = 0usize;
    for query in required_queries {
        if appended >= 16 {
            break;
        }
        let Some(parts) = packet_file_scoped_symbol_probe_parts(&query) else {
            continue;
        };
        file_scoped = file_scoped.saturating_add(1);
        if packet_probe_query_is_cited(&query, answer) {
            already_cited = already_cited.saturating_add(1);
            continue;
        }
        let Some(path) = packet_required_probe_source_path(project_root, &parts, &answer.citations)
        else {
            no_path = no_path.saturating_add(1);
            continue;
        };
        let Ok(metadata) = path.metadata() else {
            no_path = no_path.saturating_add(1);
            continue;
        };
        if metadata.len() > 1_500_000 {
            too_large = too_large.saturating_add(1);
            continue;
        }
        let Ok(source) = std::fs::read_to_string(&path) else {
            read_failed = read_failed.saturating_add(1);
            continue;
        };
        let Some(anchor) = packet_required_probe_source_anchor(&parts, &source) else {
            no_anchor = no_anchor.saturating_add(1);
            continue;
        };
        let path_string = path.to_string_lossy().to_string();
        if answer.citations.iter().any(|existing| {
            existing.display_name == anchor.display_name
                && existing.file_path.as_deref().is_some_and(|existing_path| {
                    packet_display_path(existing_path) == packet_display_path(&path_string)
                })
        }) {
            already_cited = already_cited.saturating_add(1);
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
                tier_cap: Some(40.0),
                boosts: Vec::new(),
                dampening: Vec::new(),
                final_rank_reason: Some("required source probe".to_string()),
                provenance: vec!["packet_required_file_scoped_source_probe".to_string()],
            }),
            evidence_tier: Some(codestory_contracts::api::PacketEvidenceTierDto::LexicalSource),
            evidence_producer: Some("packet_required_file_scoped_source_probe".to_string()),
            resolution_status: Some(
                codestory_contracts::api::PacketEvidenceResolutionDto::SourceRangeOnly,
            ),
            loss_reason: None,
            coverage_role: Some("required source probe".to_string()),
            eligible_for_sufficiency: Some(true),
        });
        appended += 1;
    }

    if appended > 0 || file_scoped > 0 {
        answer.retrieval_trace.annotations.push(format!(
            "packet_required_file_scoped_source_citations file_scoped={file_scoped} appended={appended} already_cited={already_cited} no_path={no_path} too_large={too_large} read_failed={read_failed} no_anchor={no_anchor}"
        ));
    }
}

fn packet_file_scoped_source_probe_inputs_from_plan(
    plan: &PacketPlanDto,
    extra_probes: &[String],
) -> Vec<String> {
    let mut probes = Vec::new();
    for probe in extra_probes {
        probes.push(probe.clone());
    }
    for query in &plan.queries {
        if packet_file_scoped_symbol_probe_parts(&query.query).is_some()
            && !probes
                .iter()
                .any(|probe| probe.eq_ignore_ascii_case(&query.query))
        {
            probes.push(query.query.clone());
        }
    }
    probes
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
        let Some(path) = citation.file_path.as_deref() else {
            continue;
        };
        let display_path = packet_display_path(path)
            .replace('\\', "/")
            .to_ascii_lowercase();
        if display_path.ends_with(&normalized_query_path) {
            return Some(std::path::PathBuf::from(path));
        }
    }
    for citation in citations {
        let Some(path) = citation.file_path.as_deref() else {
            continue;
        };
        let file_name = packet_display_path(path)
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        if packet_probe_file_name_matches(&parts.file_name, &file_name) {
            return Some(std::path::PathBuf::from(path));
        }
    }
    if !parts.query_path.contains('/') {
        return packet_find_unique_source_file_by_name(project_root, &parts.file_name);
    }
    None
}

fn packet_find_unique_source_file_by_name(
    project_root: &Path,
    file_name: &str,
) -> Option<std::path::PathBuf> {
    let mut queue = VecDeque::from([project_root.to_path_buf()]);
    let mut found = None;
    let mut visited_dirs = 0usize;

    while let Some(dir) = queue.pop_front() {
        visited_dirs = visited_dirs.saturating_add(1);
        if visited_dirs > 20_000 {
            return None;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            let entry_name = entry.file_name();
            let entry_name = entry_name.to_string_lossy();
            if file_type.is_dir() {
                if !packet_source_probe_skip_dir(&entry_name) {
                    queue.push_back(entry.path());
                }
                continue;
            }
            if packet_probe_file_name_matches(file_name, &entry_name) {
                if found.is_some() {
                    return None;
                }
                found = Some(entry.path());
            }
        }
    }

    found
}

fn packet_source_probe_skip_dir(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        ".git" | ".hg" | ".svn" | "node_modules" | "target" | "dist" | "build" | "coverage"
    )
}

fn packet_required_probe_source_anchor(
    parts: &PacketFileScopedSymbolProbe,
    source: &str,
) -> Option<PacketRequiredSourceAnchor> {
    let display_name = packet_required_probe_source_display_name(parts);
    for (index, line) in source.lines().enumerate() {
        if packet_source_line_declares_file_scoped_probe(line, parts) {
            let kind = packet_source_probe_anchor_kind(line, parts);
            return Some(PacketRequiredSourceAnchor {
                display_name,
                kind,
                line: index.saturating_add(1).try_into().unwrap_or(u32::MAX),
            });
        }
    }
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

fn packet_required_probe_source_display_name(parts: &PacketFileScopedSymbolProbe) -> String {
    let display_name = parts.raw_symbols.join(" ");
    if display_name.contains('.') || display_name.contains(':') || display_name.contains('#') {
        return display_name;
    }
    let path = parts.query_path.replace('\\', "/");
    let receiver = path.rsplit('/').next().and_then(|name| {
        let (stem, extension) = name.rsplit_once('.')?;
        match (stem.to_ascii_lowercase().as_str(), extension) {
            ("application", "js") => Some("app"),
            ("response", "js") => Some("res"),
            ("request", "js") => Some("req"),
            (_, "java") => Some(stem),
            _ => None,
        }
    });
    receiver
        .map(|receiver| format!("{receiver}.{display_name}"))
        .unwrap_or(display_name)
}

fn packet_source_line_declares_file_scoped_probe(
    line: &str,
    parts: &PacketFileScopedSymbolProbe,
) -> bool {
    if parts.raw_symbols.is_empty() {
        return false;
    }
    let terminal = packet_required_probe_terminal_symbol(&parts.raw_symbols.join(" "));
    let normalized_terminal = normalize_identifier(&terminal);
    if normalized_terminal.is_empty() || !normalize_identifier(line).contains(&normalized_terminal)
    {
        return false;
    }
    packet_source_line_declares_named_symbol(line, &normalized_terminal)
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

    if parts.symbols.len() > 1
        && !packet_source_line_is_comment_like(line)
        && parts
            .symbols
            .iter()
            .all(|symbol| normalized_line.contains(symbol))
    {
        return true;
    }

    let terminal = packet_required_probe_terminal_symbol(&raw_display);
    let normalized_terminal = normalize_identifier(&terminal);
    if normalized_terminal.is_empty() || !normalized_line.contains(&normalized_terminal) {
        return false;
    }

    if packet_shell_function_line_matches(line, &normalized_terminal) {
        return true;
    }

    if parts.symbols.len() == 1
        && normalized_line.contains(&parts.symbols[0])
        && (packet_source_line_looks_like_code_call(line)
            || packet_cpp_template_instantiation(line))
    {
        return true;
    }

    packet_source_line_declares_named_symbol(line, &normalized_terminal)
        || normalized_line == normalized_display
        || normalized_line.ends_with(&normalized_display)
}

fn packet_source_line_looks_like_code_call(line: &str) -> bool {
    if packet_source_line_is_comment_like(line) {
        return false;
    }
    let trimmed = line.trim_start();
    trimmed.contains('(') && (trimmed.contains(';') || trimmed.contains('{'))
}

fn packet_source_line_is_comment_like(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("//") || trimmed.starts_with('#') || trimmed.starts_with('*')
}

fn packet_cpp_template_instantiation(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    lower.contains("template ") && (lower.contains("api ") || lower.contains("extern "))
}

fn packet_shell_function_line_matches(line: &str, normalized_terminal: &str) -> bool {
    if normalized_terminal.is_empty() {
        return false;
    }
    let trimmed = line.trim_start().to_ascii_lowercase();
    let compact = trimmed
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect::<String>();
    compact.starts_with(&format!("{normalized_terminal}()"))
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
        || packet_shell_function_line_matches(
            line,
            &packet_required_probe_terminal_symbol(&parts.raw_symbols.join(" ")),
        )
    {
        NodeKind::METHOD
    } else {
        NodeKind::ANNOTATION
    }
}
#[cfg(test)]
fn packet_claim_for_role(
    _key: &str,
    role: PacketEvidenceRole,
    citation: &AgentCitationDto,
    prompt: &str,
) -> String {
    build_packet_claim_for_role(role, citation, prompt, &packet_rank_terms(prompt))
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
            trace.finish_ok_with_duration_ms(
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
                    field("mode", "packet_initial_sidecar_query"),
                    field("sidecar_query_ms", shadow.retrieval_total_ms.to_string()),
                ],
                shadow.retrieval_total_ms,
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
    let mut citation = AgentCitationDto {
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
            tier_cap: None,
            boosts: Vec::new(),
            dampening: Vec::new(),
            final_rank_reason: None,
            provenance: Vec::new(),
        }),
        evidence_tier: scored.hit.evidence_tier,
        evidence_producer: scored.hit.evidence_producer.clone(),
        resolution_status: scored.hit.resolution_status,
        loss_reason: scored.hit.loss_reason.clone(),
        coverage_role: scored.hit.coverage_role.clone(),
        eligible_for_sufficiency: scored.hit.eligible_for_sufficiency,
    };
    decorate_citation_from_hit(&mut citation, &scored.hit);
    citation
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
        evidence_tier: Some(codestory_contracts::api::PacketEvidenceTierDto::ResolvedGraph),
        evidence_producer: Some("test_grounding_symbol".to_string()),
        resolution_status: Some(codestory_contracts::api::PacketEvidenceResolutionDto::Resolved),
        loss_reason: None,
        coverage_role: None,
        eligible_for_sufficiency: Some(true),
        score_breakdown: Some(RetrievalScoreBreakdownDto {
            lexical: 0.35,
            semantic: 0.0,
            graph: 0.20,
            total: 0.55,
            tier_cap: None,
            boosts: Vec::new(),
            dampening: Vec::new(),
            final_rank_reason: None,
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
    use crate::agent::packet_batch::packet_anchor_probe_limit;
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
            evidence_tier: Some(codestory_contracts::api::PacketEvidenceTierDto::ResolvedGraph),
            evidence_producer: Some("test".to_string()),
            resolution_status: Some(
                codestory_contracts::api::PacketEvidenceResolutionDto::Resolved,
            ),
            loss_reason: None,
            coverage_role: None,
            eligible_for_sufficiency: Some(true),
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
            tier_cap: None,
            boosts: Vec::new(),
            dampening: Vec::new(),
            final_rank_reason: None,
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
                tier_cap: None,
                boosts: Vec::new(),
                dampening: Vec::new(),
                final_rank_reason: None,
                provenance: Vec::new(),
            }),
            evidence_tier: Some(codestory_contracts::api::PacketEvidenceTierDto::ResolvedGraph),
            evidence_producer: Some("test".to_string()),
            resolution_status: Some(
                codestory_contracts::api::PacketEvidenceResolutionDto::Resolved,
            ),
            loss_reason: None,
            coverage_role: None,
            eligible_for_sufficiency: Some(true),
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
                packet_sidecar_diagnostics: Vec::new(),
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
    fn packet_sufficiency_does_not_promote_summary_to_covered_claim() {
        let question = "Explain packet sufficiency proof boundaries.";
        let answer = packet_answer_fixture(question, Vec::new());
        let budget = PacketBudgetDto {
            requested: PacketBudgetModeDto::Compact,
            limits: packet_budget_limits(PacketBudgetModeDto::Compact),
            used: PacketBudgetUsageDto {
                anchors: 0,
                files: 0,
                snippets: 0,
                trail_edges: 0,
                output_bytes: 0,
            },
            truncated: false,
            omitted_sections: Vec::new(),
            next_deeper_command: None,
        };
        let sufficiency = build_packet_sufficiency(
            packet_fixture_project_root(),
            question,
            PacketTaskClassDto::ArchitectureExplanation,
            &answer,
            &budget,
        );

        assert_ne!(sufficiency.status, PacketSufficiencyStatusDto::Sufficient);
        assert!(
            sufficiency.covered_claims.is_empty(),
            "covered_claims must only contain source-backed claims, not summary fallback: {sufficiency:?}"
        );
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
            .take(packet_anchor_probe_limit(PacketBudgetModeDto::Compact))
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
    fn packet_required_probe_promotion_keeps_multiple_sql_schema_scripts() {
        let mut sqlite = test_packet_citation("db/schema_sqlite.sql", "db/schema_sqlite.sql", 0.7);
        sqlite.kind = NodeKind::FILE;
        let mut mysql = test_packet_citation("db/schema_mysql.sql", "db/schema_mysql.sql", 0.6);
        mysql.kind = NodeKind::FILE;
        let mut postgres =
            test_packet_citation("db/schema_postgresql.sql", "db/schema_postgresql.sql", 0.5);
        postgres.kind = NodeKind::FILE;
        let distractor = test_packet_citation("SchemaBuilder", "src/schema_builder.rs", 0.95);
        let mut answer = packet_answer_fixture(
            "Explain SQL schema relationships across seed scripts.",
            vec![distractor, sqlite, mysql, postgres],
        );

        promote_required_probe_citations(&mut answer, &["sql schema scripts".to_string()]);

        let promoted_sql_paths = answer
            .citations
            .iter()
            .take(3)
            .filter_map(|citation| citation.file_path.as_deref())
            .collect::<Vec<_>>();
        assert_eq!(
            promoted_sql_paths,
            vec![
                "db/schema_sqlite.sql",
                "db/schema_mysql.sql",
                "db/schema_postgresql.sql"
            ]
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
    fn packet_sufficiency_treats_covered_planned_flow_probes_as_hints() {
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

        assert_eq!(sufficiency.status, PacketSufficiencyStatusDto::Sufficient);
        assert!(
            sufficiency
                .gaps
                .iter()
                .all(|gap| !gap.contains("exec session")
                    && !gap.contains("exec command")
                    && !gap.contains("turn start")),
            "covered flow roles should keep planned probe strings as nonblocking hints: {sufficiency:?}"
        );
        assert!(
            sufficiency
                .follow_up_commands
                .iter()
                .all(|command| !command.contains("--query 'exec session'")),
            "sufficient packets should not emit follow-up searches for covered probe hints: {sufficiency:?}"
        );
    }

    #[test]
    fn packet_sufficiency_uses_selected_plan_role_probes() {
        let question = "Explain how the form validation examples combine native HTML constraints with custom JavaScript validation.";
        let plan = build_packet_plan_with_extra(
            question,
            Some(PacketTaskClassDto::ArchitectureExplanation),
            PacketBudgetModeDto::Compact,
            &[],
        );
        let extra_probes = packet_plan_sufficiency_extra_probes(&plan, &[]);
        for expected in ["pattern", "custom validation flow", "validity state"] {
            assert!(
                extra_probes.iter().any(|query| query == expected),
                "expected selected plan probe {expected:?} in {extra_probes:?}"
            );
        }

        let limits = packet_budget_limits(PacketBudgetModeDto::Compact);
        let mut answer = packet_answer_fixture(
            question,
            vec![
                test_packet_citation(
                    "showError",
                    "html/forms/form-validation/detailed-custom-validation.html",
                    0.9,
                ),
                test_packet_citation("errors", "accessibility/css/form-validation.html", 0.8),
                test_packet_citation(
                    "validate",
                    "accessibility/aria/validation-checkbox-disabled.js",
                    0.8,
                ),
                test_packet_citation(
                    "advancedForm",
                    "html/forms/native-form-widgets/advanced-examples.html",
                    0.8,
                ),
            ],
        );
        rank_packet_evidence(question, &mut answer);
        append_packet_evidence_sections(&mut answer, plan.task_class, &limits);
        let budget = apply_packet_budget_with_extra(
            packet_fixture_project_root(),
            question,
            plan.task_class,
            PacketBudgetModeDto::Compact,
            limits,
            &mut answer,
            &extra_probes,
        );
        let sufficiency = build_packet_sufficiency_with_extra(
            packet_fixture_project_root(),
            question,
            plan.task_class,
            &answer,
            &budget,
            &extra_probes,
        );

        assert_eq!(
            sufficiency.status,
            PacketSufficiencyStatusDto::Partial,
            "{sufficiency:?}"
        );
        assert!(
            sufficiency
                .gaps
                .iter()
                .any(|gap| gap.contains("submit prevent default")
                    && !gap.contains("pattern")
                    && !gap.contains("validity state")),
            "only selected planned probes for still-missing roles should become sufficiency gaps: {sufficiency:?}"
        );
    }

    #[test]
    fn packet_sufficiency_keeps_nonblocking_unresolved_sidecar_candidates_diagnostic() {
        let question = "Explain how packet retrieval flows through sidecar diagnostics.";
        let (mut answer, initial_sufficiency) = build_sufficient_packet_fixture(
            question,
            PacketTaskClassDto::EditPlanning,
            vec![
                test_packet_citation("PacketPlanner", "src/packet_plan.rs", 0.9),
                test_packet_citation("RuntimeCoordinator", "src/runtime.rs", 0.8),
                test_packet_citation("ProjectionStore", "src/store.rs", 0.7),
            ],
        );
        assert_eq!(
            initial_sufficiency.status,
            PacketSufficiencyStatusDto::Sufficient
        );
        answer
            .retrieval_trace
            .packet_sidecar_diagnostics
            .push(PacketSidecarQueryDiagnosticDto {
                query: "sidecar batch".to_string(),
                retrieval_mode: "full".to_string(),
                sidecar_query_ms: None,
                candidate_resolution_ms: None,
                total_elapsed_ms: None,
                sidecar_stage_count: 0,
                sidecar_stage_total_ms: None,
                batch_query_wall_ms: None,
                candidate_count: 1,
                resolved_hit_count: 0,
                unresolved_candidate_count: 1,
                diagnostic: Some(
                    "sidecar candidates did not all resolve to indexed symbols".to_string(),
                ),
            });
        answer
            .retrieval_trace
            .packet_sidecar_diagnostics
            .push(PacketSidecarQueryDiagnosticDto {
                query: "sidecar batch".to_string(),
                retrieval_mode: "full".to_string(),
                sidecar_query_ms: None,
                candidate_resolution_ms: None,
                total_elapsed_ms: None,
                sidecar_stage_count: 0,
                sidecar_stage_total_ms: None,
                batch_query_wall_ms: None,
                candidate_count: 1,
                resolved_hit_count: 0,
                unresolved_candidate_count: 1,
                diagnostic: Some(
                    "sidecar candidates did not all resolve to indexed symbols".to_string(),
                ),
            });

        let budget = PacketBudgetDto {
            requested: PacketBudgetModeDto::Compact,
            limits: packet_budget_limits(PacketBudgetModeDto::Compact),
            used: PacketBudgetUsageDto {
                anchors: 3,
                files: 0,
                snippets: 0,
                trail_edges: 0,
                output_bytes: 0,
            },
            truncated: false,
            omitted_sections: Vec::new(),
            next_deeper_command: None,
        };
        let sufficiency = build_packet_sufficiency(
            packet_fixture_project_root(),
            question,
            PacketTaskClassDto::EditPlanning,
            &answer,
            &budget,
        );

        assert_eq!(sufficiency.status, PacketSufficiencyStatusDto::Sufficient);
        assert!(
            !sufficiency
                .gaps
                .iter()
                .any(|gap| gap.contains("sidecar candidates")),
            "nonblocking sidecar diagnostics should not become sufficiency gaps: {:?}",
            sufficiency.gaps
        );
        assert_eq!(
            sufficiency
                .coverage_report
                .as_ref()
                .map(|report| report.unresolved.as_slice()),
            Some(&["sidecar batch".to_string()][..]),
            "duplicate diagnostics should remain visible once in coverage_report.unresolved: {sufficiency:?}"
        );
    }

    #[test]
    fn packet_sufficiency_accepts_required_flow_probe_coverage() {
        let (_answer, sufficiency) = build_sufficient_packet_fixture(
            "Explain how `codex exec --json` flows from the top-level CLI into the exec runtime, app-server thread and turn start requests, and JSONL event output.",
            PacketTaskClassDto::ArchitectureExplanation,
            vec![
                test_packet_citation("run_exec_session", "codex-rs/exec/src/lib.rs", 0.9),
                test_packet_citation("RuntimeCoordinator", "codex-rs/exec/src/lib.rs", 0.9),
                test_packet_citation("main", "codex-rs/cli/src/main.rs", 0.8),
                test_packet_citation("exec command", "codex-rs/cli/src/main.rs", 0.8),
                test_packet_citation(
                    "EventProcessorWithJsonOutput",
                    "codex-rs/exec/src/event_processor_with_jsonl_output.rs",
                    0.8,
                ),
                test_packet_citation("JsonlEventOutput", "codex-rs/exec/src/exec_events.rs", 0.8),
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
    fn packet_sufficiency_treats_concrete_file_probe_as_hint_when_roles_are_covered() {
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

        assert_eq!(sufficiency.status, PacketSufficiencyStatusDto::Sufficient);
        assert!(
            sufficiency
                .gaps
                .iter()
                .all(|gap| !gap.contains("exec_events")),
            "concrete file probes should be nonblocking hints when required flow roles are covered: {sufficiency:?}"
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
                r"\\?\C:\Users\alber\source\repos\codestory\target\repo-cache\repos\ripgrep\crates\core\main.rs"
            ),
            "crates/core/main.rs"
        );
        assert_eq!(
            packet_display_path("target/repo-cache/repos/axios/lib/core/Axios.js"),
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
    fn packet_budget_protects_generic_indexing_flow_probe_citations() {
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
                "indexing entrypoint",
                "crates/codestory-runtime/src/services.rs",
                0.1,
            ),
            test_packet_citation(
                "file discovery",
                "crates/codestory-workspace/src/lib.rs",
                0.1,
            ),
            test_packet_citation(
                "symbol extraction",
                "crates/codestory-indexer/src/lib.rs",
                0.1,
            ),
            test_packet_citation(
                "storage persistence",
                "crates/codestory-store/src/storage_impl/mod.rs",
                0.1,
            ),
            test_packet_citation(
                "search projection",
                "crates/codestory-store/src/storage_impl/mod.rs",
                0.1,
            ),
            test_packet_citation(
                "snapshot refresh",
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
            "indexing entrypoint",
            "file discovery",
            "symbol extraction",
            "storage persistence",
            "search projection",
            "snapshot refresh",
        ] {
            assert!(
                display_names.contains(&expected),
                "compact packet cap should protect generic indexing-flow probe {expected}: {display_names:?}"
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
                    "flag parsing",
                    "argument planning",
                    "candidate file walk",
                    "search execution",
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
                "walk builder",
                "matcher searcher printer",
                "search worker",
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

        for expected in [
            "server bootstrap",
            "command server entrypoint",
            "event loop source",
            "network command input",
            "command table dispatch",
            "event loop",
            "network input",
            "command dispatch",
        ] {
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

        let dispatch_only_question =
            "Trace how network command input reaches command table dispatch and command handlers.";
        let dispatch_only_plan = build_packet_plan(
            dispatch_only_question,
            Some(PacketTaskClassDto::ArchitectureExplanation),
            PacketBudgetModeDto::Compact,
        );
        let dispatch_only_queries = dispatch_only_plan
            .queries
            .iter()
            .map(|query| query.query.as_str())
            .collect::<Vec<_>>();
        for expected in ["network command input", "command table dispatch"] {
            assert!(
                dispatch_only_queries.contains(&expected),
                "dispatch-only command plan should include {expected}: {dispatch_only_queries:?}"
            );
        }
        for unexpected in [
            "server bootstrap",
            "command server entrypoint",
            "event loop source",
            "event loop",
            "event dispatch",
        ] {
            assert!(
                !dispatch_only_queries.contains(&unexpected),
                "dispatch-only command plan should not require {unexpected}: {dispatch_only_queries:?}"
            );
        }
        let dispatch_only_required = packet_sufficiency_required_probe_queries(
            dispatch_only_question,
            PacketTaskClassDto::ArchitectureExplanation,
        );
        for expected in ["network command input", "command table dispatch"] {
            assert!(
                dispatch_only_required.iter().any(|query| query == expected),
                "dispatch-only command sufficiency should include {expected}: {dispatch_only_required:?}"
            );
        }
        for unexpected in [
            "server bootstrap",
            "command server entrypoint",
            "event loop source",
        ] {
            assert!(
                !dispatch_only_required
                    .iter()
                    .any(|query| query == unexpected),
                "dispatch-only command sufficiency should not require {unexpected}: {dispatch_only_required:?}"
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
    fn compact_packet_plan_protects_generic_indexing_flow_probes() {
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
            "indexing entrypoint",
            "file discovery",
            "symbol extraction",
            "storage persistence",
            "search projection",
            "snapshot refresh",
        ] {
            assert!(
                queries.contains(&expected),
                "expected generic indexing-flow probe {expected} in compact packet plan: {queries:?}"
            );
        }
        for fixture_anchor in [
            "Runtime::index_service",
            "IndexService::run_indexing_blocking",
            "index service run indexing",
            "WorkspaceManifest::build_execution_plan",
            "workspace manifest build execution plan",
            "WorkspaceIndexer::run",
            "workspace indexer run",
            "index_file",
            "Storage::flush_projection_batch",
            "storage flush projection batch",
            "Storage::rebuild_search_symbol_projection_from_node_table",
            "storage rebuild search symbol projection",
            "SnapshotStore::refresh_all_with_stats",
            "snapshot refresh all stats",
        ] {
            assert!(
                !queries.contains(&fixture_anchor),
                "packet planner should protect generic indexing probes without injecting fixture-specific anchor {fixture_anchor}: {queries:?}"
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
        let _eval_probes = EvalProbesGuard::enabled();
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
                packet_sidecar_diagnostics: Vec::new(),
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
            "The command or public entrypoint for this flow is `CliCommand`",
            "`RuntimeCoordinator` coordinates runtime state transitions",
            "`WorkspacePlan` handles workspace file selection",
            "`GraphIndexer` extracts nodes, edges, occurrences",
            "`ProjectionStore` persists or projects durable graph/search state",
            "`SnapshotRefresh` refreshes post-write summaries",
            "`RouteHandler` handles route dispatch or handler ownership",
        ] {
            assert!(
                text.contains(expected_claim),
                "generic packet claims should include {expected_claim}: {text}"
            );
        }
        assert!(
            !text.contains("`PacketRegression` covers regression behavior"),
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
                packet_sidecar_diagnostics: Vec::new(),
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
            "Runtime session entrypoint evidence loads config, resolves sandbox and approval settings, and builds app-server start arguments"
        ));
        assert!(
            text.contains("The command or public entrypoint for this flow is `codex_exec::Cli`")
        );
        assert!(text.contains("`codex_exec::run_main` coordinates runtime state transitions"));
        assert!(text.contains("`EventProcessorWithJsonOutput` serializes typed runtime events"));
        assert!(text.contains("`ThreadStartParams` defines app-server thread or turn start"));
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
                packet_sidecar_diagnostics: Vec::new(),
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
                packet_sidecar_diagnostics: Vec::new(),
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
        assert!(text.contains("`Project::buildIndex` turns build-index commands"));
        assert!(text.contains("`SourceGroupCxxCdb` maps project settings"));
        assert!(text.contains("`StorageAccess` persists or projects durable graph/search state"));
        assert!(
            text.contains("`PersistentStorage` persists or projects durable graph/search state")
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
            "Indexing entrypoint evidence delegates indexing work into the runtime orchestration layer.",
            "Runtime orchestration evidence opens workspace/store state and coordinates refresh phases.",
            "Workspace discovery evidence plans source-file discovery and refresh work.",
            "Symbol extraction evidence builds graph nodes, edges, occurrences, and related source data.",
            "Persistence evidence stores graph/file data and rebuilds query/search projections.",
            "Snapshot refresh evidence updates read models after persisted graph changes.",
        ] {
            assert!(
                text.contains(expected),
                "indexing pipeline packet claims should include `{expected}`: {text}"
            );
        }
    }

    #[test]
    fn packet_sufficiency_accepts_generic_indexing_flow_probes() {
        let question = "Explain how a full indexing run moves from the CLI into runtime orchestration, file discovery, symbol extraction, persistence, and search or snapshot refresh.";
        let (_answer, sufficiency) = build_sufficient_packet_fixture(
            question,
            PacketTaskClassDto::ArchitectureExplanation,
            vec![
                test_packet_citation("CliDirection", "crates/codestory-cli/src/args.rs", 0.8),
                test_packet_citation(
                    "indexing entrypoint",
                    "crates/codestory-runtime/src/services.rs",
                    0.8,
                ),
                test_packet_citation(
                    "file discovery",
                    "crates/codestory-workspace/src/lib.rs",
                    0.8,
                ),
                test_packet_citation(
                    "symbol extraction",
                    "crates/codestory-indexer/src/lib.rs",
                    0.8,
                ),
                test_packet_citation(
                    "storage persistence",
                    "crates/codestory-store/src/storage_impl/mod.rs",
                    0.8,
                ),
                test_packet_citation(
                    "search projection",
                    "crates/codestory-store/src/storage_impl/mod.rs",
                    0.8,
                ),
                test_packet_citation(
                    "snapshot refresh",
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
        for probe in [
            "indexing entrypoint",
            "file discovery",
            "symbol extraction",
            "storage persistence",
            "search projection",
            "snapshot refresh",
        ] {
            assert!(
                sufficiency.gaps.iter().all(|gap| !gap.contains(probe)),
                "generic indexing-flow probe {probe} should satisfy required probe gaps: {sufficiency:?}"
            );
            assert!(
                sufficiency
                    .follow_up_commands
                    .iter()
                    .all(|command| !command.contains(probe)),
                "generic indexing-flow probe {probe} should not produce follow-up commands: {sufficiency:?}"
            );
        }
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
                packet_sidecar_diagnostics: Vec::new(),
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
        assert!(text.contains("`Project::buildIndex` turns build-index commands"));
        assert!(text.contains("`StorageAccess` persists or projects durable graph/search state"));
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
                packet_sidecar_diagnostics: Vec::new(),
                retrieval_shadow: None,
            },
        };

        let claims = packet_supported_claims(&answer);
        let text = claims
            .iter()
            .map(|claim| claim.claim.as_str())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(text.contains("`ExtensionService` coordinates runtime state transitions"));
        assert!(
            text.contains("The command or public entrypoint for this flow is `ExtHostCommands`")
        );
        assert!(
            text.contains("ties") || text.contains("coordinates runtime state transitions"),
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
                packet_sidecar_diagnostics: Vec::new(),
                retrieval_shadow: None,
            },
        };

        let claims = packet_supported_claims(&answer);
        let text = claims
            .iter()
            .map(|claim| claim.claim.as_str())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(text.contains("`Posts` defines collection schema fields"));
        assert!(
            text.contains(
                "`POST /posts/:slug/comments` handles route dispatch or handler ownership"
            )
        );
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
                packet_sidecar_diagnostics: Vec::new(),
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
                packet_sidecar_diagnostics: Vec::new(),
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
                packet_sidecar_diagnostics: Vec::new(),
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
            Some(PacketEvidenceRole::TestsAndRegressionCoverage)
        );
        assert_eq!(
            packet_evidence_role(&answer.citations[2]),
            Some(PacketEvidenceRole::TestsAndRegressionCoverage)
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
                packet_sidecar_diagnostics: Vec::new(),
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
                packet_sidecar_diagnostics: Vec::new(),
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
                "`RuntimeCoordinator` coordinates runtime state transitions",
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
                "`RuntimeErrorHandler` coordinates runtime state transitions",
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
                "`AffectedReferenceIndex` extracts nodes, edges, occurrences",
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
                "`RouteHandler` handles route dispatch or handler ownership",
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
                "`WorkspaceOwnerPlan` handles workspace file selection",
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
                "`ConfigRegression` covers regression behavior",
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
            assert!(
                sufficiency
                    .avoid_opening_paths
                    .iter()
                    .any(|entry| entry == avoid_path),
                "sufficient {task_class:?} packet should expose raw avoid-opening path `{avoid_path}`: {sufficiency:?}"
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
    fn architecture_sufficiency_does_not_invent_flow_roles_without_requirements() {
        let question = "Explain the module relationships.";
        let citations = vec![
            test_packet_citation("Alpha", "src/alpha.rs", 0.9),
            test_packet_citation("Beta", "src/beta.rs", 0.85),
            test_packet_citation("Gamma", "src/gamma.rs", 0.8),
        ];
        let (_answer, sufficiency) = build_sufficient_packet_fixture(
            question,
            PacketTaskClassDto::ArchitectureExplanation,
            citations,
        );

        assert_eq!(sufficiency.status, PacketSufficiencyStatusDto::Partial);
        assert!(
            sufficiency
                .gaps
                .iter()
                .all(|gap| !gap.contains("flow-role coverage")),
            "architecture sufficiency should only report flow-role gaps from shared requirements: {sufficiency:?}"
        );
    }

    #[test]
    fn generic_navigation_claims_do_not_satisfy_packet_sufficiency() {
        let generic = PacketClaimDto {
            claim: "Runtime orchestration is anchored by `RuntimeCoordinator`; inspect it there."
                .to_string(),
            citations: vec![test_packet_citation(
                "RuntimeCoordinator",
                "src/runtime.rs",
                0.9,
            )],
            coverage_role: None,
            eligible_for_sufficiency: None,
        };
        let causal = PacketClaimDto {
            claim: "`RuntimeCoordinator` coordinates runtime state transitions and downstream service calls."
                .to_string(),
            citations: vec![test_packet_citation(
                "RuntimeCoordinator",
                "src/runtime.rs",
                0.9,
            )],
            coverage_role: None,
            eligible_for_sufficiency: None,
        };
        let adjacent = PacketClaimDto {
            claim:
                "`Session.send` in `src/requests/sessions.py` ties request, session in this flow to cited definitions and adjacent ownership."
                    .to_string(),
            citations: vec![test_packet_citation(
                "Session.send",
                "src/requests/sessions.py",
                0.9,
            )],
            coverage_role: None,
            eligible_for_sufficiency: None,
        };
        let definition = PacketClaimDto {
            claim:
                "`PreparedRequest` is defined in cited source `src/requests/models.py` and should be treated as an exact source anchor for this flow."
                    .to_string(),
            citations: vec![test_packet_citation(
                "PreparedRequest",
                "src/requests/models.py",
                0.9,
            )],
            coverage_role: None,
            eligible_for_sufficiency: None,
        };

        assert!(!packet_claim_can_satisfy_sufficiency(&generic));
        assert!(!packet_claim_can_satisfy_sufficiency(&adjacent));
        assert!(!packet_claim_can_satisfy_sufficiency(&definition));
        assert!(packet_claim_can_satisfy_sufficiency(&causal));
    }

    #[test]
    fn claim_family_coverage_uses_covered_claim_semantics() {
        let claims = vec![
            PacketClaimDto {
                claim: "The public useSWR export wraps useSWRHandler with argument normalization."
                    .to_string(),
                citations: vec![test_packet_citation(
                    "useSWRHandler",
                    "src/index/use-swr.ts",
                    0.9,
                )],
                coverage_role: None,
                eligible_for_sufficiency: None,
            },
            PacketClaimDto {
                claim: "useSWRHandler serializes the key before reading cache state.".to_string(),
                citations: vec![test_packet_citation(
                    "serialize",
                    "src/_internal/utils/serialize.ts",
                    0.9,
                )],
                coverage_role: None,
                eligible_for_sufficiency: None,
            },
            PacketClaimDto {
                claim:
                    "createCacheHelper provides cache get, set, subscribe, and snapshot helpers."
                        .to_string(),
                citations: vec![test_packet_citation(
                    "createCacheHelper",
                    "src/_internal/utils/helper.ts",
                    0.9,
                )],
                coverage_role: None,
                eligible_for_sufficiency: None,
            },
            PacketClaimDto {
                claim: "internalMutate routes mutate behavior through the mutation helper."
                    .to_string(),
                citations: vec![test_packet_citation(
                    "internalMutate",
                    "src/_internal/utils/mutate.ts",
                    0.9,
                )],
                coverage_role: None,
                eligible_for_sufficiency: None,
            },
        ];

        let use_swr_handler = &claims[0].citations[0];
        assert_eq!(
            packet_evidence_role(use_swr_handler),
            Some(PacketEvidenceRole::SourceEvidence),
            "a hook handler outside route-shaped paths should not become route handling"
        );

        let families = claims
            .iter()
            .filter_map(packet_claim_family)
            .collect::<HashSet<_>>();

        for expected in [
            "public api/export",
            "key serialization",
            "cache state",
            "mutation flow",
        ] {
            assert!(
                families.contains(expected),
                "claim families should include `{expected}` from accepted covered-claim text: {families:?}"
            );
        }
        assert_eq!(packet_supported_claim_family_count(&claims), 4);
    }

    #[test]
    fn claim_family_coverage_recognizes_predicate_behavior() {
        let claims = vec![
            PacketClaimDto {
                claim:
                    "StringUtils.isBlank treats null, empty, and whitespace-only inputs as blank."
                        .to_string(),
                citations: vec![test_packet_citation(
                    "StringUtils.isBlank",
                    "src/main/java/org/apache/commons/lang3/StringUtils.java",
                    0.9,
                )],
                coverage_role: None,
                eligible_for_sufficiency: None,
            },
            PacketClaimDto {
                claim: "StringUtils.isEmpty does not trim whitespace before deciding emptiness."
                    .to_string(),
                citations: vec![test_packet_citation(
                    "StringUtils.isEmpty",
                    "src/main/java/org/apache/commons/lang3/StringUtils.java",
                    0.9,
                )],
                coverage_role: None,
                eligible_for_sufficiency: None,
            },
            PacketClaimDto {
                claim: "Strings delegates region matching work to CharSequenceUtils.regionMatches."
                    .to_string(),
                citations: vec![test_packet_citation(
                    "Strings.regionMatches",
                    "src/main/java/org/apache/commons/lang3/Strings.java",
                    0.9,
                )],
                coverage_role: None,
                eligible_for_sufficiency: None,
            },
        ];

        assert_eq!(
            packet_claim_family(&claims[0]),
            Some("predicate blank behavior")
        );
        assert_eq!(
            packet_claim_family(&claims[1]),
            Some("predicate empty behavior")
        );
        assert_eq!(
            packet_claim_family(&claims[2]),
            Some("predicate region/case flow")
        );
        assert_eq!(packet_supported_claim_family_count(&claims), 3);
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
            6
        );
        assert_eq!(
            packet_anchor_probe_limit_for_budget(PacketBudgetModeDto::Compact, budget, 8_000),
            3
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
    fn packet_non_trace_phase_annotation_is_machine_readable() {
        assert_eq!(
            packet_non_trace_phase_annotation("budget", 42),
            "packet_non_trace_phase label=budget duration_ms=42"
        );
        assert_eq!(
            packet_non_trace_phase_annotation("pre_rank_citations", 7),
            "packet_non_trace_phase label=pre_rank_citations duration_ms=7"
        );
        assert_eq!(
            packet_non_trace_phase_annotation("trace_apply", 3),
            "packet_non_trace_phase label=trace_apply duration_ms=3"
        );
    }

    #[test]
    fn packet_retrieval_trace_summary_keeps_counters_without_duplicating_full_trace() {
        let mut answer = packet_answer_fixture(
            "Explain the packet retrieval trace summary.",
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
        let retrieval_trace_summary = trace_export::packet_retrieval_trace_summary(&answer);
        let retrieval_trace_summary_bytes =
            serde_json::to_vec(&retrieval_trace_summary.retrieval_trace)
                .expect("serialize retrieval trace summary")
                .len();

        assert_eq!(answer.retrieval_trace.steps.len(), 3);
        assert_eq!(retrieval_trace_summary.search_steps, 1);
        assert_eq!(retrieval_trace_summary.trail_steps, 1);
        assert_eq!(retrieval_trace_summary.source_read_steps, 1);
        assert_eq!(retrieval_trace_summary.retrieval_trace.total_latency_ms, 42);
        assert_eq!(
            retrieval_trace_summary.retrieval_trace.sla_target_ms,
            Some(1_000)
        );
        assert!(retrieval_trace_summary.retrieval_trace.sla_missed);
        assert!(retrieval_trace_summary.retrieval_trace.steps.is_empty());
        assert!(
            retrieval_trace_summary
                .retrieval_trace
                .annotations
                .is_empty()
        );
        assert!(
            retrieval_trace_summary_bytes < full_trace_bytes / 2,
            "retrieval trace summary should stay scalar-sized: {retrieval_trace_summary_bytes} >= {full_trace_bytes}/2"
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
    fn markdown_budget_skips_tiny_diagram_intro_and_truncates_verbose_sections_first() {
        let question = "Explain compact packet proof retention.";
        let mut answer = packet_answer_fixture(
            question,
            vec![
                test_packet_citation("CliCommand", "crates/tool-cli/src/main.rs", 0.8),
                test_packet_citation("RuntimeCoordinator", "crates/core/src/runtime.rs", 0.8),
                test_packet_citation("WorkspacePlan", "crates/core/src/workspace/plan.rs", 0.8),
            ],
        );
        answer.sections = vec![
            AgentResponseSectionDto {
                id: "packet-evidence-ledger".to_string(),
                title: "Packet Evidence Ledger".to_string(),
                blocks: vec![AgentResponseBlockDto::Markdown {
                    markdown: "proof citation ledger\n".repeat(250),
                }],
            },
            AgentResponseSectionDto {
                id: "packet-flow-claims".to_string(),
                title: "Packet Claims".to_string(),
                blocks: vec![AgentResponseBlockDto::Markdown {
                    markdown: "covered proof claim\n".repeat(250),
                }],
            },
            AgentResponseSectionDto {
                id: "retrieval-evidence".to_string(),
                title: "Retrieval Evidence".to_string(),
                blocks: vec![AgentResponseBlockDto::Markdown {
                    markdown: "repeated snippet and retrieval appendix\n".repeat(1_200),
                }],
            },
            AgentResponseSectionDto {
                id: "diagrams".to_string(),
                title: "Diagrams".to_string(),
                blocks: vec![
                    AgentResponseBlockDto::Markdown {
                        markdown: "Mermaid diagrams generated from indexed graph retrieval."
                            .to_string(),
                    },
                    AgentResponseBlockDto::Mermaid {
                        graph_id: "primary".to_string(),
                    },
                ],
            },
        ];

        let original_proof_sections = answer.sections[0..2]
            .iter()
            .map(|section| match &section.blocks[0] {
                AgentResponseBlockDto::Markdown { markdown } => markdown.clone(),
                AgentResponseBlockDto::Mermaid { .. } => String::new(),
            })
            .collect::<Vec<_>>();
        let original_diagram_intro = match &answer.sections[3].blocks[0] {
            AgentResponseBlockDto::Markdown { markdown } => markdown.clone(),
            AgentResponseBlockDto::Mermaid { .. } => String::new(),
        };
        let original_bytes = serde_json::to_vec(&answer).unwrap().len();
        let truncated = truncate_answer_markdown_to_byte_cap(&mut answer, original_bytes - 6_000);

        assert!(truncated);
        for (section, original_markdown) in
            answer.sections[0..2].iter().zip(original_proof_sections)
        {
            let AgentResponseBlockDto::Markdown { markdown } = &section.blocks[0] else {
                panic!("proof section should remain markdown");
            };
            assert_eq!(
                markdown, &original_markdown,
                "proof-bearing section `{}` should not be truncated before verbose sections",
                section.id
            );
        }
        let AgentResponseBlockDto::Markdown {
            markdown: retrieval_markdown,
        } = &answer.sections[2].blocks[0]
        else {
            panic!("retrieval evidence should remain markdown");
        };
        assert!(
            retrieval_markdown.contains(PACKET_MARKDOWN_TRUNCATION_SUFFIX.trim()),
            "large retrieval evidence should absorb truncation before proof sections"
        );
        let AgentResponseBlockDto::Markdown {
            markdown: diagram_intro,
        } = &answer.sections[3].blocks[0]
        else {
            panic!("diagram intro should remain markdown");
        };
        assert_eq!(
            diagram_intro, &original_diagram_intro,
            "tiny diagram intro should be skipped instead of aborting truncation"
        );
    }

    #[test]
    fn hard_payload_budget_truncation_requires_deeper_packet() {
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
        budget.omitted_sections = vec!["packet_payload".to_string()];
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
            "hard payload truncation should be named as a sufficiency gap: {sufficiency:?}"
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
                test_packet_citation("ContentStore", "src/lib/content-data/content-store.ts", 0.9),
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
    fn retained_truncated_trail_edges_can_remain_sufficient() {
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
                test_packet_citation("ContentStore", "src/lib/content-data/content-store.ts", 0.9),
                test_packet_citation("GET /feed.xml", "src/app/feed.xml/route.ts", 0.9),
            ],
        );
        answer.graphs.push(GraphArtifactDto::Uml {
            id: "primary".to_string(),
            title: "Primary Neighborhood".to_string(),
            graph: GraphResponse {
                center_id: NodeId("session".to_string()),
                nodes: vec![node("api"), node("session"), node("adapter")],
                edges: vec![
                    edge("edge_1", "api", "session"),
                    edge("edge_2", "session", "adapter"),
                ],
                truncated: true,
                omitted_edge_count: 12,
                canonical_layout: None,
            },
        });

        let budget = PacketBudgetDto {
            requested: PacketBudgetModeDto::Compact,
            limits: packet_budget_limits(PacketBudgetModeDto::Compact),
            used: packet_budget_usage(&answer),
            truncated: true,
            omitted_sections: vec!["citations".to_string(), "trail_edges".to_string()],
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

        assert_eq!(
            sufficiency.status,
            PacketSufficiencyStatusDto::Sufficient,
            "trail clipping should not force deeper packets when graph edges, citations, and claims remain: {sufficiency:?}"
        );
        assert!(sufficiency.gaps.is_empty());
        assert!(sufficiency.follow_up_commands.is_empty());
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
        append_packet_step_trace_annotation(&mut answer);
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
        let retrieval_trace_summary = trace_export::packet_retrieval_trace_summary(&answer);
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
            retrieval_trace_summary,
        };

        enforce_packet_output_budget(packet_fixture_project_root(), &mut packet);

        let serialized_len = serde_json::to_vec(&packet).expect("serialize packet").len();
        assert!(
            serialized_len <= max_output_bytes as usize,
            "serialized packet should honor max_output_bytes: {serialized_len} > {}",
            max_output_bytes
        );
        assert_eq!(packet.budget.used.output_bytes as usize, serialized_len);
        append_packet_non_trace_phase(&mut packet.answer, "output_budget", Instant::now());
        enforce_packet_output_budget(packet_fixture_project_root(), &mut packet);
        let serialized_len = serde_json::to_vec(&packet)
            .expect("serialize packet after output budget marker")
            .len();
        assert_eq!(
            packet.budget.used.output_bytes as usize, serialized_len,
            "final diagnostic marker must be included in packet output accounting"
        );
        assert!(
            packet
                .answer
                .retrieval_trace
                .annotations
                .iter()
                .any(|annotation| annotation.starts_with("packet_step_trace ")),
            "packet step trace annotation should be present before final budget measurement"
        );
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
                packet_sidecar_diagnostics: Vec::new(),
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
                .contains("`RuntimeCoordinator` coordinates runtime state transitions")),
            "generic packet should include claim-led runtime flow notes: {sufficiency:?}"
        );
        assert!(
            sufficiency
                .avoid_opening
                .iter()
                .any(|path| path.contains("crates/app-cli/src/main.rs")),
            "sufficient packets should tell agents cited files do not need broad re-opening: {sufficiency:?}"
        );
        assert!(
            sufficiency
                .avoid_opening_paths
                .iter()
                .any(|path| path == "crates/app-cli/src/main.rs"),
            "sufficient packets should expose raw cited paths separately from prose: {sufficiency:?}"
        );
    }

    #[test]
    fn packet_plan_adds_prepared_session_adapter_exact_probes() {
        let _eval_probes = EvalProbesGuard::enabled();
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
    fn packet_plan_keeps_requests_and_express_exact_probes_eval_only() {
        let _env = EnvVarGuard::cleared(EVAL_PROBES_ENV);
        let requests_question = "Explain how Requests turns a top-level request call into a prepared request and sends it through a session adapter.";
        let requests_plan = build_packet_plan(
            requests_question,
            Some(PacketTaskClassDto::ArchitectureExplanation),
            PacketBudgetModeDto::Compact,
        );
        let requests_queries = requests_plan
            .queries
            .iter()
            .map(|query| query.query.as_str())
            .collect::<Vec<_>>();
        let requests_required = packet_sufficiency_required_probe_queries(
            requests_question,
            PacketTaskClassDto::ArchitectureExplanation,
        );

        for generic_probe in [
            "request preparation",
            "session request",
            "session send",
            "adapter send",
            "adapter selection",
        ] {
            assert!(
                requests_queries.contains(&generic_probe)
                    || requests_required.iter().any(|query| query == generic_probe),
                "production plan should keep generic request/session probe `{generic_probe}`; queries={requests_queries:?} required={requests_required:?}"
            );
        }
        for eval_only_probe in [
            "Session.request",
            "Session.prepare_request",
            "PreparedRequest.prepare",
            "Session.send",
            "HTTPAdapter.send",
        ] {
            assert!(
                !requests_queries.contains(&eval_only_probe)
                    && !requests_required
                        .iter()
                        .any(|query| query == eval_only_probe),
                "production plan should not add exact Requests probe `{eval_only_probe}`; queries={requests_queries:?} required={requests_required:?}"
            );
        }

        let express_question = "Trace how Express creates an application, registers middleware/routes, and handles an incoming request through the router and response helpers.";
        let express_plan = build_packet_plan(
            express_question,
            Some(PacketTaskClassDto::RouteTracing),
            PacketBudgetModeDto::Compact,
        );
        let express_queries = express_plan
            .queries
            .iter()
            .map(|query| query.query.as_str())
            .collect::<Vec<_>>();
        let express_required = packet_sufficiency_required_probe_queries(
            express_question,
            PacketTaskClassDto::RouteTracing,
        );
        let express_sufficiency_extra = packet_plan_sufficiency_extra_probes(&express_plan, &[]);

        for generic_probe in [
            "app initialization",
            "middleware registration",
            "request handler",
            "response send",
        ] {
            assert!(
                express_queries.contains(&generic_probe),
                "production plan should include JS route source probe `{generic_probe}` in {express_queries:?}"
            );
            assert!(
                express_sufficiency_extra
                    .iter()
                    .any(|query| query == generic_probe),
                "production plan should protect JS route source probe `{generic_probe}` during citation capping: {express_sufficiency_extra:?}"
            );
        }

        for eval_only_probe in [
            "createApplication",
            "app.init",
            "app.handle",
            "app.use",
            "app.route",
            "res.send",
            "application.js app.use",
        ] {
            assert!(
                !express_queries.contains(&eval_only_probe)
                    && !express_required
                        .iter()
                        .any(|query| query == eval_only_probe),
                "production plan should not add exact Express probe `{eval_only_probe}`; queries={express_queries:?} required={express_required:?}"
            );
        }
    }

    #[test]
    fn packet_plan_derives_java_string_check_symbol_probes() {
        let _eval_probes = EvalProbesGuard::enabled();
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
    fn packet_plan_keeps_literal_symbols_without_eval_family_expansion() {
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

        for literal_symbol in ["StringUtils", "Strings", "CharSequenceUtils"] {
            assert!(
                queries.contains(&literal_symbol),
                "production packet plan should keep literal prompt symbol `{literal_symbol}` in {queries:?}"
            );
        }
        for source_probe in [
            "StringUtils.java isBlank",
            "StringUtils.java isEmpty",
            "Strings.java regionMatches",
            "CharSequenceUtils.java regionMatches",
        ] {
            assert!(
                queries.contains(&source_probe),
                "production packet plan should derive Java source-scoped predicate probe `{source_probe}` in {queries:?}"
            );
        }
        for eval_only_probe in [
            "StringUtils.isBlank",
            "StringUtils.isEmpty",
            "StringUtils.java",
            "Strings.java",
            "CharSequenceUtils.java",
        ] {
            assert!(
                !queries.contains(&eval_only_probe),
                "production packet plan should not add eval-only family probe `{eval_only_probe}` in {queries:?}"
            );
        }
    }

    #[test]
    fn packet_plan_derives_generic_predicate_method_probes_in_production() {
        let _env = EnvVarGuard::cleared(EVAL_PROBES_ENV);
        let question = "Explain how text checks distinguish blank, empty, and case sensitive inputs. Cite the source files and name the supporting symbols.";
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

        for generic_probe in [
            "isBlank",
            "is_empty",
            "isCaseSensitive",
            "is_case_sensitive",
        ] {
            assert!(
                queries.contains(&generic_probe),
                "production packet plan should include generic predicate probe `{generic_probe}` in {queries:?}"
            );
        }

        for eval_only_probe in ["StringUtils.isBlank", "StringUtils.isEmpty"] {
            assert!(
                !queries.contains(&eval_only_probe),
                "production packet plan should not add benchmark-shaped predicate probe `{eval_only_probe}` in {queries:?}"
            );
        }
    }

    #[test]
    fn packet_plan_derives_scoped_predicate_method_probes_in_production() {
        let _env = EnvVarGuard::cleared(EVAL_PROBES_ENV);
        let question = "Explain how TextChecks and CharSequenceHelpers implement blank, empty, and case sensitive text checks. Cite the source files and name the supporting symbols.";
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

        for scoped_probe in [
            "TextChecks isBlank",
            "TextChecks isEmpty",
            "TextChecks.java isBlank",
            "TextChecks.java isEmpty",
            "regionMatches",
            "CharSequenceHelpers regionMatches",
            "CharSequenceHelpers.java regionMatches",
        ] {
            assert!(
                queries.contains(&scoped_probe),
                "production packet plan should include scoped predicate probe `{scoped_probe}` in {queries:?}"
            );
        }

        for eval_only_probe in ["TextChecks.isBlank", "TextChecks.isEmpty"] {
            assert!(
                !queries.contains(&eval_only_probe),
                "production packet plan should not add dotted predicate probe `{eval_only_probe}` in {queries:?}"
            );
        }
    }

    #[test]
    fn packet_plan_derives_runtime_formatting_probes_in_production() {
        let _env = EnvVarGuard::cleared(EVAL_PROBES_ENV);
        let question = "Explain how a formatting library turns formatting arguments into type-erased format args and reaches vformat or format_to output paths. Cite the source files and name the supporting symbols.";
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
            "format argument store",
            "dynamic format argument collection",
            "format error type",
            "format source buffer append",
            "buffer append",
            "system source vformat",
            "output formatting function",
            "system error formatting",
            "format error code",
        ] {
            assert!(
                queries.contains(&expected),
                "production packet plan should include runtime formatting probe `{expected}` in {queries:?}"
            );
            assert!(
                required.iter().any(|query| query == expected),
                "packet required probes should protect runtime formatting probe `{expected}` in {required:?}"
            );
        }

        for eval_only_probe in [
            "include/fmt/base.h format_arg_store",
            "include/fmt/args.h dynamic_format_arg_store",
            "include/fmt/format.h format_error",
        ] {
            assert!(
                !queries.contains(&eval_only_probe)
                    && !required.iter().any(|query| query == eval_only_probe),
                "production packet plan should not add holdout-shaped formatting probe `{eval_only_probe}`; queries={queries:?} required={required:?}"
            );
        }
    }

    #[test]
    fn packet_plan_derives_log_record_handler_probes_in_production() {
        let _env = EnvVarGuard::cleared(EVAL_PROBES_ENV);
        let question = "Explain how a logger turns a log call into a LogRecord and passes it through handlers. Cite the source files and name the supporting symbols.";
        let plan = build_packet_plan(
            question,
            Some(PacketTaskClassDto::DataFlow),
            PacketBudgetModeDto::Compact,
        );
        let queries = plan
            .queries
            .iter()
            .map(|query| query.query.as_str())
            .collect::<Vec<_>>();
        let required =
            packet_sufficiency_required_probe_queries(question, PacketTaskClassDto::DataFlow);

        for expected in [
            "logger handler stack",
            "handler registration",
            "logger record creation",
            "log method record handoff",
            "record handler interface",
            "processing handler write boundary",
            "handler processing",
        ] {
            assert!(
                queries.contains(&expected),
                "production packet plan should include log-record handler probe `{expected}` in {queries:?}"
            );
            assert!(
                required.iter().any(|query| query == expected),
                "packet required probes should protect log-record handler probe `{expected}` in {required:?}"
            );
        }

        for eval_only_probe in [
            "src/Monolog/Logger.php Logger::addRecord",
            "src/Monolog/Handler/AbstractProcessingHandler.php AbstractProcessingHandler::handle",
        ] {
            assert!(
                !queries.contains(&eval_only_probe)
                    && !required.iter().any(|query| query == eval_only_probe),
                "production packet plan should not add holdout-shaped log probe `{eval_only_probe}`; queries={queries:?} required={required:?}"
            );
        }
    }

    #[test]
    fn packet_plan_derives_swr_hook_flow_symbol_probes() {
        let _eval_probes = EvalProbesGuard::enabled();
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
        let _eval_probes = EvalProbesGuard::enabled();
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
        let _eval_probes = EvalProbesGuard::enabled();
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
    fn packet_plan_derives_generic_css_animation_source_probes() {
        let question = "Explain how a stylesheet defines shared animation variables, base classes, and connects named animation classes to keyframes.";
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
            "animation custom property duration",
            "animation custom property delay",
            "animation base class",
            "animation stylesheet import",
            "named animation class",
            "named keyframes animation",
            "css animation variables",
            "css animation imports",
        ] {
            assert!(
                queries.contains(&expected),
                "packet plan should include generic CSS animation probe `{expected}` in {queries:?}"
            );
            assert!(
                required.iter().any(|query| query == expected),
                "packet required probes should protect generic CSS animation probe `{expected}` in {required:?}"
            );
        }
    }

    #[test]
    fn packet_plan_derives_automapper_map_flow_symbol_probes() {
        let _eval_probes = EvalProbesGuard::enabled();
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
    fn packet_plan_derives_generic_mapper_configuration_plan_probes() {
        let question = "Explain how mapper configuration and runtime mapper APIs cooperate to map source objects to destination objects through type map plans.";
        let plan = build_packet_plan(
            question,
            Some(PacketTaskClassDto::DataFlow),
            PacketBudgetModeDto::Compact,
        );
        let queries = plan
            .queries
            .iter()
            .map(|query| query.query.as_str())
            .collect::<Vec<_>>();
        let required =
            packet_sufficiency_required_probe_queries(question, PacketTaskClassDto::DataFlow);

        for expected in [
            "mapper public api",
            "mapping runtime entrypoint",
            "mapping configuration source",
            "type map source",
            "mapping lambda plan",
            "mapping plan builder",
            "type map plan",
            "mapping execution plan",
        ] {
            assert!(
                queries.contains(&expected),
                "packet plan should include generic mapper probe `{expected}` in {queries:?}"
            );
            assert!(
                required.iter().any(|query| query == expected),
                "packet required probes should protect generic mapper probe `{expected}` in {required:?}"
            );
        }
    }

    #[test]
    fn packet_plan_derives_generic_client_send_source_probes() {
        let question = "Explain how an HTTP package exposes top-level helpers, Client convenience methods, BaseRequest finalization, and IOClient send behavior.";
        let plan = build_packet_plan(
            question,
            Some(PacketTaskClassDto::DataFlow),
            PacketBudgetModeDto::Compact,
        );
        let queries = plan
            .queries
            .iter()
            .map(|query| query.query.as_str())
            .collect::<Vec<_>>();
        let required =
            packet_sufficiency_required_probe_queries(question, PacketTaskClassDto::DataFlow);

        for expected in [
            "http top level helper",
            "client convenience method",
            "client send implementation",
            "io transport client send",
            "response stream boundary",
            "top level helpers",
            "request finalization",
            "transport send",
        ] {
            assert!(
                queries.contains(&expected),
                "packet plan should include generic client-send probe `{expected}` in {queries:?}"
            );
            assert!(
                required.iter().any(|query| query == expected),
                "packet required probes should protect generic client-send probe `{expected}` in {required:?}"
            );
        }
    }

    #[test]
    fn packet_plan_derives_generic_form_validation_source_probes() {
        let question = "Explain how form validation examples combine native HTML constraints with custom JavaScript validation.";
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
            "html form required constraint",
            "html form pattern constraint",
            "html form min max constraints",
            "custom form validation input",
            "custom validation validity state",
            "custom validation error rendering",
            "native form constraints",
            "custom validation flow",
            "validity state",
            "submit prevent default",
        ] {
            assert!(
                queries.contains(&expected),
                "packet plan should include generic form-validation probe `{expected}` in {queries:?}"
            );
            assert!(
                required.iter().any(|query| query == expected),
                "packet required probes should protect generic form-validation probe `{expected}` in {required:?}"
            );
        }
    }

    #[test]
    fn packet_plan_derives_generic_url_session_request_source_probes() {
        let question = "Trace how a Session creates requests, resumes tasks, validates data requests, and receives URLSession callbacks.";
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
            "session request creation",
            "request object creation",
            "request resume dispatch",
            "request validation pipeline",
            "delegate callback handling",
            "url session callback boundary",
            "request task resume",
            "data request validation",
            "urlsession callbacks",
        ] {
            assert!(
                queries.contains(&expected),
                "packet plan should include generic URLSession request probe `{expected}` in {queries:?}"
            );
            assert!(
                required.iter().any(|query| query == expected),
                "packet required probes should protect generic URLSession request probe `{expected}` in {required:?}"
            );
        }

        for http_seed in ["router", "route handler endpoint", "middleware"] {
            assert!(
                !queries.contains(&http_seed),
                "URLSession route-tracing prompt should not include HTTP route seed `{http_seed}` in {queries:?}"
            );
        }
    }

    #[test]
    fn packet_plan_does_not_apply_urlsession_probes_to_python_session_adapters() {
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

        for forbidden in [
            "Session.swift Session",
            "Session.swift Session.request",
            "Request.swift Request",
            "Request.swift Request.resume",
            "request_object.swift request",
            "request_object.swift validate",
            "delegate_callbacks.swift delegate",
            "delegate_callbacks.swift urlSession",
        ] {
            assert!(
                !queries.contains(&forbidden),
                "Python session-adapter prompt should not include Swift URLSession probe `{forbidden}` in {queries:?}"
            );
            assert!(
                !required.iter().any(|query| query == forbidden),
                "Python session-adapter prompt should not require Swift URLSession probe `{forbidden}` in {required:?}"
            );
        }

        for expected in [
            "request preparation",
            "session request",
            "session send",
            "adapter send",
            "adapter selection",
        ] {
            assert!(
                queries.contains(&expected),
                "Python session-adapter prompt should keep request/adapter probe `{expected}` in {queries:?}"
            );
            assert!(
                required.iter().any(|query| query == expected),
                "Python session-adapter prompt should require request/adapter probe `{expected}` in {required:?}"
            );
        }
    }

    #[test]
    fn packet_plan_keeps_javascript_route_probes_separate_from_route_tree_probes() {
        let express_question = "Trace how Express creates an application, registers middleware/routes, and handles an incoming request through the router and response helpers.";
        let express_plan = build_packet_plan(
            express_question,
            Some(PacketTaskClassDto::RouteTracing),
            PacketBudgetModeDto::Compact,
        );
        let express_queries = express_plan
            .queries
            .iter()
            .map(|query| query.query.as_str())
            .collect::<Vec<_>>();
        let express_required = packet_sufficiency_required_probe_queries(
            express_question,
            PacketTaskClassDto::RouteTracing,
        );

        for expected in [
            "middleware registration",
            "route registration",
            "request handler",
            "response send",
        ] {
            assert!(
                express_queries.contains(&expected),
                "Express route prompt should include JS/source probe `{expected}` in {express_queries:?}"
            );
        }
        for forbidden in [
            "router group",
            "route tree",
            "route tree add route",
            "router group handle route",
            "engine request handler",
            "context next handler chain",
            "engine creation",
            "engine creation router state",
        ] {
            assert!(
                !express_queries.contains(&forbidden),
                "Express route prompt should not inherit route-tree probe `{forbidden}` in {express_queries:?}"
            );
            assert!(
                !express_required.iter().any(|query| query == forbidden),
                "Express route prompt should not require route-tree probe `{forbidden}` in {express_required:?}"
            );
        }

        let gin_question = "Trace how Gin creates an engine, registers routes through router groups, stores them in method trees, and dispatches handlers for a request.";
        let gin_plan = build_packet_plan(
            gin_question,
            Some(PacketTaskClassDto::RouteTracing),
            PacketBudgetModeDto::Compact,
        );
        let gin_queries = gin_plan
            .queries
            .iter()
            .map(|query| query.query.as_str())
            .collect::<Vec<_>>();

        for expected in [
            "router group",
            "route tree",
            "route tree add route",
            "engine request handler",
            "context next handler chain",
            "engine creation router state",
        ] {
            assert!(
                gin_queries.contains(&expected),
                "Gin engine/tree prompt should keep route-tree probe `{expected}` in {gin_queries:?}"
            );
        }
    }

    #[test]
    fn packet_plan_derives_generic_shell_install_dispatch_source_probes() {
        let question = "Trace how an install script bootstraps the shell function and dispatches install, download, and use commands.";
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
            "shell installer bootstrap",
            "install download helpers",
            "shell function dispatch",
            "conditional version use",
            "shell completion",
        ] {
            assert!(
                queries.contains(&expected),
                "packet plan should include generic shell dispatch probe `{expected}` in {queries:?}"
            );
            assert!(
                required.iter().any(|query| query == expected),
                "packet required probes should protect generic shell dispatch probe `{expected}` in {required:?}"
            );
        }

        for http_seed in ["router", "route handler endpoint", "middleware"] {
            assert!(
                !queries.contains(&http_seed),
                "shell dispatcher route-tracing prompt should not include HTTP route seed `{http_seed}` in {queries:?}"
            );
        }
    }

    #[test]
    fn gin_route_dispatch_source_claims_name_registration_and_context_flow() {
        let _eval_probes = EvalProbesGuard::enabled();
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
    fn server_route_source_claims_survive_with_eval_probes() {
        let _eval_probes = EvalProbesGuard::enabled();
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
                "expected eval-only server-route claim `{expected}` for {path}; got {claims:?}"
            );
        }
    }

    #[test]
    fn express_shape_route_claims_survive_with_eval_probes() {
        let _eval_probes = EvalProbesGuard::enabled();
        let prompt = "Trace how Express creates an app, registers middleware and routes, handles an incoming request, and sends a response.";

        let fixtures = [
            (
                "createApplication",
                "lib/express.js",
                r#"
                function createApplication() {
                  var app = function(req, res, next) { app.handle(req, res, next); };
                  mixin(app, proto, false);
                  app.request = Object.create(req);
                  app.response = Object.create(res);
                  app.init();
                  return app;
                }
                "#,
                "The application factory builds a callable app object and mixes in request and response prototypes.",
            ),
            (
                "application",
                "lib/application.js",
                r#"
                app.init = function init() {
                  this.defaultConfiguration();
                  var router = new Router({});
                };

                app.handle = function handle(req, res, callback) {
                  this.router.handle(req, res, done);
                };

                app.use = function use(fn) {
                  return router.use(path, fn);
                };

                app.route = function route(path) {
                  return this.router.route(path);
                };
                "#,
                "app.init creates application state and lazy router configuration.",
            ),
            (
                "response",
                "lib/response.js",
                r#"
                res.send = function send(body) {
                  this.set('Content-Length', len);
                  return this.end(chunk, encoding);
                };
                "#,
                "res.send prepares and sends the response body.",
            ),
        ];

        for (symbol, path, source, expected) in fixtures {
            let citation = test_packet_citation(symbol, path, 0.9);
            let claims = packet_source_derived_claims_for_citation(prompt, &citation, source);
            assert!(
                claims.iter().any(|claim| claim == expected),
                "expected application-route claim `{expected}` for {path}; got {claims:?}"
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
    fn hook_cache_source_claims_survive_with_eval_probes() {
        let _eval_probes = EvalProbesGuard::enabled();
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
            "expected eval-only hook wrapper claim `{expected}`; got {claims:?}"
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
            "expected eval-only cache helper claim `{expected}`; got {claims:?}"
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
            "expected eval-only SWR key serialization claim `{expected}`; got {claims:?}"
        );
    }

    #[test]
    fn client_send_source_claims_survive_with_eval_probes() {
        let _eval_probes = EvalProbesGuard::enabled();
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
            "expected eval-only client convenience claim `{expected}`; got {claims:?}"
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
        let expected = "NativeClient.send is the dart:io transport implementation that forwards finalized requests through an HTTP client.";
        assert!(
            claims.iter().any(|claim| claim == expected),
            "expected eval-only transport send claim `{expected}`; got {claims:?}"
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
        let _eval_probes = EvalProbesGuard::enabled();
        let prompt = "Explain how animate.css defines shared animation variables/base classes and connects named animation classes to keyframes.";
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
            let citation = test_packet_citation(path, path, 0.9);
            let claims = packet_source_derived_claims_for_citation(prompt, &citation, source);
            assert!(
                claims.iter().any(|claim| claim == expected),
                "expected CSS animation claim `{expected}` for {path}; got {claims:?}"
            );
        }
    }
    #[test]
    fn generic_sql_schema_claims_survive_with_generic_claims() {
        let prompt = "Explain SQL schema relationships between artists, albums, tracks, invoices, and invoice lines across SQL seed scripts. Cite the source files.";
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
    fn generic_sql_schema_packet_plan_derives_prompt_table_probes() {
        let prompt = "Explain SQL schema relationships between artists, albums, tracks, invoices, and invoice lines across seed scripts.";
        let plan = build_packet_plan(
            prompt,
            Some(PacketTaskClassDto::DataFlow),
            PacketBudgetModeDto::Standard,
        );
        let queries = plan
            .queries
            .iter()
            .map(|query| query.query.as_str())
            .collect::<Vec<_>>();
        let required =
            packet_sufficiency_required_probe_queries(prompt, PacketTaskClassDto::DataFlow);

        for expected in [
            "CREATE TABLE Artist",
            "CREATE TABLE Album",
            "CREATE TABLE Track",
            "CREATE TABLE Invoice",
            "CREATE TABLE InvoiceLine",
            "FOREIGN KEY",
            "REFERENCES",
        ] {
            assert!(
                queries.iter().any(|query| query == &expected),
                "expected SQL schema packet query `{expected}` in {queries:?}"
            );
            assert!(
                required.iter().any(|query| query == expected),
                "expected SQL schema required probe `{expected}` in {required:?}"
            );
        }
        assert!(
            !queries.iter().any(|query| query == &"CREATE TABLE File"),
            "source-file wording should not become a SQL table probe: {queries:?}"
        );
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
            "Runtime formatting routes format calls through a central runtime argument path.",
            "Runtime formatting uses type-erased arguments before dispatching formatted output helpers.",
            "Runtime formatting defines an error type for formatting failures.",
            "Runtime formatting writes formatted output through output iterator helpers.",
        ] {
            assert!(
                claims.iter().any(|claim| claim == expected),
                "expected runtime formatting claim `{expected}` in {claims:?}"
            );
        }

        let arg_store = test_packet_citation("dynamic_format_arg_store", "include/fmt/args.h", 0.9);
        let arg_store_claims = packet_source_derived_claims_for_citation(
            prompt,
            &arg_store,
            r#"
            template <typename Context>
            class dynamic_format_arg_store {
              template <typename T>
              void push_back(const T& value) {
                data_.push_back(detail::make_arg<Context>(value));
              }
            };
            "#,
        );
        assert!(
            arg_store_claims.iter().any(|claim| claim
                == "Runtime formatting builds type-erased format argument stores before dispatching formatting."),
            "expected runtime formatting argument-store claim in {arg_store_claims:?}"
        );

        let format_cc = test_packet_citation("buffer<char>::append", "src/format.cc", 0.9);
        let source_claims = packet_source_derived_claims_for_citation(
            prompt,
            &format_cc,
            "template FMT_API void buffer<char>::append(const char*, const char*);",
        );
        assert!(
            source_claims.iter().any(|claim| claim
                == "Runtime formatting source instantiates buffer append paths for formatted output."),
            "expected runtime formatting source-buffer claim in {source_claims:?}"
        );

        let os_cc = test_packet_citation("format_windows_error", "src/os.cc", 0.9);
        let os_claims = packet_source_derived_claims_for_citation(
            prompt,
            &os_cc,
            r#"void format_windows_error(detail::buffer<char>& out, int error_code, const char* message) {
              fmt::format_to(appender(out), FMT_STRING("{}: {}"), message, format_system_error(error_code));
            }
            std::system_error vformat_system_error(int ec, string_view format_str, format_args args) {
              return std::system_error(ec, vformat(format_str, args));
            }"#,
        );
        assert!(
            os_claims.iter().any(|claim| claim
                == "Runtime formatting error-boundary code formats system errors through shared formatting helpers."),
            "expected runtime formatting OS-boundary claim in {os_claims:?}"
        );
    }

    #[test]
    fn site_build_packet_plan_derives_lifecycle_symbol_probes() {
        let prompt = "Trace how Jekyll's build command creates a site and runs the read, generate, render, and write phases.";
        let plan = build_packet_plan(
            prompt,
            Some(PacketTaskClassDto::RouteTracing),
            PacketBudgetModeDto::Standard,
        );
        let queries = plan
            .queries
            .iter()
            .map(|query| query.query.as_str())
            .collect::<Vec<_>>();
        let required =
            packet_sufficiency_required_probe_queries(prompt, PacketTaskClassDto::RouteTracing);

        for expected in [
            "build process entrypoint",
            "site lifecycle process phases",
            "site read phase",
            "site render phase",
            "site write phase",
            "content reader read phase",
            "page renderer render phase",
        ] {
            assert!(
                queries.iter().any(|query| query == &expected),
                "expected site-build packet query `{expected}` in {queries:?}"
            );
            assert!(
                required.iter().any(|query| query == expected),
                "expected site-build required probe `{expected}` in {required:?}"
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
                "Build.process constructs or processes a site.",
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
                "The site lifecycle method runs reset, read, generate, render, cleanup, and write phases.",
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
                "Content reading source owns the site content read phase.",
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
                "Page rendering source handles page and document rendering.",
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
        write_packet_fixture_file(
            &root,
            "lib/application.js",
            r#"
            app.init = function init() {
              this.defaultConfiguration();
            };
            app.handle = function handle(req, res, callback) {
              this.router.handle(req, res, callback);
            };
            app.use = function use(fn) {
              return this.router.use(fn);
            };
            "#,
        );
        write_packet_fixture_file(
            &root,
            "lib/response.js",
            r#"
            res.send = function send(body) {
              this.set('Content-Length', body.length);
              return this.end(body);
            };
            "#,
        );

        let mut pathless = test_packet_citation("pathless", "", 0.1);
        pathless.file_path = None;
        let mut answer = packet_answer_fixture("fixture packet", vec![pathless]);
        let probes = [
            "lib/jekyll/site.rb Site#process".to_string(),
            "src/Logging/Logger.php Logger::addRecord".to_string(),
            "html/forms/custom-validation/detailed-custom-validation.html input#mail".to_string(),
            "html/forms/custom-validation/detailed-custom-validation.html novalidate".to_string(),
            "lib/application.js init".to_string(),
            "lib/application.js handle".to_string(),
            "lib/application.js use".to_string(),
            "lib/response.js send".to_string(),
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
        let has_js_init = answer.citations.iter().any(|citation| {
            citation.display_name == "app.init" && citation.kind == NodeKind::METHOD
        });
        let has_js_handle = answer.citations.iter().any(|citation| {
            citation.display_name == "app.handle" && citation.kind == NodeKind::METHOD
        });
        let has_js_use = answer.citations.iter().any(|citation| {
            citation.display_name == "app.use" && citation.kind == NodeKind::METHOD
        });
        let has_js_send = answer.citations.iter().any(|citation| {
            citation.display_name == "res.send" && citation.kind == NodeKind::METHOD
        });
        let used_source_probe = answer.retrieval_trace.annotations.iter().any(|annotation| {
            annotation.starts_with("packet_required_file_scoped_source_citations ")
                && annotation.contains("appended=8")
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
            has_js_init && has_js_handle && has_js_use && has_js_send,
            "required source probe should append JavaScript receiver-method anchors: {:?}",
            answer.citations
        );
        assert!(
            used_source_probe,
            "required source probe should annotate appended anchor count: {:?}",
            answer.retrieval_trace.annotations
        );
    }

    #[test]
    fn generic_source_shape_scan_adds_receiver_method_anchors() {
        let root = packet_temp_root("generic-source-shape-route");
        let _ = std::fs::remove_dir_all(&root);
        write_packet_fixture_file(
            &root,
            "src/core/application.js",
            r#"
            service.init = function init() {
              this.defaultConfiguration();
              this.router = new Router();
            };
            service.handle = function handle(req, res, callback) {
              this.router.handle(req, res, callback);
            };
            service.use = function use(fn) {
              return this.router.use(fn);
            };
            service.route = function route(path) {
              return this.router.route(path);
            };
            "#,
        );
        write_packet_fixture_file(
            &root,
            "src/core/response.js",
            r#"
            reply.send = function send(body) {
              this.set('Content-Length', body.length);
              return this.end(body);
            };
            "#,
        );

        let mut answer = packet_answer_fixture(
            "Trace how a server application registers middleware/routes and handles a request through router and response helpers.",
            Vec::new(),
        );
        maybe_append_generic_source_shape_citations(
            &root,
            "Trace how a server application registers middleware/routes and handles a request through router and response helpers.",
            &mut answer,
        );
        let displays = answer
            .citations
            .iter()
            .map(|citation| citation.display_name.as_str())
            .collect::<Vec<_>>();
        for expected in [
            "service.init",
            "service.handle",
            "service.use",
            "service.route",
            "reply.send",
        ] {
            assert!(
                displays.contains(&expected),
                "expected generic receiver-method source shape {expected}; got {displays:?}"
            );
        }
        assert!(answer.retrieval_trace.annotations.iter().any(|annotation| {
            annotation.starts_with("packet_generic_source_shape_citations appended=")
        }));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn generic_source_shape_scan_adds_client_send_dart_anchors() {
        let root = packet_temp_root("generic-source-shape-client-send");
        let _ = std::fs::remove_dir_all(&root);
        write_packet_fixture_file(
            &root,
            "lib/http.dart",
            r#"
            export 'src/client.dart';
            export 'src/request.dart';
            export 'src/response.dart';
            Future<Response> get(Uri url) => _withClient((client) => client.get(url));
            Future<Response> post(Uri url) => _withClient((client) => client.post(url));
            Future<T> _withClient<T>(Future<T> Function(Client) fn) async {
              var client = Client();
              return await fn(client);
            }
            "#,
        );
        write_packet_fixture_file(
            &root,
            "lib/src/client.dart",
            r#"
            abstract interface class Client {
              Future<Response> get(Uri url);
              Future<Response> post(Uri url);
              Future<StreamedResponse> send(BaseRequest request);
            }
            "#,
        );
        write_packet_fixture_file(
            &root,
            "lib/src/request.dart",
            r#"
            class Request extends BaseRequest {
              ByteStream finalize() {
                return ByteStream.fromBytes(bodyBytes);
              }
            }
            "#,
        );
        write_packet_fixture_file(
            &root,
            "lib/src/response.dart",
            r#"
            class Response extends BaseResponse {
              static Future<Response> fromStream(StreamedResponse response) async {
                final body = await response.stream.toBytes();
                return Response.bytes(body, response.statusCode);
              }
            }
            "#,
        );

        let mut answer = packet_answer_fixture(
            "Explain how a package exposes top-level helpers, Client convenience methods, Request finalization, and transport send behavior.",
            Vec::new(),
        );
        maybe_append_generic_source_shape_citations(
            &root,
            "Explain how a package exposes top-level helpers, Client convenience methods, Request finalization, and transport send behavior.",
            &mut answer,
        );
        let displays = answer
            .citations
            .iter()
            .map(|citation| citation.display_name.as_str())
            .collect::<Vec<_>>();
        for expected in [
            "Top-level HTTP helpers",
            "Client interface helpers",
            "Request.finalize",
            "Response.fromStream",
        ] {
            assert!(
                displays.contains(&expected),
                "expected generic client-send source shape {expected}; got {displays:?}"
            );
        }
        assert!(
            answer.citations.iter().any(|citation| {
                citation.display_name == "Top-level HTTP helpers"
                    && citation.coverage_role.as_deref() == Some("client public facade")
                    && citation.eligible_for_sufficiency == Some(true)
            }),
            "expected public facade source shape to be sufficiency eligible: {:?}",
            answer.citations
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn generic_source_shape_scan_adds_hook_cache_anchors() {
        let root = packet_temp_root("generic-source-shape-hook-cache");
        let _ = std::fs::remove_dir_all(&root);
        write_packet_fixture_file(
            &root,
            "src/hooks/use-data.ts",
            r#"
            export const useDataHandler = (_key, cache) => {
              const [key, fnArg] = serialize(_key)
              const [getCache, setCache] = createCacheHelper(cache, key)
              return internalMutate(cache, key, fnArg)
            }
            "#,
        );
        write_packet_fixture_file(
            &root,
            "src/runtime/key.ts",
            r#"
            export const normalizeKey = key => {
              const args = key
              key = typeof key == 'string' ? key : stableHash(key)
              return [key, args]
            }
            "#,
        );
        write_packet_fixture_file(
            &root,
            "src/runtime/cache-helper.ts",
            r#"
            export const makeCacheHelper = (cache, key) => {
              const state = runtimeState.get(cache)
              return [
                () => cache.get(key) || EMPTY_CACHE,
                info => {
                  const prev = cache.get(key)
                  cache.set(key, info)
                  state[5](key, info, prev)
                },
                state[6],
                () => snapshot[key] || cache.get(key)
              ] as const
            }
            "#,
        );
        write_packet_fixture_file(
            &root,
            "src/runtime/mutate.ts",
            r#"
            export async function applyMutation(cache, _key, data) {
              return mutateByKey(_key)
              async function mutateByKey(_k) {
                const [key] = serialize(_k)
                const [get, set] = createCacheHelper(cache, key)
                set({ data })
              }
            }
            "#,
        );
        write_packet_fixture_file(
            &root,
            "src/runtime/middleware.ts",
            r#"
            export const withRuntimeMiddleware = (useHook: SWRHook, middleware) => {
              return (...args) => {
                const config = { use: [] }
                const uses = (config.use || []).concat(middleware)
                return useHook(args[0], args[1], { ...config, use: uses })
              }
            }
            "#,
        );

        let mut answer = packet_answer_fixture(
            "Explain how a public hook serializes keys, connects cache helpers, composes middleware, and routes mutate behavior.",
            Vec::new(),
        );
        maybe_append_generic_source_shape_citations(
            &root,
            "Explain how a public hook serializes keys, connects cache helpers, composes middleware, and routes mutate behavior.",
            &mut answer,
        );
        let roles = answer
            .citations
            .iter()
            .map(|citation| {
                (
                    citation.display_name.as_str(),
                    citation.coverage_role.as_deref(),
                )
            })
            .collect::<Vec<_>>();
        for (expected, role) in [
            ("useDataHandler", "hook_key_serialization"),
            ("normalizeKey", "hook_key_serialization"),
            ("makeCacheHelper", "hook_cache_helper"),
            ("applyMutation", "hook_mutation_flow"),
            ("withRuntimeMiddleware", "hook_middleware_composition"),
        ] {
            assert!(
                roles.iter().any(
                    |(display, actual_role)| *display == expected && *actual_role == Some(role)
                ),
                "expected generic hook/cache source shape {expected} with role {role}; got {roles:?}"
            );
        }

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn generic_source_shape_scan_adds_command_flow_c_anchors() {
        let root = packet_temp_root("generic-source-shape-command-flow");
        let _ = std::fs::remove_dir_all(&root);
        write_packet_fixture_file(
            &root,
            "src/server.c",
            r#"
            void initServer(void) {
              server.el = createEventLoop();
            }
            int main(int argc, char **argv) {
              initServer();
              aeMain(server.el);
            }
            int processCommand(client *c) {
              return call(c, CMD_CALL_FULL);
            }
            void call(client *c, int flags) {
              c->cmd->proc(c);
            }
            "#,
        );
        write_packet_fixture_file(
            &root,
            "src/ae.c",
            r#"
            void aeMain(aeEventLoop *eventLoop) {
              while (!eventLoop->stop) {
                aeProcessEvents(eventLoop, AE_ALL_EVENTS);
              }
            }
            "#,
        );
        write_packet_fixture_file(
            &root,
            "src/networking.c",
            r#"
            void readQueryFromClient(connection *conn) {
              client *c = connGetPrivateData(conn);
              readQueryFromClient(c);
              processInputBuffer(c);
              processCommand(c);
            }
            "#,
        );
        write_packet_fixture_file(
            &root,
            "deps/noise.c",
            r#"
            void processCommand(client *c) {}
            "#,
        );

        let mut answer = packet_answer_fixture(
            "Trace how a command server bootstrap enters an event loop, reads client input, and dispatches commands through a command table.",
            Vec::new(),
        );
        maybe_append_generic_source_shape_citations(
            &root,
            "Trace how a command server bootstrap enters an event loop, reads client input, and dispatches commands through a command table.",
            &mut answer,
        );
        let displays = answer
            .citations
            .iter()
            .map(|citation| citation.display_name.as_str())
            .collect::<Vec<_>>();
        for expected in [
            "initServer",
            "aeMain",
            "readQueryFromClient",
            "processCommand",
        ] {
            assert!(
                displays.contains(&expected),
                "expected generic command source shape {expected}; got {displays:?}"
            );
        }
        assert!(
            answer.citations.iter().any(|citation| {
                citation.display_name == "readQueryFromClient"
                    && citation.coverage_role.as_deref() == Some("command_network_input")
                    && citation.eligible_for_sufficiency == Some(true)
            }),
            "expected client input source shape to be sufficiency eligible: {:?}",
            answer.citations
        );
        assert!(
            answer.citations.iter().all(|citation| !packet_display_path(
                citation.file_path.as_deref().unwrap_or("")
            )
            .starts_with("deps/")),
            "vendor/deps candidates should stay out of generic command source shapes: {:?}",
            answer.citations
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn generic_source_shape_scan_adds_form_native_constraint_anchor() {
        let root = packet_temp_root("generic-source-shape-form-validation");
        let _ = std::fs::remove_dir_all(&root);
        write_packet_fixture_file(
            &root,
            "lessons/forms/native-validation.html",
            r#"
            <form>
              <input required pattern="[a-z]+" min="3" max="12">
            </form>
            "#,
        );
        write_packet_fixture_file(
            &root,
            "lessons/forms/pattern-example.html",
            r#"
            <form>
              <input id="fruit" required pattern="[a-z]+">
            </form>
            "#,
        );
        write_packet_fixture_file(
            &root,
            "lessons/forms/custom-validation.html",
            r#"
            <form novalidate>
              <input id="mail" type="email" required>
            </form>
            <script>
              function showError() {
                if (mail.validity.valueMissing) {
                  error.textContent = "Email required";
                } else if (mail.validity.typeMismatch) {
                  error.textContent = "Email invalid";
                } else if (mail.validity.tooShort) {
                  error.textContent = "Email too short";
                }
              }
            </script>
            "#,
        );

        let mut answer = packet_answer_fixture(
            "Explain how form validation examples combine native HTML constraints with custom JavaScript validation.",
            Vec::new(),
        );
        maybe_append_generic_source_shape_citations(
            &root,
            "Explain how form validation examples combine native HTML constraints with custom JavaScript validation.",
            &mut answer,
        );

        assert!(
            answer.citations.iter().any(|citation| {
                citation.display_name == "Native form constraints"
                    && citation.coverage_role.as_deref() == Some("form_native_constraints")
                    && citation.eligible_for_sufficiency == Some(true)
            }),
            "expected native constraint source shape: {:?}",
            answer.citations
        );
        assert!(
            answer.citations.iter().any(|citation| {
                citation.display_name == "pattern"
                    && citation.coverage_role.as_deref() == Some("form_pattern_constraint")
            }),
            "expected pattern-only form source shape: {:?}",
            answer.citations
        );
        assert!(
            answer.citations.iter().any(|citation| {
                citation.display_name == "novalidate"
                    && citation.coverage_role.as_deref() == Some("form_validation_bypass")
            }),
            "expected novalidate source shape: {:?}",
            answer.citations
        );
        assert!(
            answer.citations.iter().any(|citation| {
                citation.display_name == "input#mail"
                    && citation.coverage_role.as_deref() == Some("form_custom_input")
            }),
            "expected input id source shape: {:?}",
            answer.citations
        );
        assert!(
            answer.citations.iter().any(|citation| {
                citation.display_name == "showError"
                    && citation.coverage_role.as_deref() == Some("form_custom_error_rendering")
            }),
            "expected custom error rendering source shape: {:?}",
            answer.citations
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn generic_source_shape_scan_adds_buffered_io_anchors() {
        let root = packet_temp_root("generic-source-shape-buffered-io");
        let _ = std::fs::remove_dir_all(&root);
        write_packet_fixture_file(
            &root,
            "src/io/RealBufferedSource.kt",
            r#"
            internal class RealBufferedSource(val source: Source) {
              val buffer = Buffer()
              fun read(sink: Buffer, byteCount: Long): Long = source.read(buffer, byteCount)
            }
            "#,
        );
        write_packet_fixture_file(
            &root,
            "src/io/Okio.kt",
            r#"
            fun Source.buffer(): BufferedSource = RealBufferedSource(this)
            fun Sink.buffer(): BufferedSink = RealBufferedSink(this)
            "#,
        );

        let prompt = "Explain how Buffer, Source, Sink, and buffered wrappers cooperate to move bytes through reads and writes.";
        let mut answer = packet_answer_fixture(prompt, Vec::new());
        maybe_append_generic_source_shape_citations(&root, prompt, &mut answer);

        assert!(
            answer.citations.iter().any(|citation| {
                citation.display_name == "RealBufferedSource"
                    && citation.coverage_role.as_deref() == Some("buffered_source_impl")
            }),
            "expected buffered source implementation anchor: {:?}",
            answer.citations
        );
        assert!(
            answer.citations.iter().any(|citation| {
                citation.display_name == "buffer"
                    && citation.coverage_role.as_deref() == Some("buffered_wrapper_helper")
            }),
            "expected buffered wrapper helper anchor: {:?}",
            answer.citations
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn generic_source_shape_scan_adds_url_session_request_anchors() {
        let root = packet_temp_root("generic-source-shape-urlsession");
        let _ = std::fs::remove_dir_all(&root);
        write_packet_fixture_file(
            &root,
            "Source/Core/Request.swift",
            r#"
            open class Request {
              public func resume() -> Self {
                task?.resume()
                return self
              }
            }
            "#,
        );
        write_packet_fixture_file(
            &root,
            "Source/Core/DataRequest.swift",
            r#"
            open class DataRequest: Request {
              public func validate(_ validation: @escaping Validation) -> Self {
                validators.write { $0.append(validation) }
                return self
              }
            }
            "#,
        );
        write_packet_fixture_file(
            &root,
            "Source/Core/DownloadRequest.swift",
            r#"
            open class DownloadRequest: Request {
              public func validate(_ validation: @escaping Validation) -> Self {
                validators.write { $0.append(validation) }
                return self
              }
            }
            "#,
        );
        write_packet_fixture_file(
            &root,
            "Source/Core/SessionDelegate.swift",
            r#"
            open class SessionDelegate: NSObject, URLSessionDataDelegate {
              open func urlSession(_ session: URLSession, dataTask: URLSessionDataTask, didReceive data: Data) {
                request.didReceive(data: data)
              }
              open func urlSession(_ session: URLSession, task: URLSessionTask, didCompleteWithError error: Error?) {
                request.didReceiveResponse(nil)
              }
            }
            "#,
        );

        let prompt = "Trace how a Session creates requests, resumes tasks, validates data requests, and receives URLSession callbacks.";
        let mut answer = packet_answer_fixture(prompt, Vec::new());
        maybe_append_generic_source_shape_citations(&root, prompt, &mut answer);

        assert!(
            answer.citations.iter().any(|citation| {
                citation.display_name == "Request.resume"
                    && citation.coverage_role.as_deref() == Some("request_resume_dispatch")
            }),
            "expected request resume source anchor: {:?}",
            answer.citations
        );
        assert!(
            answer.citations.iter().any(|citation| {
                citation.display_name == "DataRequest.validate"
                    && citation.coverage_role.as_deref() == Some("request_validation_pipeline")
            }),
            "expected request validation source anchor: {:?}",
            answer.citations
        );
        let data_rank = answer
            .citations
            .iter()
            .position(|citation| citation.display_name == "DataRequest.validate")
            .expect("DataRequest.validate anchor");
        let download_rank = answer
            .citations
            .iter()
            .position(|citation| citation.display_name == "DownloadRequest.validate")
            .expect("DownloadRequest.validate anchor");
        assert!(
            data_rank < download_rank,
            "data-bearing request validation should outrank sibling validation anchors: {:?}",
            answer.citations
        );
        assert!(
            answer.citations.iter().any(|citation| {
                citation.display_name == "SessionDelegate.urlSession"
                    && citation.coverage_role.as_deref() == Some("session_callbacks")
            }),
            "expected URLSession delegate callback source anchor: {:?}",
            answer.citations
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn generic_source_shape_scan_adds_cited_request_validation_anchor() {
        let root = packet_temp_root("generic-source-shape-cited-urlsession");
        let _ = std::fs::remove_dir_all(&root);
        let request_path = root.join("Example").join("BodyRequest.swift");
        write_packet_fixture_file(
            &root,
            "Example/BodyRequest.swift",
            r#"
            open class BodyRequest: Request {
              public func validate(_ validation: @escaping Validation) -> Self {
                validators.write { $0.append(validation) }
                eventMonitor?.request(self, didValidateRequest: request)
                return self
              }
            }
            "#,
        );

        let prompt = "Trace how a Session creates requests, resumes tasks, validates data requests, and receives URLSession callbacks.";
        let mut answer = packet_answer_fixture(
            prompt,
            vec![test_packet_citation(
                "BodyRequest",
                &request_path.to_string_lossy(),
                0.9,
            )],
        );
        maybe_append_generic_source_shape_citations(&root, prompt, &mut answer);

        assert!(
            answer.citations.iter().any(|citation| {
                citation.display_name == "BodyRequest.validate"
                    && citation.coverage_role.as_deref() == Some("request_validation_pipeline")
            }),
            "expected cited request validation source anchor: {:?}",
            answer.citations
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn generic_source_shape_scan_adds_runtime_formatting_type_anchors() {
        let root = packet_temp_root("generic-source-shape-formatting");
        let _ = std::fs::remove_dir_all(&root);
        write_packet_fixture_file(
            &root,
            "include/tool/base.hpp",
            r#"
            namespace detail {
            struct runtime_format_arg_store {
              void push_back();
            };
            }
            "#,
        );
        write_packet_fixture_file(
            &root,
            "include/tool/dynamic.hpp",
            r#"
            template <typename Context> class dynamic_format_argument_store {
            public:
              void push_back();
            };
            "#,
        );
        write_packet_fixture_file(
            &root,
            "include/tool/errors.hpp",
            r#"
            class TOOL_EXPORT format_failure : public std::runtime_error {
            };
            "#,
        );

        let mut answer = packet_answer_fixture(
            "Explain how a formatting runtime turns arguments into type-erased format argument stores and reports formatting failure types.",
            Vec::new(),
        );
        maybe_append_generic_source_shape_citations(
            &root,
            "Explain how a formatting runtime turns arguments into type-erased format argument stores and reports formatting failure types.",
            &mut answer,
        );
        let displays = answer
            .citations
            .iter()
            .map(|citation| citation.display_name.as_str())
            .collect::<Vec<_>>();
        for expected in [
            "runtime_format_arg_store",
            "dynamic_format_argument_store",
            "format_failure",
        ] {
            assert!(
                displays.contains(&expected),
                "expected generic formatting source shape {expected}; got {displays:?}"
            );
        }

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn generic_source_shape_scan_adds_csharp_mapper_plan_anchors() {
        let root = packet_temp_root("generic-source-shape-csharp-mapper");
        let _ = std::fs::remove_dir_all(&root);
        write_packet_fixture_file(
            &root,
            "src/ObjectMapping/RuntimeMapper.cs",
            r#"
            namespace ObjectMapping;
            public interface IRuntimeMapperBase
            {
              TDestination Map<TSource, TDestination>(TSource source);
              object Map(object source, Type sourceType, Type destinationType);
            }
            public interface IRuntimeMapper : IRuntimeMapperBase
            {
              IConfigurationProvider ConfigurationProvider { get; }
            }
            public sealed class RuntimeMapper : IRuntimeMapper
            {
              public TDestination Map<TSource, TDestination>(TSource source) =>
                MapCore<TSource, TDestination>(source, default);
              TDestination MapCore<TSource, TDestination>(TSource source, TDestination destination) =>
                _configuration.GetExecutionPlan<TSource, TDestination>()(source, destination);
            }
            "#,
        );
        write_packet_fixture_file(
            &root,
            "src/ObjectMapping/Configuration/MappingConfiguration.cs",
            r#"
            namespace ObjectMapping;
            public sealed class MappingConfiguration
            {
              private readonly Dictionary<TypePair, MappingPlan> _configuredMaps = new();
              private readonly Dictionary<TypePair, MappingPlan> _resolvedMaps = new();
              private readonly Dictionary<MapRequest, Delegate> _executionPlans = new();
              public RuntimeMapper CreateMapper() => new(this);
              public LambdaExpression BuildExecutionPlan(Type sourceType, Type destinationType) =>
                _resolvedMaps[new(sourceType, destinationType)].MapExpression;
            }
            "#,
        );
        write_packet_fixture_file(
            &root,
            "src/ObjectMapping/MappingPlan.cs",
            r#"
            namespace ObjectMapping;
            public sealed class MappingPlan
            {
              public Type SourceType { get; }
              public Type DestinationType { get; }
              public LambdaExpression MapExpression { get; private set; }
              internal LambdaExpression BuildMapperLambda(IGlobalMappingConfiguration configuration) =>
                Types.ContainsGenericParameters ? null : new MappingPlanBuilder(configuration, this).BuildMapperLambda();
            }
            "#,
        );

        let mut answer = packet_answer_fixture(
            "Explain how mapper configuration and runtime mapper APIs cooperate to map source objects to destination objects through type-map lambda plans.",
            Vec::new(),
        );
        maybe_append_generic_source_shape_citations(
            &root,
            "Explain how mapper configuration and runtime mapper APIs cooperate to map source objects to destination objects through type-map lambda plans.",
            &mut answer,
        );
        let displays = answer
            .citations
            .iter()
            .map(|citation| citation.display_name.as_str())
            .collect::<Vec<_>>();
        assert!(
            displays.iter().any(|display| {
                display.contains("IRuntimeMapperBase")
                    && display.contains("IRuntimeMapper")
                    && display.contains("RuntimeMapper.Map")
            }),
            "expected compact generic C# mapper facade source-shape; got {displays:?}"
        );
        for expected in ["MappingConfiguration", "MappingPlan.BuildMapperLambda"] {
            assert!(
                displays.contains(&expected),
                "expected generic C# mapper source-shape {expected}; got {displays:?}"
            );
        }

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn generic_source_shape_csharp_mapper_facade_group_survives_compact_budget() {
        let root = packet_temp_root("generic-source-shape-csharp-mapper-facade");
        let _ = std::fs::remove_dir_all(&root);
        write_packet_fixture_file(
            &root,
            "src/ObjectMapping/RuntimeMapper.cs",
            r#"
            namespace ObjectMapping;
            public interface IRuntimeMapperBase
            {
              TDestination Map<TSource, TDestination>(TSource source);
              object Map(object source, Type sourceType, Type destinationType);
            }
            public interface IRuntimeMapper : IRuntimeMapperBase
            {
              IConfigurationProvider ConfigurationProvider { get; }
            }
            internal interface IInternalRuntimeMapper : IRuntimeMapper
            {
              TDestination Map<TSource, TDestination>(TSource source, TDestination destination, ResolutionContext context);
            }
            public sealed class RuntimeMapper : IRuntimeMapper, IInternalRuntimeMapper
            {
              public TDestination Map<TSource, TDestination>(TSource source) =>
                MapCore<TSource, TDestination>(source, default);
              TDestination MapCore<TSource, TDestination>(TSource source, TDestination destination) =>
                _configuration.GetExecutionPlan<TSource, TDestination>()(source, destination);
            }
            "#,
        );

        let prompt = "Explain how mapper configuration and runtime mapper APIs cooperate to map source objects to destination objects through type-map lambda plans.";
        let filler = (0..20)
            .map(|index| {
                test_packet_citation(
                    &format!("UnrelatedHelper{index}"),
                    &format!("src/ObjectMapping/Helpers/UnrelatedHelper{index}.cs"),
                    0.5,
                )
            })
            .collect::<Vec<_>>();
        let mut answer = packet_answer_fixture(prompt, filler);
        maybe_append_generic_source_shape_citations(&root, prompt, &mut answer);
        rank_packet_evidence(prompt, &mut answer);

        let mut limits = packet_budget_limits(PacketBudgetModeDto::Compact);
        limits.max_anchors = 5;
        apply_packet_budget(
            &root,
            prompt,
            PacketTaskClassDto::DataFlow,
            PacketBudgetModeDto::Compact,
            limits,
            &mut answer,
        );

        let displays = answer
            .citations
            .iter()
            .map(|citation| citation.display_name.as_str())
            .collect::<Vec<_>>();
        assert!(
            displays.iter().any(|display| {
                display.contains("IRuntimeMapperBase")
                    && display.contains("IRuntimeMapper")
                    && display.contains("RuntimeMapper.Map")
                    && !display.contains("IInternalRuntimeMapper")
            }),
            "expected compact generic mapper facade group to survive budget cap; got {displays:?}"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn generic_source_shape_scan_adds_css_animation_variable_anchor() {
        let root = packet_temp_root("generic-source-shape-css");
        let _ = std::fs::remove_dir_all(&root);
        write_packet_fixture_file(
            &root,
            "styles/tokens.css",
            r#"
            :root {
              --motion-duration: 250ms;
              --motion-delay: 75ms;
              --motion-repeat: 2;
            }
            "#,
        );

        let mut answer = packet_answer_fixture(
            "Explain how a stylesheet defines shared animation variables, base classes, and named keyframes.",
            Vec::new(),
        );
        maybe_append_generic_source_shape_citations(
            &root,
            "Explain how a stylesheet defines shared animation variables, base classes, and named keyframes.",
            &mut answer,
        );

        assert!(
            answer.citations.iter().any(|citation| {
                citation.display_name == "--motion-duration"
                    && citation.kind == NodeKind::CONSTANT
                    && citation.file_path.as_deref().is_some_and(|path| {
                        packet_display_path(path).ends_with("styles/tokens.css")
                    })
            }),
            "expected root custom-property animation variable citation; got {:?}",
            answer.citations
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn required_file_scoped_source_probe_resolves_unique_basename_anchor() {
        let root = packet_temp_root("required-source-probe-basename");
        let _ = std::fs::remove_dir_all(&root);
        write_packet_fixture_file(
            &root,
            "html/forms/form-validation/detailed-custom-validation.html",
            r#"
            <form novalidate>
              <input id="mail" type="email" required minlength="8">
            </form>
            "#,
        );

        let mut pathless = test_packet_citation("pathless", "", 0.1);
        pathless.file_path = None;
        let mut answer = packet_answer_fixture("fixture packet", vec![pathless]);
        let probes = ["detailed-custom-validation.html input#mail".to_string()];
        maybe_append_required_file_scoped_source_citations(
            &root,
            "fixture packet",
            PacketTaskClassDto::ArchitectureExplanation,
            &probes,
            &mut answer,
        );

        let has_input_anchor = answer.citations.iter().any(|citation| {
            citation.display_name == "input#mail"
                && citation.kind == NodeKind::ANNOTATION
                && citation.file_path.as_deref().is_some_and(|path| {
                    packet_display_path(path)
                        .ends_with("html/forms/form-validation/detailed-custom-validation.html")
                })
        });
        let used_source_probe = answer.retrieval_trace.annotations.iter().any(|annotation| {
            annotation.starts_with("packet_required_file_scoped_source_citations ")
                && annotation.contains("appended=1")
        });

        let _ = std::fs::remove_dir_all(&root);

        assert!(
            has_input_anchor,
            "basename source probe should append the unique HTML id anchor: {:?}",
            answer.citations
        );
        assert!(
            used_source_probe,
            "basename source probe should annotate appended anchor count: {:?}",
            answer.retrieval_trace.annotations
        );
    }

    #[test]
    fn required_file_scoped_source_probe_adds_cpp_template_and_call_anchors() {
        let root = packet_temp_root("required-source-probe-cpp-calls");
        let _ = std::fs::remove_dir_all(&root);
        write_packet_fixture_file(
            &root,
            "src/format.cc",
            r#"
            template FMT_API void buffer<char>::append(const char*, const char*);
            "#,
        );
        write_packet_fixture_file(
            &root,
            "src/os.cc",
            r#"
            // fmt::format_to appears in docs, but the call below is the source anchor.
            fmt::format_to(appender(out), FMT_STRING("{}: {}"), message, error_code);
            "#,
        );

        let mut answer = packet_answer_fixture("fixture packet", Vec::new());
        let probes = [
            "format.cc buffer append".to_string(),
            "os.cc format_to".to_string(),
        ];
        maybe_append_required_file_scoped_source_citations(
            &root,
            "fixture packet",
            PacketTaskClassDto::ArchitectureExplanation,
            &probes,
            &mut answer,
        );

        let has_format_cc_anchor = answer.citations.iter().any(|citation| {
            citation.display_name == "buffer append"
                && citation.kind == NodeKind::ANNOTATION
                && citation
                    .file_path
                    .as_deref()
                    .is_some_and(|path| packet_display_path(path).ends_with("src/format.cc"))
        });
        let has_os_cc_anchor = answer.citations.iter().any(|citation| {
            citation.display_name == "format_to"
                && citation.kind == NodeKind::ANNOTATION
                && citation.line == Some(3)
                && citation
                    .file_path
                    .as_deref()
                    .is_some_and(|path| packet_display_path(path).ends_with("src/os.cc"))
        });
        let used_source_probe = answer.retrieval_trace.annotations.iter().any(|annotation| {
            annotation.starts_with("packet_required_file_scoped_source_citations ")
                && annotation.contains("appended=2")
        });

        let _ = std::fs::remove_dir_all(&root);

        assert!(
            has_format_cc_anchor,
            "required source probe should append C++ template instantiation anchors: {:?}",
            answer.citations
        );
        assert!(
            has_os_cc_anchor,
            "required source probe should append C++ call-site anchors instead of comments: {:?}",
            answer.citations
        );
        assert!(
            used_source_probe,
            "C++ source probes should annotate appended anchor count: {:?}",
            answer.retrieval_trace.annotations
        );
    }

    #[test]
    fn required_file_scoped_source_probe_adds_shell_function_anchor() {
        let root = packet_temp_root("required-source-probe-shell");
        let _ = std::fs::remove_dir_all(&root);
        write_packet_fixture_file(
            &root,
            "install.sh",
            r#"
            nvm_do_install() {
              nvm_install_node
            }
            "#,
        );
        write_packet_fixture_file(
            &root,
            "bash_completion",
            r#"
            __nvm() {
              __nvm_commands
            }
            "#,
        );

        let mut answer = packet_answer_fixture("fixture packet", Vec::new());
        let probes = [
            "install.sh nvm_do_install".to_string(),
            "bash_completion __nvm".to_string(),
        ];
        maybe_append_required_file_scoped_source_citations(
            &root,
            "fixture packet",
            PacketTaskClassDto::RouteTracing,
            &probes,
            &mut answer,
        );

        let has_shell_anchor = answer.citations.iter().any(|citation| {
            citation.display_name == "nvm_do_install"
                && citation.kind == NodeKind::METHOD
                && citation
                    .file_path
                    .as_deref()
                    .is_some_and(|path| packet_display_path(path).ends_with("install.sh"))
        });
        let has_completion_anchor = answer.citations.iter().any(|citation| {
            citation.display_name == "__nvm"
                && citation.kind == NodeKind::METHOD
                && citation
                    .file_path
                    .as_deref()
                    .is_some_and(|path| packet_display_path(path).ends_with("bash_completion"))
        });

        let _ = std::fs::remove_dir_all(&root);

        assert!(
            has_shell_anchor,
            "required source probe should append shell function anchors: {:?}",
            answer.citations
        );
        assert!(
            has_completion_anchor,
            "required source probe should append extensionless completion-file anchors: {:?}",
            answer.citations
        );
    }

    #[test]
    fn automapper_map_flow_source_claims_name_runtime_configuration_and_plans() {
        let prompt = "Explain how mapper configuration and runtime mapper APIs cooperate to map source objects to destination objects through type map plans.";
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
                "Mapping configuration source builds and owns runtime mapping plans.",
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
                "Mapper runtime source exposes the public object-mapping entry point.",
            ),
            (
                "TypeMap.CreateMapperLambda",
                "src/AutoMapper/TypeMap.cs",
                r#"
                internal LambdaExpression CreateMapperLambda(IGlobalConfiguration configuration) =>
                    Types.ContainsGenericParameters ? null : new TypeMapPlanBuilder(configuration, this).CreateMapperLambda();
                "#,
                "Type-map source contributes lambda plans used by the mapping execution pipeline.",
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
                "The mapping plan builder participates in building expression plans for mappings.",
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
    fn express_route_flow_source_claims_name_app_router_response_flow_with_eval_probes() {
        let _eval_probes = EvalProbesGuard::enabled();
        let prompt = "Trace how Express creates an application, registers middleware/routes, and handles an incoming request through the router and response helpers.";
        let fixtures = [
            (
                "createApplication",
                "lib/express.js",
                "function createApplication() { var app = function(req, res, next) { app.handle(req, res, next); }; mixin(app, proto, false); app.request = Object.create(req); app.response = Object.create(res); app.init(); return app; }",
                "The application factory builds a callable app object and mixes in request and response prototypes.",
            ),
            (
                "app.handle",
                "lib/application.js",
                "app.init = function init() { var router = null; this.defaultConfiguration(); router = new Router({}); }\napp.handle = function handle(req, res, callback) { this.router.handle(req, res, done); }\napp.use = function use(fn) { return router.use(path, fn); }\napp.route = function route(path) { return this.router.route(path); }",
                "app.handle delegates request handling to the router.",
            ),
            (
                "app.use",
                "lib/application.js",
                "app.init = function init() { var router = null; this.defaultConfiguration(); router = new Router({}); }\napp.handle = function handle(req, res, callback) { this.router.handle(req, res, done); }\napp.use = function use(fn) { return router.use(path, fn); }\napp.route = function route(path) { return this.router.route(path); }",
                "app.use registers middleware on the router.",
            ),
            (
                "app.route",
                "lib/application.js",
                "app.init = function init() { var router = null; this.defaultConfiguration(); router = new Router({}); }\napp.handle = function handle(req, res, callback) { this.router.handle(req, res, done); }\napp.use = function use(fn) { return router.use(path, fn); }\napp.route = function route(path) { return this.router.route(path); }",
                "app.route creates route entries through the router.",
            ),
            (
                "res.send",
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
    fn url_session_request_claims_name_lifecycle_with_eval_probes() {
        let _eval_probes = EvalProbesGuard::enabled();
        let prompt = "Trace how a Session creates requests, resumes tasks, validates data requests, and receives URLSession callbacks.";
        let fixtures = [
            (
                "Session.request",
                "Source/Core/Session.swift",
                "open func request(_ convertible: URLRequestConvertible) -> DataRequest { let request = DataRequest(); performEagerlyIfNecessary(request); return request }",
                "The session request API creates request objects before optional eager execution.",
            ),
            (
                "Request.resume",
                "Source/Core/Request.swift",
                "public func resume() -> Self { delegate?.readyToPerform(request: self); task.resume(); return self }",
                "The request resume API resumes the underlying URL session task.",
            ),
            (
                "DataRequest.validate",
                "Source/Core/DataRequest.swift",
                "public func validate(_ validation: @escaping Validation) -> Self { validators.write { $0.append(validation) }; didValidateRequest(); return self }",
                "Request validation methods attach validation behavior.",
            ),
            (
                "SessionDelegate",
                "Source/Core/SessionDelegate.swift",
                "open class SessionDelegate: NSObject, URLSessionDataDelegate { open func urlSession(_ session: URLSession, dataTask: URLSessionDataTask, didReceive data: Data) { request.didReceive(data: data) } open func urlSession(_ session: URLSession, task: URLSessionTask, didCompleteWithError error: Error?) { request.didReceiveResponse(nil) } }",
                "Session delegate callbacks receive URLSession task events.",
            ),
        ];

        for (symbol, path, source, expected) in fixtures {
            let citation = test_packet_citation(symbol, path, 0.9);
            let claims = packet_source_derived_claims_for_citation(prompt, &citation, source);
            assert!(
                claims.iter().any(|claim| claim == expected),
                "expected URLSession request lifecycle claim `{expected}` for {path}; got {claims:?}"
            );
        }
    }

    #[test]
    fn java_string_check_source_claims_name_blank_empty_and_region_matching() {
        let _eval_probes = EvalProbesGuard::enabled();
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
    fn exact_family_source_claims_require_eval_probes() {
        let _env = EnvVarGuard::cleared(EVAL_PROBES_ENV);
        let cases = [
            (
                "Explain how Commons Lang implements blank and empty string checks across StringUtils.",
                test_packet_citation(
                    "org.apache.commons.lang3.StringUtils.isBlank",
                    "src/main/java/org/apache/commons/lang3/StringUtils.java",
                    0.9,
                ),
                r#"
                public static boolean isBlank(final CharSequence cs) {
                    if (cs == null || cs.length() == 0) {
                        return true;
                    }
                    return Character.isWhitespace(cs.charAt(0));
                }
                * NOTE: This method changed in Lang version 2.0. It no longer trims the CharSequence.
                public static boolean isEmpty(final CharSequence cs) {
                    return cs == null || cs.length() == 0;
                }
                "#,
                &[][..],
            ),
            (
                "Explain how fmt turns formatting arguments into type-erased format args and reaches vformat or format_to output paths.",
                test_packet_citation("vformat", "include/fmt/format.h", 0.9),
                "class format_error : public std::runtime_error {}; inline auto vformat(locale_ref loc, string_view fmt, format_args args) -> std::string { detail::vformat_to(buf, fmt, args, loc); return to_string(buf); }",
                &["vformat is the central", "format_error represents"][..],
            ),
            (
                "Trace how Jekyll's build command creates a site and runs the read, generate, render, and write phases.",
                test_packet_citation("Site#process", "lib/jekyll/site.rb", 0.9),
                "class Site\n  def process\n    read\n    generate\n    render\n    write\n  end\nend\n",
                &["Jekyll::Site", "Site#process"][..],
            ),
            (
                "Explain how AutoMapper configuration and runtime mapper APIs cooperate to map source objects to destination objects.",
                test_packet_citation(
                    "MapperConfiguration",
                    "src/AutoMapper/Configuration/MapperConfiguration.cs",
                    0.9,
                ),
                "public sealed class MapperConfiguration { Dictionary<TypePair, TypeMap> _configuredMaps; Dictionary<TypePair, TypeMap> _resolvedMaps; LambdaExpression BuildExecutionPlan(Type sourceType, Type destinationType) => null; }\n",
                &["MapperConfiguration", "Mapper.Map", "TypeMap"][..],
            ),
            (
                "Explain how Okio's Buffer, Source, Sink, and buffered wrappers cooperate to move bytes through reads and writes.",
                test_packet_citation("RealBufferedSource", "okio/RealBufferedSource.kt", 0.9),
                "class RealBufferedSource(val source: Source) { val buffer = Buffer(); override fun read(sink: Buffer, byteCount: Long): Long = source.read(buffer, byteCount) }\n",
                &["RealBufferedSource", "Buffer helpers"][..],
            ),
            (
                "Trace how Alamofire's Session creates requests, resumes tasks, validates data requests, and receives URLSession callbacks.",
                test_packet_citation("DataRequest.validate", "Source/Core/DataRequest.swift", 0.9),
                "public func validate(_ validation: @escaping Validation) -> Self { validators.write { $0.append(validation) }; didValidateRequest() }\n",
                &["Alamofire", "Source/Core", "URLSession"][..],
            ),
            (
                "Explain how package:http exposes top-level helpers, BaseClient convenience methods, BaseRequest finalization, and IOClient send behavior.",
                test_packet_citation("NativeClient", "src/native_client.dart", 0.9),
                "import 'dart:io'; class NativeClient { Future<NativeStreamedResponse> send(BaseRequest request) async { var stream = request.finalize(); var ioRequest = await _inner!.openUrl(request.method, request.url); final response = await stream.pipe(ioRequest) as HttpClientResponse; return NativeStreamedResponse(response); } }\n",
                &["IOClient", "package:http"][..],
            ),
        ];

        for (prompt, citation, source, forbidden_fragments) in cases {
            let claims = packet_source_derived_claims_for_citation(prompt, &citation, source);
            for forbidden in forbidden_fragments {
                assert!(
                    claims.iter().all(|claim| !claim.contains(forbidden)),
                    "production source claims should not include exact benchmark-family fragment `{forbidden}`: {claims:?}"
                );
            }
        }
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
        let _eval_probes = EvalProbesGuard::enabled();
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
                "The top-level request helper opens a session object and delegates to the session request method.",
            ),
            (
                "Session.request",
                "src/requests/sessions.py",
                "def request(self, method, url, **kwargs):\n    req = Request(method=method, url=url)\n    prep = self.prepare_request(req)\n    return self.send(prep, **kwargs)\n",
                "The session request method creates a request object and prepares it into a transport-ready request object.",
            ),
            (
                "PreparedRequest.prepare",
                "src/requests/models.py",
                "def prepare(self):\n    self.prepare_method(method)\n    self.prepare_url(url, params)\n    self.prepare_headers(headers)\n    self.prepare_cookies(cookies)\n    self.prepare_body(data, files, json)\n    self.prepare_auth(auth, url)\n    self.prepare_hooks(hooks)\n",
                "Request preparation builds the method, URL, headers, cookies, body, auth, and hooks.",
            ),
            (
                "PreparedRequest",
                "src/requests/models.py",
                "class PreparedRequest:\n    def prepare(self):\n        self.prepare_method(method)\n        self.prepare_url(url, params)\n        self.prepare_body(data, files, json)\n",
                "Request preparation builds the method, URL, headers, cookies, body, auth, and hooks.",
            ),
            (
                "Session.send",
                "src/requests/sessions.py",
                "def send(self, request, **kwargs):\n    adapter = self.get_adapter(url=request.url)\n    r = adapter.send(request, **kwargs)\n    return r\n",
                "The session send method chooses an adapter and calls the adapter send method.",
            ),
            (
                "BaseAdapter.send",
                "src/requests/adapters.py",
                "class HTTPAdapter:\n    def send(self, request, **kwargs):\n        resp = conn.urlopen(method=request.method, url=url)\n        return self.build_response(request, resp)\n",
                "The transport adapter send path is the response boundary.",
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
            evidence_tier: Some(codestory_contracts::api::PacketEvidenceTierDto::ResolvedGraph),
            evidence_producer: Some("test".to_string()),
            resolution_status: Some(
                codestory_contracts::api::PacketEvidenceResolutionDto::Resolved,
            ),
            loss_reason: None,
            coverage_role: None,
            eligible_for_sufficiency: Some(true),
        };

        assert_eq!(
            packet_evidence_role(&citation),
            Some(PacketEvidenceRole::CommandEntrypoint)
        );
        assert_eq!(
            packet_display_path(citation.file_path.as_deref().unwrap()),
            "crates/tool-cli/src/main.rs"
        );
        assert!(
            packet_claim_for_role(
                "command entrypoint",
                PacketEvidenceRole::CommandEntrypoint,
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
            evidence_tier: Some(codestory_contracts::api::PacketEvidenceTierDto::ResolvedGraph),
            evidence_producer: Some("test".to_string()),
            resolution_status: Some(
                codestory_contracts::api::PacketEvidenceResolutionDto::Resolved,
            ),
            loss_reason: None,
            coverage_role: None,
            eligible_for_sufficiency: Some(true),
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
                    evidence_tier: Some(
                        codestory_contracts::api::PacketEvidenceTierDto::ResolvedGraph,
                    ),
                    evidence_producer: Some("test".to_string()),
                    resolution_status: Some(
                        codestory_contracts::api::PacketEvidenceResolutionDto::Resolved,
                    ),
                    loss_reason: None,
                    coverage_role: None,
                    eligible_for_sufficiency: Some(true),
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
                    evidence_tier: Some(
                        codestory_contracts::api::PacketEvidenceTierDto::ResolvedGraph,
                    ),
                    evidence_producer: Some("test".to_string()),
                    resolution_status: Some(
                        codestory_contracts::api::PacketEvidenceResolutionDto::Resolved,
                    ),
                    loss_reason: None,
                    coverage_role: None,
                    eligible_for_sufficiency: Some(true),
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
