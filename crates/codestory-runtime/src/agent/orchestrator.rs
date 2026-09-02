use crate::agent::citation::{evidence_edge_ids_for_node, to_citation_from_hit};
use crate::agent::packet_batch::{PacketLatencyBudget, run_packet_planned_subqueries};
use crate::agent::packet_budget::{
    apply_packet_budget, enforce_packet_output_budget, packet_budget_limits,
};
use crate::agent::packet_candidate::{
    PacketProofSession, PacketSearchHit, install_packet_proof_session,
};
use crate::agent::packet_compiler::{
    apply_compiled_evidence_for_project, directed_relations_from_graphs, drill_options_from_ids,
};
use crate::agent::packet_degradation::apply_packet_semantic_degradation_counters;
use crate::agent::packet_evidence::decorate_citation_from_hit;
use crate::agent::packet_plan::{
    build_packet_plan_from_seed_plan, build_retrieval_seed_plan, packet_plan_annotation,
};
use crate::agent::packet_probe::{
    exact_packet_probe_citations, exact_packet_probe_paths, normalize_packet_probe_request,
    probes_from_seed_selectors, resolve_packet_probes, unresolved_packet_probe_queries,
};
use crate::agent::packet_scoring::{packet_display_path, packet_stage_citation_carry_limit};
use crate::agent::packet_terms::prompt_search_terms;
use crate::agent::packet_trace::merge_packet_initial_search_hits;
use crate::agent::profiles::{ResolvedProfile, TrailPlan, resolve_profile};
use crate::agent::retrieval_primary::{
    RETRIEVAL_VERSION_SIDECAR, SidecarPrimarySearchOutcome, maybe_run_retrieval_shadow,
    preadmit_packet_descriptor_queries, sidecar_retrieval_blocks_nucleo_supplement,
    sidecar_retrieval_primary_enabled, sidecar_retrieval_unavailable_error,
    try_sidecar_primary_search,
};
use crate::agent::trace::{TraceRecorder, field};
use crate::agent::trace_export;
use crate::{
    AppController, FocusedSourceContext, HybridSearchScoredHit, clamp_u128_to_u32,
    fallback_mermaid as diagnostic_mermaid, hybrid_retrieval_enabled, mermaid_flowchart,
    mermaid_gantt, mermaid_sequence,
};
use codestory_contracts::api::{
    AgentAnswerDto, AgentAskRequest, AgentCitationDto, AgentCustomRetrievalConfigDto,
    AgentHybridWeightsDto, AgentPacketDto, AgentPacketRequestDto, AgentResponseBlockDto,
    AgentResponseModeDto, AgentResponseSectionDto, AgentRetrievalPolicyModeDto,
    AgentRetrievalPresetDto, AgentRetrievalProfileSelectionDto, AgentRetrievalStepDto,
    AgentRetrievalStepKindDto, AgentRetrievalStepStatusDto, ApiError, EdgeKind, GraphArtifactDto,
    GraphNodeDto, GraphRequest, GraphResponse, IndexFreshnessDto, IndexFreshnessStatusDto,
    NodeDetailsDto, NodeDetailsRequest, NodeId, NodeKind, NodeOccurrencesRequest,
    PACKET_DRILL_MAX_DEPTH, PACKET_DRILL_MAX_HITS, PacketBudgetLimitsDto, PacketBudgetModeDto,
    PacketDispositionDto, PacketProbeDto, RetrievalAnnotationDto, RetrievalScoreBreakdownDto,
    SearchHit, SearchHitOrigin, SearchRepoTextMode, SearchRequest, SupportUnitDto,
    SupportUnitKindDto, TrailConfigDto, TrailFilterOptionsDto,
};
use codestory_contracts::compilation::INTERIM_MAX_ADMITTED_CANDIDATES;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
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
    packet_hits: Vec<PacketSearchHit>,
    citations: Vec<AgentCitationDto>,
    graphs: Vec<GraphArtifactDto>,
    focus_node_id: Option<codestory_contracts::api::NodeId>,
    focused_node: Option<NodeDetailsDto>,
    primary_graph: Option<GraphResponse>,
    diagnostic_supplement_used: bool,
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
    agent_ask_with_packet_hits(controller, req).map(|(answer, _)| answer)
}

fn agent_ask_with_packet_hits(
    controller: &AppController,
    req: AgentAskRequest,
) -> Result<(AgentAnswerDto, Vec<PacketSearchHit>), ApiError> {
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
            trace.observe(format!(
                "index_freshness status={:?} duration_ms={} indexed_files={} changed={} new={} removed={}",
                freshness.status,
                freshness.duration_ms,
                freshness.indexed_file_count,
                freshness.changed_file_count,
                freshness.new_file_count,
                freshness.removed_file_count,
            ));
            if let Some(annotation) = stale_freshness_annotation(&freshness) {
                // A stale index means cited evidence may no longer match the working tree.
                trace.annotate_gap(annotation);
            }
            Some(freshness)
        }
        Err(error) => {
            // Freshness could not be established, so index drift is unproven.
            trace.annotate_gap(format!("Index freshness not checked: {}", error.message));
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
        trace.annotate_gap(format!(
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
        // Latency, not evidence: `sla_missed` already carries the confidence consequence.
        trace_payload
            .annotations
            .push(RetrievalAnnotationDto::observation(format!(
                "Completeness-first run exceeded SLA target ({} ms > {} ms).",
                trace_payload.total_latency_ms, target_ms
            )));
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

    let packet_hits = std::mem::take(&mut bundle.packet_hits);
    let answer = AgentAnswerDto {
        source_coverage: Vec::new(),
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
    };
    Ok((answer, packet_hits))
}

pub(crate) fn agent_packet(
    controller: &AppController,
    req: AgentPacketRequestDto,
) -> Result<AgentPacketDto, ApiError> {
    let question = req.question.trim().to_string();
    if question.is_empty() {
        return Err(ApiError::invalid_argument("Question cannot be empty."));
    }
    codestory_contracts::api::validate_packet_probe_request(&req.probes)
        .map_err(ApiError::invalid_argument)?;
    let project_root = controller.require_project_root()?;
    let project_id = codestory_workspace::project_identity_v3(&project_root).project_id;
    controller.begin_packet_retrieval();
    let proof_session = std::rc::Rc::new(PacketProofSession::new());
    let _proof_session_guard = install_packet_proof_session(std::rc::Rc::clone(&proof_session));

    if !req.option_ids.is_empty() && req.parent_packet_id.is_none() {
        return Err(ApiError::invalid_argument(
            "packet option_ids require parent_packet_id for a generation-bound drill",
        ));
    }
    let is_drill_continuation = req.parent_packet_id.is_some() || !req.option_ids.is_empty();
    let free_queries = unresolved_packet_probe_queries(&req.probes);
    let seed_plan = build_retrieval_seed_plan(&question, &free_queries);
    let mut probes = normalize_packet_probe_request(&req.probes);
    for option in drill_options_from_ids(&req.option_ids) {
        if let Some(path) = option.path {
            probes.push(PacketProbeDto::ExactPath { path });
        } else if let Some(symbol_id) = option.symbol_id {
            probes.push(PacketProbeDto::SymbolId { id: symbol_id });
        }
    }
    for probe in probes_from_seed_selectors(&seed_plan.exact_selectors) {
        if !probes.contains(&probe) {
            probes.push(probe);
        }
    }
    let probe_resolutions = resolve_packet_probes(controller, probes);
    let mut plan = build_packet_plan_from_seed_plan(&seed_plan, req.budget);
    plan.probe_resolutions = probe_resolutions;
    let mut descriptor_queries = Vec::new();
    for query in std::iter::once(seed_plan.generic_query.as_str())
        .chain(seed_plan.free_queries.iter().map(String::as_str))
    {
        let query = query.trim();
        if !query.is_empty()
            && !descriptor_queries
                .iter()
                .any(|existing: &String| existing.eq_ignore_ascii_case(query))
        {
            descriptor_queries.push(query.to_string());
        }
    }
    preadmit_packet_descriptor_queries(controller, &descriptor_queries, req.latency_budget_ms)?;
    let exact_probe_citations =
        exact_packet_probe_citations(controller, &plan.probe_resolutions, &question, true);
    let limits = packet_budget_limits_for_request(req.budget, is_drill_continuation);
    let packet_latency = PacketLatencyBudget::new(req.latency_budget_ms);
    let retrieval_profile = packet_retrieval_profile(req.budget, &limits, is_drill_continuation);
    let (mut answer, initial_packet_hits) = agent_ask_with_packet_hits(
        controller,
        AgentAskRequest {
            prompt: question.clone(),
            retrieval_profile,
            focus_node_id: None,
            max_results: Some(
                limits
                    .max_anchors
                    .clamp(1, INTERIM_MAX_ADMITTED_CANDIDATES as u32),
            ),
            response_mode: AgentResponseModeDto::Structured,
            latency_budget_ms: req.latency_budget_ms,
            include_evidence: true,
            hybrid_weights: None,
        },
    )?;
    if !initial_packet_hits.is_empty() {
        let selected = merge_packet_initial_search_hits(
            &mut answer,
            &initial_packet_hits,
            true,
            packet_stage_citation_carry_limit(&limits),
        );
        answer
            .retrieval_trace
            .annotations
            .push(RetrievalAnnotationDto::observation(format!(
                "packet_initial_search_provenance hits={} selected={selected}",
                initial_packet_hits.len()
            )));
    }
    if !exact_probe_citations.is_empty() {
        answer
            .retrieval_trace
            .annotations
            .push(RetrievalAnnotationDto::observation(format!(
                "packet_exact_probe_citations appended={}",
                exact_probe_citations.len()
            )));
        answer.citations.splice(0..0, exact_probe_citations);
    }
    // Plan telemetry echoes prompt-derived query text; it reports the run, not a gap.
    answer
        .retrieval_trace
        .annotations
        .push(RetrievalAnnotationDto::observation(packet_plan_annotation(
            &plan,
        )));
    run_packet_planned_subqueries(
        controller,
        &plan,
        req.budget,
        &limits,
        true,
        packet_latency,
        &mut answer,
    )?;
    let phase_started = Instant::now();
    append_packet_non_trace_phase(&mut answer, "pre_rank_citations", phase_started);
    let phase_started = Instant::now();
    packet_latency.apply_to_trace(&mut answer);
    append_packet_non_trace_phase(&mut answer, "trace_apply", phase_started);

    let phase_started = Instant::now();
    rank_packet_evidence(&mut answer);
    append_packet_non_trace_phase(&mut answer, "rank_and_window", phase_started);

    let phase_started = Instant::now();
    if answer.retrieval_trace.retrieval_shadow.is_none()
        && let Some(shadow) =
            maybe_run_retrieval_shadow(controller, &question, req.latency_budget_ms)
    {
        answer
            .retrieval_trace
            .annotations
            .push(RetrievalAnnotationDto::observation(format!(
                "retrieval_shadow mode={} total_ms={} candidates={} would_rank={}",
                shadow.retrieval_mode,
                shadow.retrieval_total_ms,
                shadow.candidates.len(),
                shadow.would_rank.len()
            )));
        answer.retrieval_trace.retrieval_shadow = Some(shadow);
    }
    append_packet_step_trace_annotation(&mut answer);
    apply_packet_semantic_degradation_counters(&mut answer);
    append_packet_non_trace_phase(&mut answer, "shadow_and_trace", phase_started);

    let exact_probe_paths = exact_packet_probe_paths(&plan.probe_resolutions);

    // Capture admitted repository evidence before presentation-only capping.
    // The pure compiler, rather than legacy graph/citation budgets, decides
    // which source ranges and directed edges enter the public packet.
    let source_support = append_packet_source_evidence(controller, &mut answer);
    let compilation_relations =
        directed_relations_from_graphs(&answer.graphs, &proof_session.receipts());

    // Coverage belongs only to exact selectors and deterministic source reads
    // that survived admission. Prompt-ranked citations have no authority here.
    let mut covered_paths = exact_probe_paths.clone();
    covered_paths.extend(
        source_support
            .iter()
            .filter_map(|source| source.path.clone()),
    );
    answer.source_coverage =
        crate::source_coverage::observe_source_coverage(controller, &covered_paths);
    let retained_coverage_paths = exact_probe_paths
        .into_iter()
        .chain(
            source_support
                .iter()
                .filter_map(|source| source.path.clone()),
        )
        .map(|path| packet_display_path(&path))
        .collect::<HashSet<_>>();
    answer.source_coverage.retain(|observation| {
        retained_coverage_paths.contains(&packet_display_path(&observation.path))
    });

    let phase_started = Instant::now();
    let budget = apply_packet_budget(
        &project_root,
        &question,
        req.budget,
        limits.clone(),
        &mut answer,
    );
    append_packet_non_trace_phase(&mut answer, "budget", phase_started);

    let phase_started = Instant::now();
    append_packet_evidence_sections(&mut answer, &limits);
    if let Some(section) = packet_resolved_relations_section(&answer) {
        answer.sections.push(section);
    }
    order_packet_sections(&mut answer.sections);
    append_packet_non_trace_phase(&mut answer, "evidence_sections", phase_started);

    // `agent_packet` executes inside one stable retrieval-publication scope. Compile the packet
    // while that pin is still active so a one-shot drill carries the exact generations it must
    // send back. Attaching publication only to the returned DTO was too late: the compiler had
    // already emitted empty drill pins, and the continuation could not validate them.
    if let Some(publication) =
        crate::agent::retrieval_primary::active_pinned_retrieval_publication(controller)
    {
        answer.retrieval_trace.retrieval_publication = Some(publication);
    }

    // Typed field, not `annotations`: readiness re-verification was previously
    // invisible, which is what let one packet pay for several full content
    // passes unnoticed. Publishing it here — before the trace summary is taken
    // — reports the passes this packet's own operation performed.
    answer.retrieval_trace.source_freshness_telemetry =
        crate::source_freshness_telemetry_for_operation();

    let phase_started = Instant::now();
    let retrieval_trace_summary = trace_export::packet_retrieval_trace_summary(&answer);
    append_packet_non_trace_phase(&mut answer, "trace_summary", phase_started);

    let phase_started = Instant::now();
    let mut packet = AgentPacketDto {
        packet_id: answer.answer_id.clone(),
        question,
        plan,
        answer,
        support: source_support,
        disposition: PacketDispositionDto::not_established("compile pending"),
        budget,
        retrieval_trace_summary,
        answer_sufficiency: Default::default(),
    };
    append_packet_non_trace_phase(&mut packet.answer, "packet_dto", phase_started);
    enforce_packet_output_budget(&project_root, &mut packet);

    if let Some(diagnostic) = trace_export::write_packet_step_trace_from_env(&packet.answer) {
        // Failing to export the developer step-trace artifact says nothing about packet evidence.
        packet
            .answer
            .retrieval_trace
            .annotations
            .push(RetrievalAnnotationDto::observation(diagnostic));
        enforce_packet_output_budget(&project_root, &mut packet);
    }
    apply_compiled_evidence_for_project(
        &mut packet,
        Some(&req),
        &project_id,
        compilation_relations,
    );
    enforce_packet_output_budget(&project_root, &mut packet);

    Ok(packet)
}

fn packet_budget_limits_for_request(
    budget: PacketBudgetModeDto,
    _is_drill_continuation: bool,
) -> PacketBudgetLimitsDto {
    // Drill retrieval stays depth-bounded in `packet_retrieval_profile`. The
    // compiled packet still advertises the requested budget's public caps so
    // nested and MCP clients can consume a DrillOnce continuation.
    packet_budget_limits(budget)
}

fn append_packet_non_trace_phase(answer: &mut AgentAnswerDto, label: &str, started_at: Instant) {
    answer
        .retrieval_trace
        .annotations
        .push(RetrievalAnnotationDto::observation(
            packet_non_trace_phase_annotation(
                label,
                clamp_u128_to_u32(started_at.elapsed().as_millis()),
            ),
        ));
}

fn packet_non_trace_phase_annotation(label: &str, duration_ms: u32) -> String {
    format!("packet_non_trace_phase label={label} duration_ms={duration_ms}")
}

fn append_packet_step_trace_annotation(answer: &mut AgentAnswerDto) {
    answer
        .retrieval_trace
        .annotations
        .push(RetrievalAnnotationDto::observation(format!(
            "packet_step_trace search_total_ms={} step_count={}",
            trace_export::search_step_total_ms(answer),
            answer.retrieval_trace.steps.len()
        )));
}

fn hybrid_weights_are_lexical_only(weights: Option<&AgentHybridWeightsDto>) -> bool {
    weights
        .and_then(|weights| weights.semantic)
        .is_some_and(|semantic| semantic <= f32::EPSILON)
}

fn rank_packet_evidence(answer: &mut AgentAnswerDto) {
    // Retrieval order is already versioned by the owning sidecar. Packet
    // assembly must not reinterpret it using prompt vocabulary or answer
    // shapes.
    if answer.citations.len() > INTERIM_MAX_ADMITTED_CANDIDATES {
        answer.citations.truncate(INTERIM_MAX_ADMITTED_CANDIDATES);
    }
}

fn append_packet_evidence_sections(answer: &mut AgentAnswerDto, limits: &PacketBudgetLimitsDto) {
    if answer.citations.is_empty() {
        return;
    }

    let ledger_markdown = packet_evidence_ledger_markdown(answer, limits);
    answer.sections.push(AgentResponseSectionDto {
        id: "packet-evidence-ledger".to_string(),
        title: "Packet Evidence Ledger".to_string(),
        blocks: vec![AgentResponseBlockDto::Markdown {
            markdown: ledger_markdown,
        }],
    });
}

pub(crate) const PACKET_RESOLVED_RELATIONS_SECTION_ID: &str = "packet-resolved-relations";

/// The section is bounded so one densely-connected subgraph cannot spend the whole window;
/// the packet's own cap fixpoint still runs after it.
const RESOLVED_RELATIONS_MAX_BYTES: usize = 900;

/// Turn `answer.graphs` into one assertible sentence per resolved relation.
///
/// The index resolves these edges and the packet already carries them in `answer.graphs`,
/// but no section renders them, so they reach a text consumer in no form at all -- a model
/// reading the packet sees the symbols and never sees what connects them. The verb comes
/// from the edge kind and the spelling from the graph; nothing here knows a language or a
/// framework.
fn packet_resolved_relations_section(answer: &AgentAnswerDto) -> Option<AgentResponseSectionDto> {
    let mut markdown = String::new();
    let mut seen = HashSet::new();
    for artifact in &answer.graphs {
        let GraphArtifactDto::Uml { graph, .. } = artifact else {
            continue;
        };
        let nodes = graph
            .nodes
            .iter()
            .map(|node| (&node.id, node))
            .collect::<HashMap<_, _>>();
        for edge in &graph.edges {
            let (Some(source), Some(target)) = (nodes.get(&edge.source), nodes.get(&edge.target))
            else {
                continue;
            };
            if !seen.insert((source.label.as_str(), target.label.as_str())) {
                continue;
            }
            let line = format!(
                "- `{}` ({:?}, {}) {} `{}` ({:?}, {}).\n",
                source.label,
                source.kind,
                packet_relation_path(source),
                packet_relation_verb(edge.kind),
                target.label,
                target.kind,
                packet_relation_path(target),
            );
            if markdown.len() + line.len() > RESOLVED_RELATIONS_MAX_BYTES {
                break;
            }
            markdown.push_str(&line);
        }
    }
    if markdown.is_empty() {
        return None;
    }
    Some(AgentResponseSectionDto {
        id: PACKET_RESOLVED_RELATIONS_SECTION_ID.to_string(),
        title: "Resolved Relations".to_string(),
        blocks: vec![AgentResponseBlockDto::Markdown { markdown }],
    })
}

fn packet_relation_path(node: &GraphNodeDto) -> String {
    node.file_path
        .as_deref()
        .map(packet_display_path)
        .unwrap_or_else(|| "<unknown path>".to_string())
}

/// Reads as a sentence, and says only what the edge kind actually asserts.
fn packet_relation_verb(kind: EdgeKind) -> &'static str {
    match kind {
        EdgeKind::CALL => "calls",
        EdgeKind::IMPORT | EdgeKind::INCLUDE => "imports",
        EdgeKind::INHERITANCE => "inherits from",
        EdgeKind::OVERRIDE => "overrides",
        EdgeKind::MEMBER => "declares",
        EdgeKind::TYPE_USAGE | EdgeKind::TYPE_ARGUMENT => "uses the type",
        EdgeKind::USAGE => "uses",
        EdgeKind::MACRO_USAGE => "expands",
        EdgeKind::ANNOTATION_USAGE => "is annotated by",
        EdgeKind::TEMPLATE_SPECIALIZATION => "specializes",
        EdgeKind::UNKNOWN => "is related to",
    }
}

/// Where each answer section sits in the packet, lowest first.
///
/// Consumers do not read the whole packet. The agent-facing projection carries only the
/// first few thousand characters of the concatenated sections, and the stdio surface caps
/// its compact text the same way, so this order decides what reaches a model at all.
///
/// The evidence ledger repeats structured citation data, so it goes after the
/// bounded source and relation evidence.
///
/// Between those extremes sit the presentational sections -- the analysis preamble, the
/// diagram intros, the per-subquery trace sections. They are worth keeping for a reader of
/// the whole packet and worth nothing to a reader who only ever sees the first few thousand
/// characters, so they rank below the evidence and above the restatement. Ordering rather
/// than dropping them costs a capped reader nothing and costs an uncapped reader nothing.
///
/// Resolved relations and bounded source lead the evidence itself. The
/// retrieval appendix follows and cannot evict compiled repository evidence.
fn packet_section_order_rank(id: &str) -> u8 {
    match id {
        PACKET_RESOLVED_RELATIONS_SECTION_ID => 0,
        "packet-source-evidence" => 1,
        "retrieval-evidence" => 2,
        "packet-evidence-ledger" => 4,
        _ => 3,
    }
}

/// Stable, so sections sharing a rank keep the order their builders produced.
fn order_packet_sections(sections: &mut [AgentResponseSectionDto]) {
    sections.sort_by_key(|section| packet_section_order_rank(&section.id));
}

/// Byte ceiling for the deterministic source-evidence section. Per-row source
/// bytes are bounded by the descriptor reservation contract.
const PACKET_SOURCE_MAX_TOTAL_BYTES: usize = 14_336;

struct PacketSourceRange {
    path: String,
    start_line: u32,
    body: String,
}

/// Truncate on a UTF-8 character boundary, never mid-codepoint.
fn truncate_packet_source(body: &str, max_bytes: usize) -> &str {
    if body.len() <= max_bytes {
        return body;
    }
    let mut end = max_bytes;
    while end > 0 && !body.is_char_boundary(end) {
        end -= 1;
    }
    &body[..end]
}

fn append_packet_source_evidence(
    controller: &AppController,
    answer: &mut AgentAnswerDto,
) -> Vec<SupportUnitDto> {
    if answer.citations.is_empty() {
        return Vec::new();
    }

    let mut rendered = String::new();
    let mut source_support = Vec::new();
    let mut steps = Vec::new();
    for citation in answer
        .citations
        .iter()
        .take(INTERIM_MAX_ADMITTED_CANDIDATES)
    {
        let started = Instant::now();
        let Some(source) = repository_source_range(controller, citation) else {
            continue;
        };
        let retained = truncate_packet_source(
            source.body.trim_end(),
            codestory_contracts::compilation::INTERIM_SOURCE_ROW_UPPER_BOUND,
        );
        if retained.is_empty() {
            continue;
        }
        let entry = format!("### {}\n\n{}\n\n", citation.display_name, retained);
        if rendered.len().saturating_add(entry.len()) > PACKET_SOURCE_MAX_TOTAL_BYTES {
            break;
        }
        rendered.push_str(&entry);
        let path = packet_display_path(&source.path);
        let (start_line, end_line) = source_receipt_line_range(retained, source.start_line);
        source_support.push(SupportUnitDto {
            id: format!("source:{}:{start_line}", citation.node_id.0),
            kind: SupportUnitKindDto::SourceRange,
            summary: citation.display_name.clone(),
            path: Some(path),
            symbol_id: Some(citation.node_id.0.clone()),
            start_line: Some(start_line),
            end_line: Some(end_line),
            snippet: Some(retained.to_string()),
            edge_kind: None,
            from_symbol: None,
            to_symbol: None,
            query: None,
        });
        steps.push(AgentRetrievalStepDto {
            kind: AgentRetrievalStepKindDto::SourceRead,
            status: AgentRetrievalStepStatusDto::Ok,
            duration_ms: started.elapsed().as_millis().try_into().unwrap_or(u32::MAX),
            input: Vec::new(),
            output: Vec::new(),
            message: Some(format!("bounded source for {}", citation.display_name)),
        });
    }

    answer.retrieval_trace.steps.extend(steps);
    if !rendered.is_empty() {
        answer.sections.push(AgentResponseSectionDto {
            id: "packet-source-evidence".to_string(),
            title: "Source Evidence".to_string(),
            blocks: vec![AgentResponseBlockDto::Markdown { markdown: rendered }],
        });
    }
    source_support
}

fn repository_source_range(
    controller: &AppController,
    citation: &AgentCitationDto,
) -> Option<PacketSourceRange> {
    let max_bytes = codestory_contracts::compilation::INTERIM_SOURCE_ROW_UPPER_BOUND;
    if matches!(
        citation.kind,
        NodeKind::FUNCTION | NodeKind::METHOD | NodeKind::CLASS | NodeKind::STRUCT
    ) && let Ok(snippet) = controller.snippet_function_body_context(citation.node_id.clone(), 0)
    {
        if let Some(end_line) = snippet.node.end_line.filter(|end| *end >= snippet.line)
            && let Ok((path, bounded)) = controller.bounded_file_snippet_range(
                &snippet.path,
                crate::BoundedSnippetRangeOptions {
                    focus_line: snippet.line,
                    start_line: snippet.line,
                    end_line,
                    context_lines: 0,
                    max_bytes,
                    truncation_suffix: SOURCE_SNIPPET_TRUNCATION_SUFFIX,
                },
            )
        {
            return Some(PacketSourceRange {
                path,
                start_line: snippet.line,
                body: bounded.markdown,
            });
        }
        return Some(PacketSourceRange {
            path: snippet.path,
            start_line: snippet.line,
            body: truncate_packet_source(&snippet.snippet, max_bytes).to_string(),
        });
    }

    let path = citation.file_path.as_deref()?;
    let line = citation.line?;
    controller
        .bounded_file_snippet(path, line, 4, max_bytes, SOURCE_SNIPPET_TRUNCATION_SUFFIX)
        .ok()
        .map(|(path, snippet)| PacketSourceRange {
            path,
            start_line: line.saturating_sub(4).max(1),
            body: snippet.markdown,
        })
}

fn source_receipt_line_range(markdown: &str, fallback_start: u32) -> (u32, u32) {
    let numbered_lines = markdown.lines().filter_map(|line| {
        let line = line
            .trim_start()
            .strip_prefix("> ")
            .unwrap_or(line.trim_start());
        let (line_number, _) = line.split_once(" | ")?;
        line_number.trim().parse::<u32>().ok()
    });
    let mut start = None;
    let mut end = None;
    for line in numbered_lines {
        start = Some(start.map_or(line, |current: u32| current.min(line)));
        end = Some(end.map_or(line, |current: u32| current.max(line)));
    }
    (
        start.unwrap_or(fallback_start),
        end.unwrap_or(fallback_start),
    )
}

/// A long function is rarely best represented by its prologue. Select at most three bounded,
/// distinct windows whose source words match separate action clauses from the question.
/// The ranges remain exact source receipts for the already selected symbol; this changes only
/// which verified bytes spend the packet's fixed source budget.
fn packet_evidence_ledger_markdown(
    answer: &AgentAnswerDto,
    limits: &PacketBudgetLimitsDto,
) -> String {
    let mut markdown = String::new();
    markdown.push_str("These source-backed anchors retain the owning retrieval order.\n");
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
    // `display_name` is a host path for FILE-kind citations, so it needs the same
    // normalisation as `file_path` -- otherwise the row prints an absolute checkout path
    // next to the relative path it duplicates, and the reader cannot use either as an anchor.
    let name = packet_display_path(&citation.display_name);
    format!(
        "- `{}` ({:?}) - `{}`{} - score {:.3}",
        name, citation.kind, path, line, citation.score
    )
}

fn packet_retrieval_profile(
    budget: PacketBudgetModeDto,
    limits: &PacketBudgetLimitsDto,
    is_drill_continuation: bool,
) -> AgentRetrievalProfileSelectionDto {
    if is_drill_continuation
        || matches!(
            budget,
            PacketBudgetModeDto::Tiny | PacketBudgetModeDto::Compact
        )
    {
        return AgentRetrievalProfileSelectionDto::Custom {
            config: AgentCustomRetrievalConfigDto {
                depth: if is_drill_continuation {
                    PACKET_DRILL_MAX_DEPTH
                } else if matches!(budget, PacketBudgetModeDto::Tiny) {
                    1
                } else {
                    2
                },
                max_nodes: if is_drill_continuation {
                    PACKET_DRILL_MAX_HITS
                        .saturating_mul(PACKET_DRILL_MAX_DEPTH)
                        .clamp(10, 2_000)
                } else {
                    limits.max_trail_edges.clamp(10, 2_000)
                },
                include_edge_occurrences: false,
                enable_source_reads: true,
                ..AgentCustomRetrievalConfigDto::default()
            },
        };
    }

    AgentRetrievalProfileSelectionDto::Preset {
        preset: AgentRetrievalPresetDto::Architecture,
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

fn retain_packet_hits_for_final_hits(
    packet_hits: Vec<PacketSearchHit>,
    final_hits: &[SearchHit],
) -> Vec<PacketSearchHit> {
    let retained_identities = final_hits
        .iter()
        .map(|hit| (hit.node_id.clone(), hit.file_path.clone(), hit.line))
        .collect::<HashSet<_>>();
    packet_hits
        .into_iter()
        .filter(|packet_hit| {
            retained_identities.contains(&(
                packet_hit.hit.node_id.clone(),
                packet_hit.hit.file_path.clone(),
                packet_hit.hit.line,
            ))
        })
        .collect()
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
    let packet_compilation =
        crate::agent::packet_candidate::active_packet_proof_session().is_some();
    let semantic_required =
        hybrid_retrieval_enabled() && !hybrid_weights_are_lexical_only(req.hybrid_weights.as_ref());

    let max_results = req
        .max_results
        .unwrap_or(DEFAULT_MAX_RESULTS)
        .clamp(1, resolved_profile.max_search_results) as usize;

    let (mut scored_hits, hits, initial_packet_hits) =
        match try_sidecar_primary_search(controller, prompt, max_results, req.latency_budget_ms) {
            Some(SidecarPrimarySearchOutcome::Served {
                hits,
                packet_hits,
                scored_hits,
                shadow,
            }) => {
                trace.set_retrieval_shadow(shadow.clone());
                trace.observe(format!(
                    "retrieval_primary mode={} candidates={} resolved_hits={}",
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
                (scored_hits, hits, packet_hits)
            }
            Some(SidecarPrimarySearchOutcome::Rejected { shadow, reason }) => {
                trace.set_retrieval_shadow(shadow);
                trace.annotate_gap(format!(
                    "retrieval_primary rejected=true fail_closed=true reason={reason}"
                ));
                return Err(sidecar_retrieval_unavailable_error(
                    controller,
                    format!("retrieval rejected query: {reason}"),
                ));
            }
            Some(SidecarPrimarySearchOutcome::Unavailable { reason }) => {
                trace.annotate_gap(format!(
                    "retrieval_primary unavailable=true fail_closed=true reason={reason}"
                ));
                return Err(sidecar_retrieval_unavailable_error(controller, reason));
            }
            Some(SidecarPrimarySearchOutcome::Retryable { error }) => return Err(error),
            None => {
                return Err(sidecar_retrieval_unavailable_error(
                    controller,
                    "full retrieval is mandatory; legacy initial search is disabled",
                ));
            }
        };

    let initial_hit_count = hits.len();
    let mut hits = hits;
    let literal_diagnostic_signal = !packet_compilation && has_literal_diagnostic_signal(prompt);
    let promotable_focus_available = !packet_compilation
        && (req.focus_node_id.is_some() || investigation_focus_anchor(prompt, &hits).is_some());
    let mut expansion_added_hits = false;
    let block_nucleo_supplement =
        sidecar_retrieval_blocks_nucleo_supplement(controller, hits.len());
    if !packet_compilation && block_nucleo_supplement && weak_initial_hits(prompt, &hits) {
        trace.annotate_gap(
            "retrieval_primary skipped local nucleo investigation supplement on weak hits",
        );
    }
    if !packet_compilation
        && should_investigate(resolved_profile)
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
                trace.annotate_gap(format!(
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
            trace.annotate_gap(
                "Investigation discarded expansion-only hits for an unanchored natural-language query.",
            );
        }

        if weak_initial_hits(prompt, &hits) && literal_diagnostic_signal {
            trace.annotate_gap(
                "Investigation skipped repo-text diagnostics because packet evidence must come from sidecar-backed resolvable hits or direct source reads.",
            );
        } else if weak_initial_hits(prompt, &hits) {
            if !hits.is_empty() {
                hits.clear();
                scored_hits.clear();
                trace.annotate_gap(
                    "Investigation discarded low-confidence unanchored hits for a natural-language query.",
                );
            }
            trace.annotate_gap(
                "Repo-text diagnostics are disabled for packet evidence; weak unanchored hits were not promoted.",
            );
            trace.annotate_gap("Investigation low confidence gap after sidecar query expansion.");
        }
    } else if !packet_compilation
        && should_investigate(resolved_profile)
        && weak_initial_hits(prompt, &hits)
        && promotable_focus_available
    {
        trace.observe(
            "Investigation kept an explicit or prompt-anchored focus instead of broad diagnostics.",
        );
    }

    let focus_node_id = if packet_compilation {
        req.focus_node_id.clone()
    } else {
        investigation_focus_node(req, prompt, &hits)
    };

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
            trace.annotate_gap(
                "Trail filter options unavailable; continuing with unsanitized filters.",
            );
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
                trace.annotate_gap(
                    "Neighborhood retrieval failed; continuing with trail retrieval.",
                );
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
                        trace.annotate_gap(trail_truncated_annotation(idx + 1, plan.max_nodes));
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
                    trace.annotate_gap(format!("Trail {} failed and was skipped.", idx + 1));
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
        trace.annotate_gap("Latency-first cutoff skipped node occurrence lookups.");
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
                    trace.annotate_gap(format!(
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
        trace.annotate_gap("Latency-first cutoff skipped edge occurrence lookups.");
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
    bundle.packet_hits = retain_packet_hits_for_final_hits(initial_packet_hits, &bundle.hits);
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
        target: scored.hit.target.clone(),
        resolvable: scored.hit.resolvable,
        subgraph_id: subgraph_id.map(ToOwned::to_owned),
        evidence_edge_ids: if include_evidence {
            evidence_edge_ids_for_node(primary_graph, &scored.hit.node_id)
        } else {
            Vec::new()
        },
        retrieval_score_breakdown: include_evidence.then(|| {
            scored
                .hit
                .score_breakdown
                .clone()
                .unwrap_or(RetrievalScoreBreakdownDto {
                    lexical: scored.lexical_score,
                    semantic: scored.semantic_score,
                    graph: scored.graph_score,
                    total: scored.total_score,
                    tier_cap: None,
                    boosts: Vec::new(),
                    dampening: Vec::new(),
                    final_rank_reason: None,
                    provenance: Vec::new(),
                })
        }),
        evidence_tier: scored.hit.evidence_tier,
        evidence_producer: scored.hit.evidence_producer.clone(),
        resolution_status: scored.hit.resolution_status,
        loss_reason: scored.hit.loss_reason.clone(),
        eligible_for_sufficiency: scored.hit.eligible_for_sufficiency,
        source_excerpt: scored.hit.source_excerpt.clone(),
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

fn investigation_focus_node(
    req: &AgentAskRequest,
    prompt: &str,
    hits: &[SearchHit],
) -> Option<NodeId> {
    req.focus_node_id
        .clone()
        .or_else(|| investigation_focus_anchor(prompt, hits))
        .or_else(|| compact_search_flow_executable_focus(req, prompt, hits))
        .or_else(|| {
            hits.iter()
                .find(|hit| hit.resolvable)
                .map(|hit| hit.node_id.clone())
        })
}

fn compact_search_flow_executable_focus(
    req: &AgentAskRequest,
    _prompt: &str,
    hits: &[SearchHit],
) -> Option<NodeId> {
    if !matches!(
        &req.retrieval_profile,
        AgentRetrievalProfileSelectionDto::Custom { .. }
    ) {
        return None;
    }
    let fallback = hits.iter().find(|hit| hit.resolvable)?;
    if !matches!(
        fallback.kind,
        NodeKind::MODULE | NodeKind::NAMESPACE | NodeKind::PACKAGE
    ) {
        return None;
    }
    hits.iter()
        .find(|hit| {
            hit.resolvable
                && hit.origin == SearchHitOrigin::IndexedSymbol
                && matches!(
                    hit.kind,
                    NodeKind::FUNCTION | NodeKind::METHOD | NodeKind::MACRO
                )
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
        trace.annotate_gap("Latency-first cutoff skipped investigation query expansion.");
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
        trace.annotate_gap("Latency-first cutoff skipped source reads.");
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

fn needs_source_context(_prompt: &str) -> bool {
    true
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
        trace.annotate_gap("Latency-first cutoff skipped mermaid synthesis.");
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
    // This section leads the packet, and a capped reader may see nothing else, so it opens
    // with what it found in the repository and closes with how it looked. The question is
    // not restated: whoever called `packet` supplied it, and it is a field on the packet
    // besides -- echoing it back spent the top of the window telling the reader something
    // it had already.
    let mut markdown = String::new();
    let mut provenance = String::new();

    let _ = writeln!(
        provenance,
        "Resolved profile: `{:?}` (`{:?}` mode)",
        profile.preset, profile.policy_mode
    );
    let _ = writeln!(
        provenance,
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

    provenance.push_str("\nWhat I checked:\n");
    provenance.push_str("- Initial indexed-symbol search with current hybrid ranking.\n");
    if bundle.diagnostic_supplement_used {
        provenance.push_str("- Deterministic query expansion because initial hits were weak.\n");
    }
    if !bundle.diagnostic_supplement_used && should_investigate(profile) {
        provenance.push_str("- Initial sidecar hits cleared the investigation confidence gate.\n");
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

    // How the evidence above was gathered, after the evidence itself.
    markdown.push('\n');
    markdown.push_str(&provenance);
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
