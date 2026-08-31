use crate::agent::citation::{evidence_edge_ids_for_node, to_citation_from_hit};
#[cfg(test)]
use crate::agent::eval_probes::source_derived_claims_for_citation as packet_source_derived_claims_for_citation;
use crate::agent::packet_batch::{
    PacketLatencyBudget, packet_anchor_probe_queries, run_packet_planned_subqueries,
};
#[cfg(test)]
use crate::agent::packet_batch::{
    packet_anchor_hit_is_relevant, packet_anchor_probe_limit_for_budget,
};
#[cfg(test)]
use crate::agent::packet_budget::{
    PACKET_MARKDOWN_TRUNCATION_SUFFIX, apply_packet_budget, next_deeper_packet_argv,
    next_deeper_packet_command, truncate_answer_markdown_to_byte_cap,
};
use crate::agent::packet_budget::{
    apply_packet_budget_with_extra_and_obligation_carriers, enforce_packet_output_budget,
    packet_budget_limits,
};
use crate::agent::packet_candidate::{
    PacketProofSession, PacketSearchHit, install_packet_proof_session, packet_atom_hydration_spec,
};
use crate::agent::packet_capping::{
    PACKET_MATERIAL_OWNER_MEMBER_PROBE_ROLE, PACKET_MATERIAL_SCHEMA_ENTITY_ROLE,
};
#[cfg(test)]
use crate::agent::packet_capping::{
    cap_citations, cap_packet_citations_with_obligation_carriers,
    promote_focus_neighborhood_citations, promote_required_probe_citations,
};
use crate::agent::packet_claim_profiles::packet_claim_profile_registry_summary;
#[cfg(test)]
use crate::agent::packet_claims::packet_claim_for_role as build_packet_claim_for_role;
#[cfg(test)]
use crate::agent::packet_claims::packet_supported_claims;
use crate::agent::packet_claims::packet_supported_claims_with_telemetry;
use crate::agent::packet_compiler::{
    apply_compiled_evidence_with_proof_reconciliation, drill_options_from_ids,
};
use crate::agent::packet_degradation::apply_packet_semantic_degradation_counters;
use crate::agent::packet_evidence::decorate_citation_from_hit;
use crate::agent::packet_evidence_carriers::packet_server_dispatch_callable_rank_bonus;
use crate::agent::packet_evidence_roles::{
    PacketEvidenceRole, packet_claim_key_for_citation, packet_evidence_role,
};
#[cfg(test)]
use crate::agent::packet_obligations::build_packet_obligation_plan;
use crate::agent::packet_obligations::{
    PacketProofEvidenceExtras, append_packet_probe_obligations, bind_claims_to_packet_obligations,
    capture_packet_obligation_edge_proofs_before_budget, finalize_packet_obligation_plan,
    install_retained_packet_obligation_edge_proofs,
    packet_claims_with_obligation_receipts_and_telemetry, packet_proof_receipts_view,
    protected_packet_obligation_carrier_node_ids, protected_packet_obligation_edge_ids,
};
#[cfg(test)]
use crate::agent::packet_plan::{
    build_packet_plan, packet_concept_queries, packet_symbol_probe_queries,
};
use crate::agent::packet_plan::{
    build_packet_plan_with_extra, packet_owner_member_probe_queries, packet_plan_annotation,
    packet_rank_terms,
};
use crate::agent::packet_probe::{
    exact_packet_probe_citations, exact_packet_probe_paths, normalize_packet_probe_request,
    resolve_packet_probes, resolved_packet_probe_queries,
};
use crate::agent::packet_required_probes::packet_named_schema_entity_queries;
#[cfg(test)]
use crate::agent::packet_required_probes::packet_sufficiency_required_probe_queries;
#[cfg(test)]
use crate::agent::packet_required_probes::{
    PacketFileScopedSymbolProbe, packet_file_scoped_symbol_probe_parts,
    packet_probe_file_name_matches, packet_probe_query_is_cited,
    packet_sufficiency_required_probe_queries_with_extra,
};
#[cfg(test)]
use crate::agent::packet_scoring::packet_citation_key;
use crate::agent::packet_scoring::{
    normalize_identifier, packet_citation_rank, packet_display_path,
    packet_drop_excess_unrequested_animation_class_siblings,
    packet_drop_excess_unrequested_keyframe_siblings,
    packet_drop_unrequested_animation_file_aliases,
    packet_drop_unrequested_animation_file_only_sheets,
    packet_drop_unrequested_duplicate_client_type_paths,
    packet_drop_unrequested_example_and_binding_siblings,
    packet_drop_unrequested_export_macro_displays,
    packet_drop_unrequested_formatter_specialization_siblings,
    packet_drop_unrequested_formatting_extension_siblings,
    packet_drop_unrequested_mapper_annotation_siblings, packet_drop_unrequested_markdown_siblings,
    packet_drop_unrequested_named_client_adapter_siblings,
    packet_drop_unrequested_non_primary_flow_siblings,
    packet_drop_unrequested_non_stylesheet_animation_siblings,
    packet_drop_unrequested_python_siblings, packet_drop_unrequested_repo_root_stylesheet_siblings,
    packet_drop_unrequested_single_letter_displays,
    packet_drop_unrequested_sql_schema_variant_siblings,
    packet_drop_unrequested_system_format_failure_siblings, packet_drop_unrequested_test_siblings,
    packet_drop_unrequested_wide_char_siblings,
    packet_drop_unrequested_windows_formatting_siblings,
    packet_keep_shared_source_set_over_platform_duplicates, packet_sql_schema_file_is_variant_copy,
    packet_stage_citation_carry_limit, sort_by_cached_rank_desc,
};
use crate::agent::packet_terms::{
    packet_probe_terms, packet_terms_indicate_client_send_flow,
    packet_terms_indicate_mapper_configuration_plan_flow,
    packet_terms_indicate_runtime_formatting_flow, packet_terms_indicate_search_execution_flow,
    packet_terms_indicate_stylesheet_animation_flow, prompt_search_terms,
};
#[cfg(test)]
use crate::agent::packet_terms::{
    packet_terms_have_any, packet_terms_indicate_buffered_io_flow,
    packet_terms_indicate_event_loop_command_flow, packet_terms_indicate_form_validation_flow,
    packet_terms_indicate_hook_cache_flow, packet_terms_indicate_server_route_dispatch_flow,
    packet_terms_indicate_sql_schema_flow, packet_terms_indicate_url_session_request_flow,
};
use crate::agent::packet_trace::merge_packet_initial_search_hits;
use crate::agent::profiles::{ResolvedProfile, TrailPlan, resolve_profile};
use crate::agent::retrieval_primary::{
    RETRIEVAL_VERSION_SIDECAR, SidecarPrimarySearchOutcome,
    hydrate_packet_exact_call_boundaries_post_pass, maybe_run_retrieval_shadow,
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
#[cfg(test)]
use codestory_agent::packet_command::quote_packet_command_value;
use codestory_agent::packet_flow_requirements::{FlowRequirement, packet_material_facet_carriers};
use codestory_agent::packet_proof_atoms::{
    DischargedFact, FlowProofFormula, FlowProofOutcome, ProofAtomId, ProofEndpointPattern,
    ProofFactPattern, SourceAspectKind, TrailCoverage, TrailDirection as ProofTrailDirection,
    VerifiedSourceAspectReceipt, match_flow_requirements,
};
use codestory_agent::text::symbol_query_tokens;
use codestory_contracts::api::{
    AgentAnswerDto, AgentAskRequest, AgentCitationDto, AgentCustomRetrievalConfigDto,
    AgentHybridWeightsDto, AgentPacketDto, AgentPacketRequestDto, AgentResponseBlockDto,
    AgentResponseModeDto, AgentResponseSectionDto, AgentRetrievalPolicyModeDto,
    AgentRetrievalPresetDto, AgentRetrievalProfileSelectionDto, AgentRetrievalStepDto,
    AgentRetrievalStepKindDto, AgentRetrievalStepStatusDto, ApiError, EdgeId, EdgeKind,
    GraphArtifactDto, GraphNodeDto, GraphRequest, GraphResponse, GroundingBudgetDto,
    IndexFreshnessDto, IndexFreshnessStatusDto, NodeDetailsDto, NodeDetailsRequest, NodeId,
    NodeKind, NodeOccurrencesRequest, PACKET_DRILL_MAX_DEPTH, PACKET_DRILL_MAX_HITS,
    PacketBudgetLimitsDto, PacketBudgetModeDto, PacketDispositionDto, PacketEvidenceResolutionDto,
    PacketEvidenceTierDto, PacketObligationPlanDto, PacketPlanDto, PacketProbeDto,
    PacketTaskClassDto, RetrievalAnnotationDto, RetrievalScoreBreakdownDto, SearchHit,
    SearchHitOrigin, SearchRepoTextMode, SearchRequest, SnippetScopeDto,
    SourceCoverageObservationDto, SourceCoverageStatusDto, SupportUnitDto, SupportUnitKindDto,
    TrailConfigDto, TrailFilterOptionsDto,
};
#[cfg(test)]
use codestory_contracts::api::{
    PacketPlanQueryDto, RetrievalAnnotationKindDto, SearchMatchQualityDto,
};
use codestory_contracts::graph::FileCoverageReason;
use std::cmp::Ordering;
#[cfg(test)]
use std::collections::VecDeque;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
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
    codestory_contracts::api::validate_packet_probe_request(&req.probes, &req.extra_probes)
        .map_err(ApiError::invalid_argument)?;
    let project_root = controller.require_project_root()?;
    controller.begin_packet_retrieval();

    if !req.option_ids.is_empty() && req.parent_packet_id.is_none() {
        return Err(ApiError::invalid_argument(
            "packet option_ids require parent_packet_id for a generation-bound drill",
        ));
    }
    let is_drill_continuation = req.parent_packet_id.is_some() || !req.option_ids.is_empty();
    let mut probes = normalize_packet_probe_request(&req.probes, &req.extra_probes);
    for option in drill_options_from_ids(&req.option_ids) {
        if let Some(path) = option.path {
            probes.push(PacketProbeDto::ExactPath { path });
        } else if let Some(symbol_id) = option.symbol_id {
            probes.push(PacketProbeDto::SymbolId { id: symbol_id });
        } else if let Some(query) = option.query {
            probes.push(PacketProbeDto::FreeQuery { query });
        }
    }
    let probe_resolutions = resolve_packet_probes(controller, probes);
    let exact_probe_citations =
        exact_packet_probe_citations(controller, &probe_resolutions, &question, true);
    let extra_probes = resolved_packet_probe_queries(&probe_resolutions);
    let mut plan =
        build_packet_plan_with_extra(&question, req.task_class, req.budget, &extra_probes);
    let task_class = plan.task_class;
    append_packet_probe_obligations(
        &mut plan.obligations,
        &probe_resolutions,
        &question,
        task_class,
    );
    plan.probe_resolutions = probe_resolutions;
    let limits = packet_budget_limits_for_request(req.budget, is_drill_continuation);
    let packet_latency = PacketLatencyBudget::new(req.latency_budget_ms);
    let retrieval_profile = packet_retrieval_profile(
        Some(plan.task_class),
        req.budget,
        &limits,
        is_drill_continuation,
    );
    let retrieval_prompt = packet_retrieval_prompt(&question, &plan, req.budget);
    // R2/R7: the packet proof session scopes the widened atom-trail hydration
    // to this operation and collects the per-artifact trail-scan ledger the
    // proof-evidence extras builder reads. Task classes without
    // formula-bearing requirements derive an empty hydration spec, so their
    // retrieval behavior is unchanged.
    let flow_requirements =
        crate::agent::packet_flow_requirements::packet_flow_requirements_for_terms(
            &packet_probe_terms(&question),
            plan.task_class,
        );
    let proof_session = std::rc::Rc::new(PacketProofSession::new(packet_atom_hydration_spec(
        &flow_requirements,
    )));
    let _proof_session_guard = install_packet_proof_session(std::rc::Rc::clone(&proof_session));
    let (mut answer, initial_packet_hits) = agent_ask_with_packet_hits(
        controller,
        AgentAskRequest {
            prompt: question.clone(),
            retrieval_profile,
            focus_node_id: None,
            max_results: Some(limits.max_anchors.clamp(1, 25)),
            response_mode: AgentResponseModeDto::Structured,
            latency_budget_ms: req.latency_budget_ms,
            include_evidence: true,
            hybrid_weights: None,
        },
    )?;
    let rank_terms = packet_rank_terms(&question);
    if !initial_packet_hits.is_empty() {
        let selected = merge_packet_initial_search_hits(
            &mut answer,
            &initial_packet_hits,
            true,
            &rank_terms,
            packet_stage_citation_carry_limit(&limits),
            &flow_requirements,
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
        answer.citations.extend(exact_probe_citations);
    }
    // Plan telemetry echoes prompt-derived query text; it reports the run, not a gap.
    answer
        .retrieval_trace
        .annotations
        .push(RetrievalAnnotationDto::observation(packet_plan_annotation(
            &plan,
        )));
    if retrieval_prompt != question {
        answer
            .retrieval_trace
            .annotations
            .push(RetrievalAnnotationDto::observation(format!(
                "packet_initial_retrieval raw_question_only=true deferred_planned_probe_chars={}",
                retrieval_prompt.len().saturating_sub(question.len())
            )));
    }
    run_packet_planned_subqueries(
        controller,
        &question,
        &plan,
        req.budget,
        &limits,
        true,
        packet_latency,
        &rank_terms,
        &mut answer,
    )?;
    normalize_packet_citation_paths(&project_root, &mut answer.citations);
    let phase_started = Instant::now();
    append_packet_non_trace_phase(&mut answer, "pre_rank_citations", phase_started);
    let phase_started = Instant::now();
    packet_latency.apply_to_trace(&mut answer);
    append_packet_non_trace_phase(&mut answer, "trace_apply", phase_started);

    let phase_started = Instant::now();
    maybe_append_cited_stylesheet_import_citations(&project_root, &question, &mut answer);
    maybe_append_cited_formatting_type_citations(&project_root, &question, &mut answer);
    maybe_append_cited_mapper_interface_citations(&project_root, &question, &mut answer);
    maybe_append_cited_client_relative_imports(&project_root, &question, &mut answer);
    normalize_packet_citation_paths(&project_root, &mut answer.citations);
    let pre_rank_citations =
        crate::agent::trace_export::packet_step_trace_armed().then(|| answer.citations.clone());
    rank_packet_evidence(&question, &mut answer);
    maybe_annotate_packet_candidate_window(&question, &limits, &mut answer);
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

    let mut sufficiency_extra_probes = packet_plan_sufficiency_extra_probes(&plan, &extra_probes);
    for probe in promote_retained_owner_member_probes(&question, &mut answer) {
        push_packet_sufficiency_extra_probe(&mut sufficiency_extra_probes, &probe);
    }
    promote_retained_schema_entity_probes(&question, &mut answer);
    let exact_probe_paths = exact_packet_probe_paths(&plan.probe_resolutions);
    // R2 post-pass hydration (F3 REVISE): the remaining atom-kind trails and
    // the depth-2 FILE structural trails run HERE, over the retained
    // candidate set, after every sidecar query has finished — never on the
    // sidecar stage clock. This fills the session ledger the extras builder
    // reads below.
    crate::agent::retrieval_primary::hydrate_packet_atom_trails_post_pass(controller, &mut answer);
    hydrate_packet_exact_call_boundaries_post_pass(controller, &flow_requirements, &mut answer);
    // R7: the runtime's verified evidence extras — R2 trail-coverage records
    // and R4 anchored receipts — are threaded through the proving passes.
    // The pre-cap capture proves against the uncapped graphs with coverage
    // only (no anchored receipts exist before the budget's source reread).
    let pre_cap_extras = build_packet_proof_evidence_extras(&answer, &proof_session, Vec::new());
    let phase_started = Instant::now();
    let obligation_edge_proofs = capture_packet_obligation_edge_proofs_before_budget(
        &question,
        plan.task_class,
        &plan.obligations,
        &answer,
        &pre_cap_extras,
    );
    // R3: pre-cap partial-atom matching over the uncapped typed edges and
    // exact-probe resolutions. Provisional anchors let anchor-requiring atoms
    // match structurally (rule 4: provenance decides which receipts get
    // produced), emitting the carrier node ids — including TypedRelation
    // edge-endpoint ids (review-005 finding 10) — and edge ids the caps must
    // protect so the post-cap receipt pass can reach them.
    let partial_protection = packet_partial_atom_protection(
        controller,
        &flow_requirements,
        &answer,
        &pre_cap_extras,
        &proof_session,
    );
    let material_facet_carriers =
        packet_material_facet_carriers(&flow_requirements, &answer.citations);
    let material_facet_node_ids = material_facet_carriers
        .iter()
        .map(|carrier| carrier.node_id.clone())
        .collect::<Vec<_>>();
    let (protected_carrier_node_ids, protected_edge_ids) = select_protected_obligation_carriers(
        &answer,
        &material_facet_node_ids,
        protected_packet_obligation_carrier_node_ids(&obligation_edge_proofs),
        protected_packet_obligation_edge_ids(&obligation_edge_proofs),
        &partial_protection,
    );
    if let Some(diagnostic) =
        crate::agent::packet_accuracy_stage_ledger::record_pre_projection_stages_from_env(
            &plan,
            pre_rank_citations.as_deref(),
            &answer.citations,
            &material_facet_carriers,
            &protected_carrier_node_ids,
            &protected_edge_ids,
        )
    {
        answer
            .retrieval_trace
            .annotations
            .push(RetrievalAnnotationDto::observation(diagnostic));
    }
    let budget = apply_packet_budget_with_extra_and_obligation_carriers(
        &project_root,
        &question,
        plan.task_class,
        req.budget,
        limits.clone(),
        &mut answer,
        &sufficiency_extra_probes,
        &protected_carrier_node_ids,
        &protected_edge_ids,
    );
    append_packet_non_trace_phase(&mut answer, "budget", phase_started);

    // Structural collectors are discovery aids until the runtime rereads the cited source range.
    // Enrich them before final obligation classification so an exact, retained source receipt can
    // prove a structural flow without globally trusting generated structural projections.
    let (source_support, anchored_receipts) = append_packet_carrier_source_sections(
        controller,
        &question,
        &flow_requirements,
        &mut answer,
        &limits,
        &proof_session,
        &protected_carrier_node_ids,
    );
    if let Some(diagnostic) =
        crate::agent::packet_accuracy_stage_ledger::record_source_support_stage_from_env(
            &answer,
            &source_support,
        )
    {
        answer
            .retrieval_trace
            .annotations
            .push(RetrievalAnnotationDto::observation(diagnostic));
    }

    // Coverage belongs to the evidence that survives the real citation cap. Observing the
    // uncapped candidate set made a packet unavailable because of a partial file that the packet
    // did not retain and therefore could not rest a claim on. Exact path probes remain included:
    // a user-named unread path still fails closed even when it yielded no retained citation.
    let mut covered_paths = exact_probe_paths.clone();
    covered_paths.extend(
        answer
            .citations
            .iter()
            .filter(|citation| {
                crate::agent::packet_evidence::citation_sufficiency_eligible(citation)
            })
            .filter_map(|citation| citation.file_path.clone()),
    );
    answer.source_coverage =
        crate::source_coverage::observe_source_coverage(controller, &covered_paths);

    // Parser-partial files are usable only for positive ranges that the runtime actually reread
    // and retained. Demote any citation whose exact receipt missed the snippet/byte cap or failed
    // to read, then drop its now-unused coverage observation. User-requested exact paths stay in
    // the set and therefore continue to fail closed even when they produced no retained citation.
    demote_parser_partial_citations_without_source_receipts(
        Some(&project_root),
        &mut answer.citations,
        &source_support,
        &answer.source_coverage,
    );
    let retained_coverage_paths = exact_probe_paths
        .into_iter()
        .chain(
            answer
                .citations
                .iter()
                .filter(|citation| {
                    crate::agent::packet_evidence::citation_sufficiency_eligible(citation)
                })
                .filter_map(|citation| citation.file_path.clone()),
        )
        .map(|path| packet_project_relative_display_path(Some(&project_root), &path))
        .collect::<HashSet<_>>();
    answer.source_coverage.retain(|observation| {
        retained_coverage_paths.contains(&packet_project_relative_display_path(
            Some(&project_root),
            &observation.path,
        ))
    });

    let phase_started = Instant::now();
    install_retained_packet_obligation_edge_proofs(
        &mut plan.obligations,
        &answer,
        &budget,
        &obligation_edge_proofs,
        limits.max_anchors as usize,
    );
    // The post-cap extras: coverage records rebuilt against the CAPPED
    // graphs (the evidence-completeness obligation is re-checked against
    // exactly the evidence this finalize will see) plus the R4 anchored
    // receipts.
    let proof_evidence_extras =
        build_packet_proof_evidence_extras(&answer, &proof_session, anchored_receipts.clone());
    finalize_packet_obligation_plan(
        &question,
        plan.task_class,
        &mut plan.obligations,
        &answer,
        &budget,
        // The post-cap receipts view: the reread source support plus the live
        // `answer.graphs`. This is the ONE proving site (R7(b)); later
        // rebuilds re-verify by receipt survival against the manifest this
        // finalize records.
        &source_support,
        &proof_evidence_extras,
    );
    append_packet_evidence_sections(
        &mut answer,
        plan.task_class,
        &limits,
        Some(&plan.obligations),
    );
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
        task_class: Some(plan.task_class),
        plan,
        answer,
        support: source_support,
        disposition: PacketDispositionDto::not_established("compile pending"),
        budget,
        retrieval_trace_summary,
    };
    append_packet_non_trace_phase(&mut packet.answer, "packet_dto", phase_started);
    enforce_packet_output_budget(&project_root, &mut packet);

    if let Some(diagnostic) =
        trace_export::write_packet_step_trace_from_env(&packet.answer, &packet.plan.obligations)
    {
        // Failing to export the developer step-trace artifact says nothing about packet evidence.
        packet
            .answer
            .retrieval_trace
            .annotations
            .push(RetrievalAnnotationDto::observation(diagnostic));
        enforce_packet_output_budget(&project_root, &mut packet);
    }
    // R5 re-proves with the same anchored receipts the primary finalize used,
    // while the coverage records are rebuilt by the SAME builder against the
    // packet answer as it stands here: the evidence-completeness obligation
    // binds at every view build, so a scan whose enumerated edges were
    // dropped by the output-budget passes between finalize and compile must
    // not stay attached (a stale completeness claim could let an absence
    // fact rebind over evidence the matcher can no longer see).
    let reconcile_extras =
        build_packet_proof_evidence_extras(&packet.answer, &proof_session, anchored_receipts);
    apply_compiled_evidence_with_proof_reconciliation(&mut packet, Some(&req), &reconcile_extras);

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

fn packet_plan_sufficiency_extra_probes(
    plan: &PacketPlanDto,
    _explicit_extra_probes: &[String],
) -> Vec<String> {
    let mut probes = Vec::new();
    for query in &plan.queries {
        if !query.purpose.contains("explicit")
            && packet_plan_query_can_gate_sufficiency(&query.query)
        {
            push_packet_sufficiency_extra_probe(&mut probes, &query.query);
        }
    }
    probes
}

fn promote_retained_owner_member_probes(
    question: &str,
    answer: &mut AgentAnswerDto,
) -> Vec<String> {
    let probes = packet_owner_member_probe_queries(question, &answer.citations, 10)
        .into_iter()
        .filter(|probe| {
            let probe = normalize_identifier(probe);
            answer.citations.iter().any(|citation| {
                citation.origin == SearchHitOrigin::IndexedSymbol
                    && citation.resolvable
                    && citation.eligible_for_sufficiency == Some(true)
                    && normalize_identifier(&citation.display_name) == probe
            })
        })
        .collect::<Vec<_>>();
    let probe_keys = probes
        .iter()
        .map(|probe| normalize_identifier(probe))
        .collect::<HashSet<_>>();
    for citation in &mut answer.citations {
        if citation.origin == SearchHitOrigin::IndexedSymbol
            && citation.resolvable
            && citation.eligible_for_sufficiency == Some(true)
            && probe_keys.contains(&normalize_identifier(&citation.display_name))
            && citation.coverage_role.is_none()
        {
            citation.coverage_role = Some(PACKET_MATERIAL_OWNER_MEMBER_PROBE_ROLE.to_string());
        }
    }
    probes
}

fn promote_retained_schema_entity_probes(question: &str, answer: &mut AgentAnswerDto) {
    let entities = packet_named_schema_entity_queries(question);
    if entities.is_empty() {
        return;
    }
    for entity in &entities {
        let table_key = normalize_identifier(&entity.replace(' ', ""));
        if table_key.len() < 4 {
            continue;
        }
        let catalog_key = normalize_identifier(&format!("public.{entity}").replace(' ', ""));
        let matching = answer
            .citations
            .iter()
            .enumerate()
            .filter(|(_, citation)| {
                citation.origin == SearchHitOrigin::IndexedSymbol
                    && citation.resolvable
                    && citation
                        .evidence_producer
                        .as_deref()
                        .is_some_and(|producer| producer.contains("structural_sql"))
                    && {
                        let display = normalize_identifier(&citation.display_name);
                        display == catalog_key || sql_catalog_table_key(&display) == table_key
                    }
            })
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        let Some(&best_index) = matching.iter().max_by(|left, right| {
            let left = &answer.citations[**left];
            let right = &answer.citations[**right];
            sql_schema_dialect_rank(left.file_path.as_deref().unwrap_or_default())
                .total_cmp(&sql_schema_dialect_rank(
                    right.file_path.as_deref().unwrap_or_default(),
                ))
                .then_with(|| left.score.total_cmp(&right.score))
        }) else {
            continue;
        };
        answer.citations[best_index].coverage_role =
            Some(PACKET_MATERIAL_SCHEMA_ENTITY_ROLE.to_string());
        for index in matching {
            let table_token = {
                let display = &answer.citations[index].display_name;
                display
                    .rsplit(['.', ' ', '/', '\\'])
                    .next()
                    .unwrap_or(display)
                    .trim()
                    .to_string()
            };
            if table_token.is_empty() || normalize_identifier(&table_token).contains("createtable")
            {
                continue;
            }
            let alias_name = format!("CREATE TABLE {table_token}");
            if answer.citations[index].display_name != alias_name {
                answer.citations[index].display_name = alias_name;
            }
        }
    }
    promote_sql_schema_dialect_files(answer);
    promote_sql_schema_relationship_constraints(answer);
}

fn sql_schema_dialect_rank(path: &str) -> f32 {
    let lower = packet_display_path(path).to_ascii_lowercase();
    if lower.contains("sqlite") {
        4.0
    } else if lower.contains("mysql") || lower.contains("postgres") || lower.contains("postgresql")
    {
        3.0
    } else if lower.contains("sqlserver") {
        1.0
    } else {
        0.0
    }
}

fn promote_sql_schema_dialect_files(answer: &mut AgentAnswerDto) {
    let mut file_identities = Vec::new();
    for marker in ["sqlite", "mysql", "postgres"] {
        let best = answer
            .citations
            .iter()
            .enumerate()
            .filter(|(_, citation)| {
                citation.file_path.as_deref().is_some_and(|path| {
                    let display = packet_display_path(path).to_ascii_lowercase();
                    display.ends_with(".sql")
                        && display.contains(marker)
                        && !packet_sql_schema_file_is_variant_copy(path)
                })
            })
            .max_by(|(_, left), (_, right)| left.score.total_cmp(&right.score))
            .map(|(index, _)| index);
        let Some(index) = best else {
            continue;
        };
        if answer.citations[index].coverage_role.is_none() {
            answer.citations[index].coverage_role =
                Some(PACKET_MATERIAL_SCHEMA_ENTITY_ROLE.to_string());
        }
        let Some(path) = answer.citations[index].file_path.as_deref() else {
            continue;
        };
        let relative = packet_display_path(path);
        if relative.is_empty()
            || answer
                .citations
                .iter()
                .any(|citation| citation.display_name == relative)
            || file_identities
                .iter()
                .any(|citation: &AgentCitationDto| citation.display_name == relative)
        {
            continue;
        }
        let source = &answer.citations[index];
        file_identities.push(AgentCitationDto {
            node_id: NodeId(format!(
                "packet::sql_schema_dialect_file::{}::{relative}",
                source.node_id.0
            )),
            display_name: relative,
            kind: NodeKind::FILE,
            file_path: source.file_path.clone(),
            line: source.line,
            score: source.score.max(20.0),
            origin: SearchHitOrigin::TextMatch,
            target: None,
            resolvable: false,
            subgraph_id: source.subgraph_id.clone(),
            evidence_edge_ids: Vec::new(),
            retrieval_score_breakdown: source.retrieval_score_breakdown.clone(),
            evidence_tier: source.evidence_tier,
            evidence_producer: Some("packet_sql_schema_dialect_file".to_string()),
            resolution_status: source.resolution_status,
            loss_reason: None,
            coverage_role: Some(PACKET_MATERIAL_SCHEMA_ENTITY_ROLE.to_string()),
            eligible_for_sufficiency: Some(false),
            source_excerpt: None,
        });
    }
    answer.citations.extend(file_identities);
}

fn citation_path_is_retained_sql_dialect(path: &str) -> bool {
    let display = packet_display_path(path).to_ascii_lowercase();
    display.ends_with(".sql")
        && (display.contains("sqlite") || display.contains("mysql") || display.contains("postgres"))
        && !packet_sql_schema_file_is_variant_copy(path)
}

fn promote_sql_schema_relationship_constraints(answer: &mut AgentAnswerDto) {
    let retains_common_dialects = answer.citations.iter().any(|citation| {
        citation
            .file_path
            .as_deref()
            .is_some_and(citation_path_is_retained_sql_dialect)
    });
    if !retains_common_dialects {
        return;
    }
    for citation in &mut answer.citations {
        if citation.coverage_role.is_some()
            || packet_evidence_role(citation) != Some(PacketEvidenceRole::SqlRelationshipConstraint)
            || citation
                .file_path
                .as_deref()
                .is_some_and(packet_sql_schema_file_is_variant_copy)
        {
            continue;
        }
        citation.coverage_role = Some(PACKET_MATERIAL_SCHEMA_ENTITY_ROLE.to_string());
    }
}

fn sql_catalog_table_key(normalized_display: &str) -> String {
    let without_create = normalized_display
        .strip_prefix("createtable")
        .unwrap_or(normalized_display);
    without_create
        .strip_prefix("public")
        .unwrap_or(without_create)
        .to_string()
}

struct CitedSourceShapeCitation<'a> {
    path: &'a Path,
    display_name: &'a str,
    kind: NodeKind,
    line: u32,
    score: f32,
    coverage_role: &'a str,
    producer: &'a str,
}

fn push_cited_source_shape_citation(
    answer: &mut AgentAnswerDto,
    project_root: &Path,
    citation: CitedSourceShapeCitation<'_>,
) -> bool {
    let display_name = citation.display_name.trim();
    if display_name.is_empty() {
        return false;
    }
    let path_string = citation.path.to_string_lossy().to_string();
    let relative = citation
        .path
        .strip_prefix(project_root)
        .map(|stripped| packet_display_path(&stripped.to_string_lossy()))
        .unwrap_or_else(|_| packet_display_path(&path_string));
    if answer.citations.iter().any(|existing| {
        existing.display_name == display_name
            && existing.file_path.as_deref().is_some_and(|existing_path| {
                packet_display_path(existing_path) == packet_display_path(&path_string)
                    || packet_display_path(existing_path) == relative
            })
    }) {
        return false;
    }
    answer.citations.push(AgentCitationDto {
        node_id: NodeId(format!(
            "packet::cited_source::{}::{relative}::{display_name}",
            citation.producer
        )),
        display_name: display_name.to_string(),
        kind: citation.kind,
        file_path: Some(path_string),
        line: Some(citation.line),
        score: citation.score,
        origin: SearchHitOrigin::TextMatch,
        target: None,
        resolvable: false,
        subgraph_id: None,
        evidence_edge_ids: Vec::new(),
        retrieval_score_breakdown: None,
        evidence_tier: Some(PacketEvidenceTierDto::SyntheticSourceScan),
        evidence_producer: Some(citation.producer.to_string()),
        resolution_status: Some(PacketEvidenceResolutionDto::SourceRangeOnly),
        loss_reason: None,
        coverage_role: Some(citation.coverage_role.to_string()),
        eligible_for_sufficiency: Some(true),
        source_excerpt: None,
    });
    true
}

fn maybe_append_cited_stylesheet_import_citations(
    project_root: &Path,
    question: &str,
    answer: &mut AgentAnswerDto,
) {
    let terms = packet_probe_terms(question);
    if !packet_terms_indicate_stylesheet_animation_flow(&terms) {
        return;
    }
    let cited_css = answer
        .citations
        .iter()
        .filter_map(|citation| citation.file_path.as_deref())
        .filter(|path| {
            packet_display_path(path)
                .to_ascii_lowercase()
                .ends_with(".css")
        })
        .map(|path| path.to_string())
        .collect::<Vec<_>>();
    if cited_css.is_empty() {
        return;
    }

    let mut appended = 0usize;
    for cited in &cited_css {
        if appended >= 6 {
            break;
        }
        let source_path = if Path::new(cited).is_absolute() {
            PathBuf::from(cited)
        } else {
            project_root.join(cited)
        };
        let Ok(source) = std::fs::read_to_string(&source_path) else {
            continue;
        };
        if source.len() > 1_500_000 {
            continue;
        }
        let parent = source_path.parent().unwrap_or(project_root);
        for import in packet_css_relative_imports(&source).into_iter().take(8) {
            if appended >= 6 {
                break;
            }
            let imported = parent.join(&import);
            let relative = imported
                .strip_prefix(project_root)
                .unwrap_or(&imported)
                .to_string_lossy()
                .replace('\\', "/");
            if crate::retrieval_file_role_from_path(&relative).is_non_primary() {
                continue;
            }
            if answer.citations.iter().any(|existing| {
                existing.file_path.as_deref().is_some_and(|existing_path| {
                    packet_display_path(existing_path) == packet_display_path(&relative)
                })
            }) {
                continue;
            }
            let Ok(imported_source) = std::fs::read_to_string(&imported) else {
                continue;
            };
            let lower = imported_source.to_ascii_lowercase();
            let is_keyframe = lower.contains("@keyframes");
            let is_vars = lower.contains(":root") && lower.contains("--");
            if !is_keyframe && !is_vars {
                continue;
            }
            let (display_name, line) = if is_keyframe {
                packet_first_css_at_rule_display(&imported_source, "@keyframes")
                    .unwrap_or_else(|| ("@keyframes".to_string(), 1))
            } else {
                packet_first_css_custom_property_display(&imported_source)
                    .unwrap_or_else(|| ("css custom property".to_string(), 1))
            };
            let class_anchor = is_keyframe
                .then(|| packet_first_css_class_display(&imported_source))
                .flatten();
            answer.citations.push(AgentCitationDto {
                node_id: NodeId(format!(
                    "packet::css_import::{}::{display_name}",
                    packet_display_path(&relative)
                )),
                display_name,
                kind: NodeKind::CONSTANT,
                file_path: Some(imported.to_string_lossy().to_string()),
                line: Some(line),
                score: if is_keyframe { 48.0 } else { 44.0 },
                origin: SearchHitOrigin::TextMatch,
                target: None,
                resolvable: false,
                subgraph_id: None,
                evidence_edge_ids: Vec::new(),
                retrieval_score_breakdown: None,
                evidence_tier: Some(
                    codestory_contracts::api::PacketEvidenceTierDto::SyntheticSourceScan,
                ),
                evidence_producer: Some("packet_cited_stylesheet_import".to_string()),
                resolution_status: Some(
                    codestory_contracts::api::PacketEvidenceResolutionDto::SourceRangeOnly,
                ),
                loss_reason: None,
                coverage_role: Some(if is_keyframe {
                    "css keyframes".to_string()
                } else {
                    "css animation variables".to_string()
                }),
                eligible_for_sufficiency: Some(true),
                source_excerpt: None,
            });
            if is_keyframe {
                push_cited_source_shape_citation(
                    answer,
                    project_root,
                    CitedSourceShapeCitation {
                        path: &imported,
                        display_name: &relative,
                        kind: NodeKind::FILE,
                        line,
                        score: 47.0,
                        coverage_role: "css animation source file",
                        producer: "packet_cited_stylesheet_import",
                    },
                );
            }
            if let Some((class_name, class_line)) = class_anchor {
                let already = answer.citations.iter().any(|existing| {
                    existing.display_name == class_name
                        && existing.file_path.as_deref().is_some_and(|path| {
                            packet_display_path(path) == packet_display_path(&relative)
                        })
                });
                if !already {
                    answer.citations.push(AgentCitationDto {
                        node_id: NodeId(format!(
                            "packet::css_import_class::{}::{class_name}",
                            packet_display_path(&relative)
                        )),
                        display_name: class_name,
                        kind: NodeKind::CONSTANT,
                        file_path: Some(imported.to_string_lossy().to_string()),
                        line: Some(class_line),
                        score: 46.0,
                        origin: SearchHitOrigin::TextMatch,
                        target: None,
                        resolvable: false,
                        subgraph_id: None,
                        evidence_edge_ids: Vec::new(),
                        retrieval_score_breakdown: None,
                        evidence_tier: Some(
                            codestory_contracts::api::PacketEvidenceTierDto::SyntheticSourceScan,
                        ),
                        evidence_producer: Some("packet_cited_stylesheet_import".to_string()),
                        resolution_status: Some(
                            codestory_contracts::api::PacketEvidenceResolutionDto::SourceRangeOnly,
                        ),
                        loss_reason: None,
                        coverage_role: Some("css animation selector".to_string()),
                        eligible_for_sufficiency: Some(true),
                        source_excerpt: None,
                    });
                }
            }
            appended = appended.saturating_add(1);
        }
    }
    maybe_append_cited_stylesheet_entry_sheets(project_root, answer);
}

fn packet_stylesheet_path_is_animation_base(path: &Path) -> bool {
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("");
    let normalized = normalize_identifier(stem);
    normalized == "base"
        || normalized.ends_with("base")
        || normalized == "vars"
        || normalized.ends_with("vars")
}

fn maybe_append_cited_stylesheet_entry_sheets(project_root: &Path, answer: &mut AgentAnswerDto) {
    let mut cited_names = Vec::new();
    let mut parent_dirs = Vec::new();
    for citation in &answer.citations {
        let Some(path) = citation.file_path.as_deref() else {
            continue;
        };
        if !packet_display_path(path)
            .to_ascii_lowercase()
            .ends_with(".css")
        {
            continue;
        }
        let source_path = if Path::new(path).is_absolute() {
            PathBuf::from(path)
        } else {
            project_root.join(path)
        };
        if !packet_stylesheet_path_is_animation_base(&source_path) {
            continue;
        }
        if let Some(name) = source_path.file_name().and_then(|name| name.to_str())
            && !cited_names.iter().any(|existing| existing == name)
        {
            cited_names.push(name.to_string());
        }
        if let Some(parent) = source_path.parent()
            && !parent_dirs.iter().any(|existing| existing == parent)
        {
            parent_dirs.push(parent.to_path_buf());
        }
    }
    if cited_names.is_empty() {
        return;
    }

    let mut appended = 0usize;
    for parent in parent_dirs {
        if appended >= 2 {
            break;
        }
        let Ok(entries) = std::fs::read_dir(&parent) else {
            continue;
        };
        let mut css_files = entries
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("css"))
            })
            .collect::<Vec<_>>();
        css_files.sort();
        css_files.truncate(32);
        for candidate in css_files {
            if appended >= 2 {
                break;
            }
            let relative = candidate
                .strip_prefix(project_root)
                .unwrap_or(&candidate)
                .to_string_lossy()
                .replace('\\', "/");
            if crate::retrieval_file_role_from_path(&relative).is_non_primary() {
                continue;
            }
            if answer.citations.iter().any(|existing| {
                existing.file_path.as_deref().is_some_and(|existing_path| {
                    packet_display_path(existing_path) == packet_display_path(&relative)
                })
            }) {
                continue;
            }
            let Ok(source) = std::fs::read_to_string(&candidate) else {
                continue;
            };
            if source.len() > 1_500_000 {
                continue;
            }
            if source.to_ascii_lowercase().contains("@keyframes") {
                continue;
            }
            let imports = packet_css_relative_imports(&source);
            if imports.len() < 2 {
                continue;
            }
            let imports_cited = imports.iter().any(|spec| {
                let spec_name = spec.rsplit(['/', '\\']).next().unwrap_or(spec);
                cited_names.iter().any(|cited| cited == spec_name)
            });
            if !imports_cited {
                continue;
            }
            if push_cited_source_shape_citation(
                answer,
                project_root,
                CitedSourceShapeCitation {
                    path: &candidate,
                    display_name: &relative,
                    kind: NodeKind::FILE,
                    line: 1,
                    score: 45.0,
                    coverage_role: "css animation source file",
                    producer: "packet_cited_stylesheet_entry",
                },
            ) {
                appended = appended.saturating_add(1);
            }
        }
    }
}

fn packet_css_relative_imports(source: &str) -> Vec<String> {
    let mut imports = Vec::new();
    for line in source.lines() {
        let trimmed = line.trim();
        let lower = trimmed.to_ascii_lowercase();
        if !lower.starts_with("@import") {
            continue;
        }
        let Some(start) = trimmed.find(['\'', '"']) else {
            continue;
        };
        let quote = trimmed.as_bytes()[start];
        let rest = &trimmed[start + 1..];
        let Some(end) = rest.as_bytes().iter().position(|ch| *ch == quote) else {
            continue;
        };
        let spec = &rest[..end];
        if spec.is_empty()
            || spec.contains("://")
            || spec.starts_with('/')
            || spec.starts_with("url(")
        {
            continue;
        }
        if spec.rsplit_once('.').is_some_and(|(_, ext)| {
            !matches!(ext.to_ascii_lowercase().as_str(), "css" | "scss" | "sass")
        }) {
            continue;
        }
        if !imports.iter().any(|existing| existing == spec) {
            imports.push(spec.to_string());
        }
    }
    imports
}

fn packet_first_css_at_rule_display(source: &str, rule: &str) -> Option<(String, u32)> {
    for (index, line) in source.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.to_ascii_lowercase().starts_with(rule) {
            let name = trimmed
                .split(['{', ';'])
                .next()
                .unwrap_or(trimmed)
                .trim()
                .to_string();
            return Some((name, index.saturating_add(1).try_into().unwrap_or(u32::MAX)));
        }
    }
    None
}

fn packet_first_css_custom_property_display(source: &str) -> Option<(String, u32)> {
    for (index, line) in source.lines().enumerate() {
        let trimmed = line.trim();
        let Some(start) = trimmed.find("--") else {
            continue;
        };
        let name = trimmed[start..]
            .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '-' || ch == '_'))
            .next()
            .unwrap_or_default();
        if name.len() > 2 {
            return Some((
                name.to_string(),
                index.saturating_add(1).try_into().unwrap_or(u32::MAX),
            ));
        }
    }
    None
}

fn packet_first_css_class_display(source: &str) -> Option<(String, u32)> {
    for (index, line) in source.lines().enumerate() {
        let trimmed = line.trim();
        if !trimmed.starts_with('.') {
            continue;
        }
        let name = trimmed
            .split(['{', ',', ':', ' ', '\t'])
            .next()
            .unwrap_or(trimmed)
            .trim();
        if name.len() > 1 && name.starts_with('.') {
            return Some((
                name.to_string(),
                index.saturating_add(1).try_into().unwrap_or(u32::MAX),
            ));
        }
    }
    None
}

fn maybe_append_cited_formatting_type_citations(
    project_root: &Path,
    question: &str,
    answer: &mut AgentAnswerDto,
) {
    let terms = packet_probe_terms(question);
    if !packet_terms_indicate_runtime_formatting_flow(&terms) {
        return;
    }
    let cited_paths = cited_source_paths_with_extensions(
        answer,
        project_root,
        &["h", "hpp", "hh", "cc", "cpp", "cxx"],
    );
    let mut appended = 0usize;
    for path in cited_paths {
        if appended >= 6 {
            break;
        }
        let Ok(source) = std::fs::read_to_string(&path) else {
            continue;
        };
        if source.len() > 1_500_000 {
            continue;
        }
        for (name, kind, line, argument_store) in packet_cited_formatting_types(&source) {
            if appended >= 6 {
                break;
            }
            let coverage_role = if argument_store {
                "runtime format argument store"
            } else {
                "runtime formatting failure type"
            };
            if push_cited_source_shape_citation(
                answer,
                project_root,
                CitedSourceShapeCitation {
                    path: &path,
                    display_name: &name,
                    kind,
                    line,
                    score: if argument_store { 46.0 } else { 44.0 },
                    coverage_role,
                    producer: "packet_cited_formatting_type",
                },
            ) {
                appended = appended.saturating_add(1);
            }
        }
    }
}

fn maybe_append_cited_mapper_interface_citations(
    project_root: &Path,
    question: &str,
    answer: &mut AgentAnswerDto,
) {
    let terms = packet_probe_terms(question);
    if !packet_terms_indicate_mapper_configuration_plan_flow(&terms) {
        return;
    }
    let cited_paths = cited_source_paths_with_extensions(answer, project_root, &["cs"]);
    let mut appended = 0usize;
    for path in cited_paths {
        if appended >= 4 {
            break;
        }
        let Ok(source) = std::fs::read_to_string(&path) else {
            continue;
        };
        if source.len() > 1_500_000 {
            continue;
        }
        for (name, line) in packet_cited_mapper_interface_names(&source) {
            if appended >= 4 {
                break;
            }
            if push_cited_source_shape_citation(
                answer,
                project_root,
                CitedSourceShapeCitation {
                    path: &path,
                    display_name: &name,
                    kind: NodeKind::INTERFACE,
                    line,
                    score: 45.0,
                    coverage_role: "mapper public api",
                    producer: "packet_cited_mapper_interface",
                },
            ) {
                appended = appended.saturating_add(1);
            }
        }
    }
}

fn maybe_append_cited_client_relative_imports(
    project_root: &Path,
    question: &str,
    answer: &mut AgentAnswerDto,
) {
    let terms = packet_probe_terms(question);
    if !packet_terms_indicate_client_send_flow(&terms) {
        return;
    }
    let cited_paths = cited_source_paths_with_extensions(answer, project_root, &["dart"]);
    let mut pending = Vec::new();
    for cited in cited_paths {
        let Ok(source) = std::fs::read_to_string(&cited) else {
            continue;
        };
        if source.len() > 1_500_000 {
            continue;
        }
        let parent = cited.parent().unwrap_or(project_root);
        let mut imports = packet_dart_relative_imports(&source);
        imports.sort_by_key(|spec| packet_client_relative_import_priority(spec));
        for import in imports {
            if !packet_client_relative_import_stem(&import) {
                continue;
            }
            let imported = parent.join(&import);
            if !imported.is_file() {
                continue;
            }
            let relative = imported
                .strip_prefix(project_root)
                .unwrap_or(&imported)
                .to_string_lossy()
                .replace('\\', "/");
            if crate::retrieval_file_role_from_path(&relative).is_non_primary() {
                continue;
            }
            let Ok(imported_source) = std::fs::read_to_string(&imported) else {
                continue;
            };
            let Some((display_name, line)) = packet_dart_declared_type_name(&imported_source)
            else {
                continue;
            };
            pending.push((
                packet_client_relative_import_priority(&import),
                imported,
                relative,
                display_name,
                line,
            ));
        }
    }
    pending.sort_by(|left, right| {
        left.0
            .cmp(&right.0)
            .then_with(|| left.2.cmp(&right.2))
            .then_with(|| left.3.cmp(&right.3))
    });
    pending.dedup_by(|left, right| left.1 == right.1);
    let mut appended = 0usize;
    for (priority, imported, relative, display_name, line) in pending {
        if appended >= 6 {
            break;
        }
        let exact_stem = priority == 0;
        if push_cited_source_shape_citation(
            answer,
            project_root,
            CitedSourceShapeCitation {
                path: &imported,
                display_name: &display_name,
                kind: NodeKind::CLASS,
                line,
                score: if exact_stem { 50.0 } else { 42.0 },
                coverage_role: "client imported type",
                producer: "packet_cited_client_import",
            },
        ) {
            let _ = push_cited_source_shape_citation(
                answer,
                project_root,
                CitedSourceShapeCitation {
                    path: &imported,
                    display_name: &relative,
                    kind: NodeKind::FILE,
                    line,
                    score: if exact_stem { 49.0 } else { 41.0 },
                    coverage_role: "client imported source file",
                    producer: "packet_cited_client_import",
                },
            );
            appended = appended.saturating_add(1);
        }
    }
}

fn cited_source_paths_with_extensions(
    answer: &AgentAnswerDto,
    project_root: &Path,
    extensions: &[&str],
) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for citation in &answer.citations {
        let Some(raw) = citation.file_path.as_deref() else {
            continue;
        };
        let path = if Path::new(raw).is_absolute() {
            PathBuf::from(raw)
        } else {
            project_root.join(raw)
        };
        let extension = path
            .extension()
            .and_then(|extension| extension.to_str())
            .map(|extension| extension.to_ascii_lowercase())
            .unwrap_or_default();
        if !extensions.iter().any(|expected| extension == *expected) {
            continue;
        }
        if paths.iter().any(|existing| existing == &path) {
            continue;
        }
        paths.push(path);
    }
    for node in answer
        .graphs
        .iter()
        .filter_map(|artifact| match artifact {
            GraphArtifactDto::Uml { graph, .. } => Some(&graph.nodes),
            GraphArtifactDto::Mermaid { .. } => None,
        })
        .flatten()
    {
        let Some(raw) = node.file_path.as_deref() else {
            continue;
        };
        let path = if Path::new(raw).is_absolute() {
            PathBuf::from(raw)
        } else {
            project_root.join(raw)
        };
        let extension = path
            .extension()
            .and_then(|extension| extension.to_str())
            .map(|extension| extension.to_ascii_lowercase())
            .unwrap_or_default();
        if extensions.iter().any(|expected| extension == *expected)
            && !paths.iter().any(|existing| existing == &path)
        {
            paths.push(path);
        }
    }
    paths
}

fn packet_cited_formatting_types(source: &str) -> Vec<(String, NodeKind, u32, bool)> {
    let mut types = Vec::new();
    for (index, line) in source.lines().enumerate() {
        if packet_source_line_is_comment(line) {
            continue;
        }
        let Some((name, kind)) = packet_cpp_declared_type_name(line) else {
            continue;
        };
        let normalized = normalize_identifier(&name);
        let argument_store = normalized.contains("format")
            && (normalized.contains("arg") || normalized.contains("argument"))
            && normalized.contains("store");
        let failure_type = normalized.contains("format")
            && (normalized.contains("error")
                || normalized.contains("failure")
                || normalized.contains("exception"))
            && !normalized.contains("windows")
            && !normalized.contains("system")
            && !normalized.contains("duration");
        if !argument_store && !failure_type {
            continue;
        }
        types.push((
            name,
            kind,
            index.saturating_add(1).try_into().unwrap_or(u32::MAX),
            argument_store,
        ));
    }
    types
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
        if ch == '_' || ch.is_ascii_alphanumeric() {
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

fn packet_source_line_is_comment(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("//") || trimmed.starts_with('#') || trimmed.starts_with('*')
}

fn packet_cited_mapper_interface_names(source: &str) -> Vec<(String, u32)> {
    let mut names = Vec::new();
    for (index, line) in source.lines().enumerate() {
        if packet_source_line_is_comment(line) {
            continue;
        }
        let Some(after) = packet_text_after_keyword(line, "interface") else {
            continue;
        };
        for token in packet_identifier_tokens(after) {
            let normalized = normalize_identifier(&token);
            if normalized.is_empty()
                || matches!(
                    normalized.as_str(),
                    "public" | "private" | "protected" | "internal" | "partial"
                )
                || !normalized.contains("mapper")
            {
                continue;
            }
            names.push((
                token,
                index.saturating_add(1).try_into().unwrap_or(u32::MAX),
            ));
            break;
        }
    }
    names
}

fn packet_dart_relative_imports(source: &str) -> Vec<String> {
    let mut imports = Vec::new();
    for line in source.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with("import ") {
            continue;
        }
        let Some(start) = trimmed.find(['\'', '"']) else {
            continue;
        };
        let quote = trimmed.as_bytes()[start];
        let rest = &trimmed[start + 1..];
        let Some(end) = rest.as_bytes().iter().position(|ch| *ch == quote) else {
            continue;
        };
        let spec = &rest[..end];
        if spec.is_empty() || spec.contains(':') || spec.starts_with('/') {
            continue;
        }
        if !spec.to_ascii_lowercase().ends_with(".dart") {
            continue;
        }
        if !imports.iter().any(|existing| existing == spec) {
            imports.push(spec.to_string());
        }
    }
    imports
}

fn packet_client_relative_import_stem(spec: &str) -> bool {
    let file_name = spec.rsplit(['/', '\\']).next().unwrap_or(spec);
    let stem = file_name
        .rsplit_once('.')
        .map(|(stem, _)| stem)
        .unwrap_or(file_name);
    let normalized = normalize_identifier(stem);
    (normalized.contains("client")
        || normalized.contains("request")
        || normalized.contains("response"))
        && !normalized.contains("stub")
        && !normalized.contains("test")
}

fn packet_client_relative_import_priority(spec: &str) -> u8 {
    let file_name = spec.rsplit(['/', '\\']).next().unwrap_or(spec);
    let stem = file_name
        .rsplit_once('.')
        .map(|(stem, _)| stem)
        .unwrap_or(file_name);
    let normalized = normalize_identifier(stem);
    if matches!(normalized.as_str(), "client" | "request" | "response") {
        0
    } else if normalized.ends_with("client") {
        1
    } else {
        2
    }
}

fn packet_dart_declared_type_name(source: &str) -> Option<(String, u32)> {
    for (index, line) in source.lines().enumerate() {
        if packet_source_line_is_comment(line) {
            continue;
        }
        let Some(after) = packet_text_after_keyword(line, "class") else {
            continue;
        };
        for token in packet_identifier_tokens(after) {
            let normalized = normalize_identifier(&token);
            if normalized.is_empty()
                || matches!(
                    normalized.as_str(),
                    "abstract" | "interface" | "base" | "final" | "sealed" | "mixin"
                )
            {
                continue;
            }
            return Some((
                token,
                index.saturating_add(1).try_into().unwrap_or(u32::MAX),
            ));
        }
    }
    None
}

fn packet_plan_query_can_gate_sufficiency(query: &str) -> bool {
    packet_probe_terms(query)
        .iter()
        .any(|term| term.len() >= 4 && !crate::agent::packet_scoring::packet_query_stop_term(term))
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
    answer
        .retrieval_trace
        .annotations
        .push(RetrievalAnnotationDto::observation(format!(
            "packet_step_trace search_total_ms={} step_count={}",
            trace_export::search_step_total_ms(answer),
            answer.retrieval_trace.steps.len()
        )));
}

fn packet_retrieval_prompt(
    question: &str,
    plan: &PacketPlanDto,
    budget: PacketBudgetModeDto,
) -> String {
    let anchor_probes = packet_anchor_probe_queries(plan);
    if plan.queries.len() <= 1 {
        return question.to_string();
    }
    let mut prompt = String::from(question);
    prompt.push_str("\n\nPlanned CodeStory queries:");
    let compact = matches!(
        budget,
        PacketBudgetModeDto::Compact | PacketBudgetModeDto::Tiny
    );
    let planned_lines = if compact {
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

fn hybrid_weights_are_lexical_only(weights: Option<&AgentHybridWeightsDto>) -> bool {
    weights
        .and_then(|weights| weights.semantic)
        .is_some_and(|semantic| semantic <= f32::EPSILON)
}

fn rank_packet_evidence(question: &str, answer: &mut AgentAnswerDto) {
    let terms = packet_rank_terms(question);
    let prefer_primary_sources = !query_mentions_non_primary_source(question);
    sort_by_cached_rank_desc(&mut answer.citations, |citation| {
        packet_citation_rank(citation, &terms, prefer_primary_sources)
            + packet_server_dispatch_callable_rank_bonus(citation, &terms)
    });
    packet_drop_unrequested_wide_char_siblings(&mut answer.citations, &terms);
    packet_drop_unrequested_python_siblings(&mut answer.citations, &terms);
    packet_drop_unrequested_windows_formatting_siblings(&mut answer.citations, &terms);
    packet_drop_unrequested_formatting_extension_siblings(&mut answer.citations, &terms);
    packet_drop_unrequested_formatter_specialization_siblings(&mut answer.citations, &terms);
    packet_drop_unrequested_export_macro_displays(&mut answer.citations, &terms);
    packet_drop_unrequested_system_format_failure_siblings(&mut answer.citations, &terms);
    packet_drop_unrequested_single_letter_displays(&mut answer.citations, &terms);
    packet_drop_unrequested_named_client_adapter_siblings(&mut answer.citations, &terms);
    packet_drop_unrequested_duplicate_client_type_paths(&mut answer.citations, &terms);
    packet_drop_unrequested_example_and_binding_siblings(&mut answer.citations, &terms);
    packet_drop_unrequested_mapper_annotation_siblings(&mut answer.citations, &terms);
    packet_drop_unrequested_test_siblings(&mut answer.citations, &terms);
    packet_keep_shared_source_set_over_platform_duplicates(&mut answer.citations, &terms);
    packet_drop_unrequested_sql_schema_variant_siblings(&mut answer.citations, &terms);
    packet_drop_unrequested_non_primary_flow_siblings(&mut answer.citations, &terms);
    packet_drop_excess_unrequested_keyframe_siblings(&mut answer.citations, &terms);
    packet_drop_excess_unrequested_animation_class_siblings(&mut answer.citations, &terms);
    packet_drop_unrequested_animation_file_aliases(&mut answer.citations, &terms);
    packet_drop_unrequested_animation_file_only_sheets(&mut answer.citations, &terms);
    packet_drop_unrequested_non_stylesheet_animation_siblings(&mut answer.citations, &terms);
    packet_drop_unrequested_repo_root_stylesheet_siblings(&mut answer.citations, &terms);
    packet_drop_unrequested_markdown_siblings(&mut answer.citations, &terms);
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
    // Rows echo citation display names and paths; ranking telemetry is never an evidence gap.
    answer
        .retrieval_trace
        .annotations
        .push(RetrievalAnnotationDto::observation(format!(
            "packet_candidate_trace filter=`{}` candidates={} matched={} max_anchors={} rows={}",
            filter.replace('`', "'"),
            answer.citations.len(),
            matched,
            limits.max_anchors,
            rows.join(" | ")
        )));
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
    task_class: PacketTaskClassDto,
    limits: &PacketBudgetLimitsDto,
    obligations: Option<&PacketObligationPlanDto>,
) {
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

    let (mut claims, claim_telemetry) = if let Some(obligations) = obligations {
        let supported_claims_with_telemetry = packet_supported_claims_with_telemetry(answer);
        packet_claims_with_obligation_receipts_and_telemetry(
            answer,
            task_class,
            obligations,
            supported_claims_with_telemetry,
        )
    } else {
        packet_supported_claims_with_telemetry(answer)
    };
    if let Some(obligations) = obligations {
        bind_claims_to_packet_obligations(obligations, &mut claims);
    }
    // Typed field, not `annotations`: annotations are scanned as evidence by packet consumers,
    // so always-on telemetry published there is read as a permanent gap and downgrades every
    // packet's confidence.
    answer.retrieval_trace.packet_claim_profile_telemetry =
        Some(claim_telemetry.to_dto(packet_claim_profile_registry_summary()));
    // Claims stay in structured sufficiency. Do not publish English flow-catalog
    // sentences as agent-facing packet conclusions.
    let _ = claims;
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
/// The evidence ledger and the claims list are renderings of `answer.citations` and
/// `sufficiency.covered_claims`, which every consumer already receives as structured fields
/// outside that cut. Leading with them spent the entire window restating data the reader
/// held already and evicted the retrieval evidence and carrier source, which are sent
/// nowhere else. They stay in the packet -- they are the anchor list for consumers that
/// render sections only -- but they go last.
///
/// Between those extremes sit the presentational sections -- the analysis preamble, the
/// diagram intros, the per-subquery trace sections. They are worth keeping for a reader of
/// the whole packet and worth nothing to a reader who only ever sees the first few thousand
/// characters, so they rank below the evidence and above the restatement. Ordering rather
/// than dropping them costs a capped reader nothing and costs an uncapped reader nothing.
///
/// Resolved relations and carrier source lead the evidence itself. They state assertible
/// connections and source text for the selected anchors. The retrieval appendix follows:
/// its focused-symbol preamble and raw top matches are useful diagnostics, but can describe
/// a neighboring symbol and must not evict the compiled relation/source evidence from a
/// capped agent projection.
fn packet_section_order_rank(id: &str) -> u8 {
    match id {
        PACKET_RESOLVED_RELATIONS_SECTION_ID => 0,
        "packet-carrier-source" => 1,
        "retrieval-evidence" => 2,
        "packet-flow-claims" => 4,
        "packet-evidence-ledger" => 5,
        _ => 3,
    }
}

/// Stable, so sections sharing a rank keep the order their builders produced.
fn order_packet_sections(sections: &mut [AgentResponseSectionDto]) {
    sections.sort_by_key(|section| packet_section_order_rank(&section.id));
}

/// Byte ceilings for the carrier-source section. The packet's own output cap is the real
/// constraint and the cap fixpoint still runs after this; these only stop one long function
/// body from spending the whole snippet allowance before the rest of the evidence is placed.
const CARRIER_SOURCE_MAX_TOTAL_BYTES: usize = 14_336;
const CARRIER_SOURCE_MAX_SNIPPET_BYTES: usize = 1_536;
const CARRIER_SOURCE_FOCUSED_SNIPPET_BYTES: usize = CARRIER_SOURCE_MAX_SNIPPET_BYTES / 2;
const CARRIER_SOURCE_FOCUS_CONTEXT_LINES: usize = 5;
const CARRIER_SOURCE_FILE_FOCUS_CONTEXT_LINES: usize = 17;
const CARRIER_SOURCE_MAX_FOCUSED_WINDOWS: usize = 3;
const CARRIER_SOURCE_MAX_FOCUS_FILE_BYTES: u64 = 512 * 1_024;
const CARRIER_SOURCE_DECLARATION_FALLBACK_LINES: u32 = 24;
/// R4 anchored windows span the declaration line plus this many lines below
/// it, so the receipt-carried line is exactly the window start.
const ATOM_ANCHOR_WINDOW_LINES: u32 = 12;
const SOURCE_RECEIPT_NOT_RETAINED: &str = "source_receipt_not_retained";

struct CarrierSourceRange {
    path: String,
    start_line: u32,
    body: String,
}

fn carrier_source_line_context_end_line(scope: SnippetScopeDto, start_line: u32) -> Option<u32> {
    (scope == SnippetScopeDto::LineContext).then(|| {
        start_line.saturating_add(CARRIER_SOURCE_DECLARATION_FALLBACK_LINES.saturating_sub(1))
    })
}

/// Truncate on a UTF-8 character boundary, never mid-codepoint.
fn truncate_carrier_source(body: &str, max_bytes: usize) -> &str {
    if body.len() <= max_bytes {
        return body;
    }
    let mut end = max_bytes;
    while end > 0 && !body.is_char_boundary(end) {
        end -= 1;
    }
    &body[..end]
}

/// Attach each material carrier's own source to the packet.
///
/// The packet is the agent's only view of the repository — the packet-first contract holds
/// it to zero source reads afterwards — and `budget.limits.max_snippets` has always reserved
/// room for exactly this. Nothing ever filled it: every packet shipped `snippets: 0` against
/// an allowance of twelve. So a packet named a carrier, cited its file and line, and asserted
/// a receipt about it, while the code itself stayed unreadable at answering time. What a
/// carrier says about itself — its doc comment, the call it makes, the prototype it mixes in —
/// is evidence the graph cannot restate, and it was being dropped on the floor.
fn citation_is_lexical_source_range(citation: &AgentCitationDto) -> bool {
    citation.evidence_tier == Some(PacketEvidenceTierDto::LexicalSource)
        && citation.resolution_status == Some(PacketEvidenceResolutionDto::SourceRangeOnly)
}

fn citation_is_structural_source_range(citation: &AgentCitationDto) -> bool {
    citation.evidence_tier == Some(PacketEvidenceTierDto::StructuralText)
        && citation
            .evidence_producer
            .as_deref()
            .is_some_and(|producer| producer.starts_with("structural_"))
}

fn citation_is_internal_synthetic_source_range(citation: &AgentCitationDto) -> bool {
    citation.evidence_tier == Some(PacketEvidenceTierDto::SyntheticSourceScan)
        && citation.resolution_status == Some(PacketEvidenceResolutionDto::SourceRangeOnly)
        && citation
            .evidence_producer
            .as_deref()
            .is_some_and(|producer| producer.starts_with("packet_"))
}

fn citation_needs_bounded_source_read(citation: &AgentCitationDto) -> bool {
    citation_is_structural_source_range(citation)
        || citation_is_lexical_source_range(citation)
        || citation_is_internal_synthetic_source_range(citation)
}

/// Upgrade only a CodeStory-owned source-shape lead after its bounded source read succeeded.
fn promote_verified_bounded_source_citation(citation: &mut AgentCitationDto) -> bool {
    if !citation_is_structural_source_range(citation)
        && !citation_is_internal_synthetic_source_range(citation)
    {
        return false;
    }
    let producer = citation
        .evidence_producer
        .as_deref()
        .unwrap_or("source_shape_collector");
    citation.evidence_tier = Some(PacketEvidenceTierDto::ExactSource);
    citation.evidence_producer = Some(format!("verified_{producer}_source_read"));
    citation.resolution_status = Some(PacketEvidenceResolutionDto::SourceRangeOnly);
    citation.eligible_for_sufficiency = Some(true);
    true
}

fn normalize_packet_citation_paths(project_root: &Path, citations: &mut [AgentCitationDto]) {
    for citation in citations {
        let Some(path) = citation.file_path.as_deref() else {
            continue;
        };
        citation.file_path = Some(packet_project_relative_display_path(
            Some(project_root),
            path,
        ));
    }
}

fn packet_project_relative_display_path(project_root: Option<&Path>, path: &str) -> String {
    if let Some(project_root) = project_root {
        let candidate = Path::new(path);
        if let Ok(relative) = candidate.strip_prefix(project_root)
            && !relative.as_os_str().is_empty()
        {
            return relative.to_string_lossy().replace('\\', "/");
        }
    }
    packet_display_path(path)
}

fn demote_parser_partial_citations_without_source_receipts(
    project_root: Option<&Path>,
    citations: &mut [AgentCitationDto],
    source_support: &[SupportUnitDto],
    source_coverage: &[SourceCoverageObservationDto],
) {
    let parser_partial_paths = source_coverage
        .iter()
        .filter(|observation| {
            observation.status == SourceCoverageStatusDto::Incomplete
                && observation.reason == Some(FileCoverageReason::ParserPartial)
        })
        .map(|observation| packet_project_relative_display_path(project_root, &observation.path))
        .collect::<HashSet<_>>();
    let retained_receipts = source_support
        .iter()
        .filter(|unit| unit.kind == SupportUnitKindDto::SourceRange)
        .filter_map(|unit| {
            Some((
                unit.symbol_id.clone()?,
                packet_project_relative_display_path(project_root, unit.path.as_deref()?),
            ))
        })
        .collect::<HashSet<_>>();
    for citation in citations.iter_mut().filter(|citation| {
        citation.file_path.as_deref().is_some_and(|path| {
            parser_partial_paths.contains(&packet_project_relative_display_path(project_root, path))
        })
    }) {
        let has_receipt = citation.file_path.as_deref().is_some_and(|path| {
            retained_receipts.contains(&(
                citation.node_id.0.clone(),
                packet_project_relative_display_path(project_root, path),
            ))
        });
        if !has_receipt {
            citation.eligible_for_sufficiency = Some(false);
            citation
                .loss_reason
                .get_or_insert_with(|| SOURCE_RECEIPT_NOT_RETAINED.to_string());
        }
    }
}

/// Builds the caller-supplied proof-evidence extras from the session's
/// trail-scan ledger and the CURRENT live answer (R2/R7). This is the ONE
/// builder for every proving site — the pre-cap capture, R3's partial
/// matching, R4's anchor planning, the primary finalize, and the R5
/// reconcile all go through it, so the enrichment path cannot fork.
///
/// Evidence-completeness obligation (binding rule 7, enforced HERE): a
/// `TrailCoverage::Scanned` record is attached — to `trail_scans` or to an
/// edge via `edge_coverage` — only when EVERY edge in the scan's NARROWED
/// coverage set (the absence-subject edges plus the depth-2 MEMBER witnesses;
/// see `PacketCandidateTrailScan`) is present in the live `answer.graphs` the
/// view will be built from. A scan whose coverage set lost an edge to a cap
/// is dropped entirely, and every absence fact over it fails closed instead
/// of certifying over receipts the matcher cannot see; an incidental
/// enumerated edge of another kind lost to a cap does NOT void the coverage
/// (F3 finding 3 — truncation covers reach, the witness receipts cover
/// membership, the covered-kind edges are the absence subject). Edge coverage
/// is attached from untruncated scans only (a truncated scan never witnesses
/// anything); the first scan in ledger order wins per edge, deterministically.
fn build_packet_proof_evidence_extras(
    answer: &AgentAnswerDto,
    session: &PacketProofSession,
    anchored_receipts: Vec<VerifiedSourceAspectReceipt>,
) -> PacketProofEvidenceExtras {
    let mut extras = PacketProofEvidenceExtras {
        anchored_receipts,
        ..PacketProofEvidenceExtras::default()
    };
    let ledger = session.artifact_scans();
    if ledger.is_empty() {
        return extras;
    }
    let mut live_artifact_ids = HashSet::new();
    let mut live_edge_ids = HashSet::new();
    for artifact in &answer.graphs {
        let GraphArtifactDto::Uml { id, graph, .. } = artifact else {
            continue;
        };
        live_artifact_ids.insert(id.as_str());
        for edge in &graph.edges {
            live_edge_ids.insert(edge.id.clone());
        }
    }
    for (artifact_id, scans) in &ledger {
        if !live_artifact_ids.contains(artifact_id.as_str()) {
            continue;
        }
        for scan in scans {
            if !scan
                .coverage_edge_ids
                .iter()
                .all(|edge_id| live_edge_ids.contains(edge_id))
            {
                continue;
            }
            let coverage = TrailCoverage::Scanned {
                root: NodeId(scan.root.clone()),
                traversal_kinds: scan.edge_kinds.clone(),
                direction: match scan.direction {
                    crate::agent::packet_candidate::PacketGraphDirection::Outgoing => {
                        ProofTrailDirection::Outgoing
                    }
                    crate::agent::packet_candidate::PacketGraphDirection::Incoming => {
                        ProofTrailDirection::Incoming
                    }
                },
                depth: scan.depth,
                truncated: scan.truncated,
            };
            if !scan.truncated {
                for edge_id in &scan.coverage_edge_ids {
                    extras
                        .edge_coverage
                        .entry(edge_id.clone())
                        .or_insert_with(|| coverage.clone());
                }
            }
            extras.trail_scans.push(coverage);
        }
    }
    extras
}

/// One planned R4 anchor: the atom that names the carrier, the window-owning
/// node, the line-carrying node, and the receipt-carried declaration line the
/// window must start at. The provisional receipt is planning input only
/// (rule 4: provenance decides which receipts get produced; discharge happens
/// exclusively against real reread receipts).
#[derive(Debug, Clone, PartialEq, Eq)]
struct PlannedAtomAnchor {
    atom: ProofAtomId,
    owner: NodeId,
    symbol: NodeId,
    line: u32,
    receipt: VerifiedSourceAspectReceipt,
}

/// Derives the provisional anchored receipts for every anchor-requiring fact
/// of the given formulas, from the live graphs plus storage declaration
/// lines. The derivation is identity-only and reads the formula structure —
/// for a `require_atom_anchor` carrier range, the sibling typed-relation fact
/// that binds the same role names the node kind and edge kind that identify
/// candidate nodes; for an anchored containment, the sibling fact binding the
/// line role names the (window owner → line carrier) edge shape. Nodes
/// without a storage declaration line are skipped (they could never be
/// anchored honestly).
fn planned_atom_anchor_candidates(
    controller: &AppController,
    answer: &AgentAnswerDto,
    formulas: &[&'static FlowProofFormula],
) -> Vec<PlannedAtomAnchor> {
    let Ok(storage) = controller.open_storage() else {
        return Vec::new();
    };
    let mut declaration_lines: HashMap<String, Option<u32>> = HashMap::new();
    planned_atom_anchor_candidates_with_lines(answer, formulas, |node_id| {
        *declaration_lines
            .entry(node_id.0.clone())
            .or_insert_with(|| {
                let core_id = node_id.0.parse::<i64>().ok()?;
                storage
                    .get_node(codestory_contracts::graph::NodeId(core_id))
                    .ok()
                    .flatten()
                    .and_then(|node| node.start_line)
            })
    })
}

/// Controller-free core of the anchor planning, parameterized on the
/// declaration-line lookup (storage in production, a map in tests).
fn planned_atom_anchor_candidates_with_lines(
    answer: &AgentAnswerDto,
    formulas: &[&'static FlowProofFormula],
    mut declaration_line: impl FnMut(&NodeId) -> Option<u32>,
) -> Vec<PlannedAtomAnchor> {
    let mut node_kinds: HashMap<&str, NodeKind> = HashMap::new();
    let mut live_edges: Vec<&codestory_contracts::api::GraphEdgeDto> = Vec::new();
    for artifact in &answer.graphs {
        let GraphArtifactDto::Uml { graph, .. } = artifact else {
            continue;
        };
        for node in &graph.nodes {
            node_kinds.entry(node.id.0.as_str()).or_insert(node.kind);
        }
        live_edges.extend(graph.edges.iter());
    }
    let mut planned: Vec<PlannedAtomAnchor> = Vec::new();
    let push_planned = |planned: &mut Vec<PlannedAtomAnchor>, anchor: PlannedAtomAnchor| {
        if !planned.iter().any(|existing| {
            existing.atom == anchor.atom
                && existing.owner == anchor.owner
                && existing.symbol == anchor.symbol
        }) {
            planned.push(anchor);
        }
    };
    for formula in formulas {
        for atom in formula.atoms {
            for fact in atom.facts {
                match fact {
                    ProofFactPattern::SourceAspect(pattern) if pattern.require_atom_anchor => {
                        let ProofEndpointPattern::Role(symbol_role) = pattern.symbol else {
                            continue;
                        };
                        // The sibling typed-relation fact that binds the same
                        // role names the identifying edge shape.
                        let Some(shape) = atom.facts.iter().find_map(|sibling| match sibling {
                            ProofFactPattern::TypedRelation(relation)
                                if relation.target == ProofEndpointPattern::Role(symbol_role) =>
                            {
                                Some(relation)
                            }
                            _ => None,
                        }) else {
                            continue;
                        };
                        for edge in &live_edges {
                            if edge.kind != shape.kind {
                                continue;
                            }
                            if let Some(required_kind) = shape.target_kind
                                && node_kinds.get(edge.target.0.as_str()).copied()
                                    != Some(required_kind)
                            {
                                continue;
                            }
                            let Some(line) = declaration_line(&edge.target) else {
                                continue;
                            };
                            push_planned(
                                &mut planned,
                                PlannedAtomAnchor {
                                    atom: atom.id,
                                    owner: edge.target.clone(),
                                    symbol: edge.target.clone(),
                                    line,
                                    receipt: VerifiedSourceAspectReceipt {
                                        kind: SourceAspectKind::VerifiedCarrierRange,
                                        owner: edge.target.clone(),
                                        symbol_id: Some(edge.target.clone()),
                                        start_line: Some(line),
                                        end_line: Some(line),
                                        atom_anchor: Some(atom.id),
                                    },
                                },
                            );
                        }
                    }
                    ProofFactPattern::AnchoredLineContainment(pattern) => {
                        let Some(shape) = atom.facts.iter().find_map(|sibling| match sibling {
                            ProofFactPattern::TypedRelation(relation)
                                if relation.target
                                    == ProofEndpointPattern::Role(pattern.line_symbol)
                                    && relation.source
                                        == ProofEndpointPattern::Role(pattern.window_owner) =>
                            {
                                Some(relation)
                            }
                            _ => None,
                        }) else {
                            continue;
                        };
                        for edge in &live_edges {
                            if edge.kind != shape.kind {
                                continue;
                            }
                            if let Some(required_kind) = shape.target_kind
                                && node_kinds.get(edge.target.0.as_str()).copied()
                                    != Some(required_kind)
                            {
                                continue;
                            }
                            let Some(line) = declaration_line(&edge.target) else {
                                continue;
                            };
                            push_planned(
                                &mut planned,
                                PlannedAtomAnchor {
                                    atom: atom.id,
                                    owner: edge.source.clone(),
                                    symbol: edge.target.clone(),
                                    line,
                                    receipt: VerifiedSourceAspectReceipt {
                                        kind: pattern.kind,
                                        owner: edge.source.clone(),
                                        symbol_id: Some(edge.target.clone()),
                                        start_line: Some(line),
                                        end_line: Some(line),
                                        atom_anchor: Some(atom.id),
                                    },
                                },
                            );
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    planned
}

/// The distinct formulas behind the packet's flow requirements, in
/// requirement order.
fn packet_flow_proof_formulas(
    flow_requirements: &[FlowRequirement],
) -> Vec<&'static FlowProofFormula> {
    let mut formulas: Vec<&'static FlowProofFormula> = Vec::new();
    for requirement in flow_requirements {
        if let Some(formula) = requirement.proof.formula()
            && !formulas
                .iter()
                .any(|existing| std::ptr::eq(*existing, formula))
        {
            formulas.push(formula);
        }
    }
    formulas
}

/// R3 output: the carriers and edges partially matched atoms need, plus the
/// per-carrier atom cover the selection step's weighted set cover reads.
#[derive(Debug, Clone, Default)]
struct PacketPartialAtomProtection {
    carrier_node_ids: Vec<NodeId>,
    edge_ids: Vec<EdgeId>,
    carrier_atom_cover: Vec<(NodeId, BTreeSet<ProofAtomId>)>,
}

/// R3 — pre-cap partial-atom matching for carrier protection, over the
/// uncapped typed edges (plus exact-probe resolutions already merged into the
/// pre-cap answer) and provisional anchors. Emits every receipt identity a
/// matched atom used: source-aspect owners and symbols, anchored-window
/// owners, and — for TypedRelation facts — BOTH effective endpoint node ids
/// and the edge id itself, so a proof whose facts are all TypedRelation with
/// non-citation endpoints (review-005 finding 10: handler_processing's M2/M3)
/// still reaches the citation and graph caps with protection.
fn packet_partial_atom_protection(
    controller: &AppController,
    flow_requirements: &[FlowRequirement],
    answer: &AgentAnswerDto,
    coverage_extras: &PacketProofEvidenceExtras,
    session: &PacketProofSession,
) -> PacketPartialAtomProtection {
    let formulas = packet_flow_proof_formulas(flow_requirements);
    if formulas.is_empty() {
        return PacketPartialAtomProtection::default();
    }
    let planned = planned_atom_anchor_candidates(controller, answer, &formulas);
    packet_partial_atom_protection_with_planned(
        &formulas,
        answer,
        coverage_extras,
        &planned,
        &session.artifact_scans(),
    )
}

/// Controller-free core of R3, parameterized on the provisional anchors and
/// the trail-scan ledger.
fn packet_partial_atom_protection_with_planned(
    formulas: &[&'static FlowProofFormula],
    answer: &AgentAnswerDto,
    coverage_extras: &PacketProofEvidenceExtras,
    planned: &[PlannedAtomAnchor],
    ledger: &[(
        String,
        Vec<crate::agent::packet_candidate::PacketCandidateTrailScan>,
    )],
) -> PacketPartialAtomProtection {
    let mut evidence = packet_proof_receipts_view(&[], answer, coverage_extras);
    evidence
        .source_aspects
        .extend(planned.iter().map(|anchor| anchor.receipt.clone()));
    let mut protection = PacketPartialAtomProtection::default();
    let mut seen_nodes = HashSet::new();
    let mut seen_edges = HashSet::new();
    let add_node = |protection: &mut PacketPartialAtomProtection,
                    seen_nodes: &mut HashSet<NodeId>,
                    node_id: &NodeId,
                    atom: ProofAtomId| {
        if seen_nodes.insert(node_id.clone()) {
            protection.carrier_node_ids.push(node_id.clone());
        }
        if let Some((_, cover)) = protection
            .carrier_atom_cover
            .iter_mut()
            .find(|(existing, _)| existing == node_id)
        {
            cover.insert(atom);
        } else {
            protection
                .carrier_atom_cover
                .push((node_id.clone(), BTreeSet::from([atom])));
        }
    };
    for formula in formulas {
        for (_, outcome) in match_flow_requirements(formula, &evidence) {
            let FlowProofOutcome::Proved(proof) = outcome else {
                continue;
            };
            for atom in &proof.atoms {
                for fact in &atom.facts {
                    match fact {
                        DischargedFact::TypedRelation {
                            edge_id,
                            source,
                            target,
                        } => {
                            add_node(&mut protection, &mut seen_nodes, source, atom.atom);
                            add_node(&mut protection, &mut seen_nodes, target, atom.atom);
                            if seen_edges.insert(edge_id.clone()) {
                                protection.edge_ids.push(edge_id.clone());
                            }
                        }
                        DischargedFact::SourceAspect {
                            owner, symbol_id, ..
                        } => {
                            add_node(&mut protection, &mut seen_nodes, owner, atom.atom);
                            if let Some(symbol_id) = symbol_id {
                                add_node(&mut protection, &mut seen_nodes, symbol_id, atom.atom);
                            }
                        }
                        DischargedFact::AnchoredLineContainment { window_owner, .. } => {
                            add_node(&mut protection, &mut seen_nodes, window_owner, atom.atom);
                        }
                        DischargedFact::CoveredAbsence {
                            root,
                            edge_kind,
                            depth,
                        } => {
                            // F3 finding 3: an atom-depended covering scan's
                            // NARROWED coverage set must survive the caps, or
                            // the extras builder will refuse the scan at
                            // finalize and the absence fails. Protect exactly
                            // the recorded ids of the ledger scans this
                            // discharged absence could have used.
                            for (_, scans) in ledger {
                                for scan in scans {
                                    if scan.root == root.0
                                        && scan.depth == *depth
                                        && !scan.truncated
                                        && scan.direction
                                            == crate::agent::packet_candidate::PacketGraphDirection::Outgoing
                                        && scan.edge_kinds.contains(edge_kind)
                                    {
                                        for edge_id in &scan.coverage_edge_ids {
                                            if seen_edges.insert(edge_id.clone()) {
                                                protection.edge_ids.push(edge_id.clone());
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    protection
}

/// The selection step: deterministic weighted set cover over verified atoms
/// for the citation cap's protected set. One best source-bearing carrier per
/// material facet comes first. The snapshot's fully-proven carriers keep their
/// existing priority and order after those facets; the
/// partial-atom carriers are then ordered by greedy atom cover with the
/// stated tie-breaks — verified tier, lower source-byte cost, existing rank,
/// stable identity — and any remaining partial carriers fill the tail in
/// existing citation-rank order. The ordered list drives the citation cap's
/// front-promotion directly, so when the protected set exceeds capacity the
/// cover order decides survival.
fn select_protected_obligation_carriers(
    answer: &AgentAnswerDto,
    material_facet_node_ids: &[NodeId],
    snapshot_carrier_node_ids: &[NodeId],
    snapshot_edge_ids: &[EdgeId],
    partial: &PacketPartialAtomProtection,
) -> (Vec<NodeId>, Vec<EdgeId>) {
    let citation_rank: HashMap<&str, usize> = answer
        .citations
        .iter()
        .enumerate()
        .map(|(index, citation)| (citation.node_id.0.as_str(), index))
        .collect();
    let citation_by_node: HashMap<&str, &AgentCitationDto> = answer
        .citations
        .iter()
        .map(|citation| (citation.node_id.0.as_str(), citation))
        .collect();
    let tier_rank = |node_id: &NodeId| -> u8 {
        match citation_by_node
            .get(node_id.0.as_str())
            .and_then(|citation| citation.evidence_tier)
        {
            Some(PacketEvidenceTierDto::ExactSource) => 0,
            Some(PacketEvidenceTierDto::ResolvedGraph) => 1,
            Some(PacketEvidenceTierDto::StructuralText) => 2,
            Some(PacketEvidenceTierDto::LexicalSource) => 3,
            Some(PacketEvidenceTierDto::SymbolDoc) => 4,
            Some(PacketEvidenceTierDto::ComponentReport) => 5,
            Some(PacketEvidenceTierDto::DenseSemantic) => 6,
            Some(PacketEvidenceTierDto::SyntheticSourceScan) => 7,
            Some(PacketEvidenceTierDto::GeneratedSummary) => 8,
            None => 9,
        }
    };
    // Source-byte cost: the bytes this carrier's verification would still
    // spend — zero for an exact-source citation whose bytes are already
    // retained, one bounded-window budget for any other citation, and the
    // maximum for a carrier with no citation at all.
    let byte_cost = |node_id: &NodeId| -> usize {
        match citation_by_node.get(node_id.0.as_str()) {
            Some(citation)
                if citation.evidence_tier == Some(PacketEvidenceTierDto::ExactSource) =>
            {
                0
            }
            Some(_) => CARRIER_SOURCE_MAX_SNIPPET_BYTES,
            None => usize::MAX,
        }
    };
    let rank = |node_id: &NodeId| -> usize {
        citation_rank
            .get(node_id.0.as_str())
            .copied()
            .unwrap_or(usize::MAX)
    };

    let mut ordered = Vec::new();
    let mut seen = HashSet::new();
    for node_id in material_facet_node_ids {
        if seen.insert(node_id.clone()) {
            ordered.push(node_id.clone());
        }
    }
    for node_id in snapshot_carrier_node_ids {
        if seen.insert(node_id.clone()) {
            ordered.push(node_id.clone());
        }
    }
    // Greedy weighted set cover over the atoms the partial matching verified.
    let mut uncovered: BTreeSet<ProofAtomId> = partial
        .carrier_atom_cover
        .iter()
        .flat_map(|(_, atoms)| atoms.iter().copied())
        .collect();
    let mut remaining: Vec<&(NodeId, BTreeSet<ProofAtomId>)> =
        partial.carrier_atom_cover.iter().collect();
    while !uncovered.is_empty() && !remaining.is_empty() {
        let Some((position, _)) = remaining
            .iter()
            .enumerate()
            .map(|(position, (node_id, atoms))| {
                let gain = atoms.intersection(&uncovered).count();
                (position, (gain, node_id.clone()))
            })
            .filter(|(_, (gain, _))| *gain > 0)
            .max_by(|(_, (left_gain, left_id)), (_, (right_gain, right_id))| {
                left_gain
                    .cmp(right_gain)
                    // Ties invert: the BEST candidate has the LOWEST tier
                    // rank, byte cost, and rank, and the smallest id.
                    .then_with(|| tier_rank(right_id).cmp(&tier_rank(left_id)))
                    .then_with(|| byte_cost(right_id).cmp(&byte_cost(left_id)))
                    .then_with(|| rank(right_id).cmp(&rank(left_id)))
                    .then_with(|| right_id.0.cmp(&left_id.0))
            })
        else {
            break;
        };
        let (node_id, atoms) = remaining.remove(position);
        for atom in atoms {
            uncovered.remove(atom);
        }
        if seen.insert(node_id.clone()) {
            ordered.push(node_id.clone());
        }
    }
    // Fill remaining capacity in existing rank order (stable identity last).
    let mut tail: Vec<&NodeId> = remaining.iter().map(|(node_id, _)| node_id).collect();
    tail.sort_by(|left, right| {
        rank(left)
            .cmp(&rank(right))
            .then_with(|| left.0.cmp(&right.0))
    });
    for node_id in tail {
        if seen.insert(node_id.clone()) {
            ordered.push(node_id.clone());
        }
    }

    let mut edges = Vec::new();
    let mut seen_edges = HashSet::new();
    for edge_id in snapshot_edge_ids.iter().chain(partial.edge_ids.iter()) {
        if seen_edges.insert(edge_id.clone()) {
            edges.push(edge_id.clone());
        }
    }
    (ordered, edges)
}

fn append_packet_carrier_source_sections(
    controller: &AppController,
    question: &str,
    flow_requirements: &[FlowRequirement],
    answer: &mut AgentAnswerDto,
    limits: &PacketBudgetLimitsDto,
    proof_session: &PacketProofSession,
    protected_carrier_node_ids: &[NodeId],
) -> (Vec<SupportUnitDto>, Vec<VerifiedSourceAspectReceipt>) {
    if answer.citations.is_empty() || limits.max_snippets == 0 {
        return (Vec::new(), Vec::new());
    }

    let mut rendered = String::new();
    let mut steps = Vec::new();
    let mut source_support = Vec::new();
    let project_root = controller.require_project_root().ok();
    let file_focus_text = packet_file_source_focus_text(question, answer);
    let load_sources = |citation: &AgentCitationDto| -> Vec<CarrierSourceRange> {
        let bounded_source_read = citation_needs_bounded_source_read(citation);
        let behavioral_source = matches!(
            citation.kind,
            NodeKind::FUNCTION | NodeKind::METHOD | NodeKind::CLASS | NodeKind::STRUCT
        );
        if !bounded_source_read && !behavioral_source {
            return Vec::new();
        }
        let sources = if bounded_source_read {
            let (Some(path), Some(line)) = (citation.file_path.as_deref(), citation.line) else {
                return Vec::new();
            };
            let focused =
                if citation.kind == NodeKind::FILE && citation_is_lexical_source_range(citation) {
                    focused_file_source_ranges(controller, &file_focus_text, path)
                } else if behavioral_source {
                    controller
                        .snippet_function_body_context(citation.node_id.clone(), 0)
                        .ok()
                        .map(|snippet| {
                            focused_function_source_ranges(
                                controller,
                                &file_focus_text,
                                &snippet.path,
                                snippet.line,
                                snippet.node.end_line,
                            )
                        })
                        .unwrap_or_default()
                } else {
                    Vec::new()
                };
            if focused.is_empty() {
                controller
                    .bounded_file_snippet(
                        path,
                        line,
                        6,
                        CARRIER_SOURCE_MAX_SNIPPET_BYTES,
                        SOURCE_SNIPPET_TRUNCATION_SUFFIX,
                    )
                    .ok()
                    .map(|(path, snippet)| {
                        vec![CarrierSourceRange {
                            path,
                            start_line: line,
                            body: snippet.markdown,
                        }]
                    })
                    .unwrap_or_default()
            } else {
                focused
            }
        } else {
            controller
                .snippet_function_body_context(citation.node_id.clone(), 0)
                .ok()
                .map(|snippet| {
                    if matches!(citation.kind, NodeKind::FUNCTION | NodeKind::METHOD) {
                        let focused = long_behavioral_function_source_ranges(
                            controller,
                            &file_focus_text,
                            &snippet.path,
                            snippet.line,
                            snippet.node.end_line,
                        );
                        if !focused.is_empty() {
                            return focused;
                        }
                    }
                    if let Some(end_line) =
                        carrier_source_line_context_end_line(snippet.scope, snippet.line)
                        && let Ok((path, bounded)) = controller.bounded_file_snippet_range(
                            &snippet.path,
                            crate::BoundedSnippetRangeOptions {
                                focus_line: snippet.line,
                                start_line: snippet.line,
                                end_line,
                                context_lines: 0,
                                max_bytes: CARRIER_SOURCE_MAX_SNIPPET_BYTES,
                                truncation_suffix: SOURCE_SNIPPET_TRUNCATION_SUFFIX,
                            },
                        )
                    {
                        return vec![CarrierSourceRange {
                            path,
                            start_line: snippet.line,
                            body: bounded.markdown,
                        }];
                    }
                    if snippet.snippet_truncated
                        || snippet.snippet.len() > CARRIER_SOURCE_MAX_SNIPPET_BYTES
                    {
                        let focused = focused_function_source_ranges(
                            controller,
                            &file_focus_text,
                            &snippet.path,
                            snippet.line,
                            snippet.node.end_line,
                        );
                        if !focused.is_empty() {
                            return focused;
                        }
                    }
                    vec![CarrierSourceRange {
                        path: snippet.path,
                        start_line: snippet.line,
                        body: snippet.snippet,
                    }]
                })
                .unwrap_or_default()
        };
        attach_enclosing_type_source_context(controller, citation, sources)
    };

    // Spend one source slot on each exact material carrier first, then return to the original
    // citation/window order for optional context. Reads remain on demand: once the existing
    // snippet or byte cap binds, later citations are never opened.
    let append_source = |answer: &mut AgentAnswerDto,
                         rendered: &mut String,
                         source_support: &mut Vec<SupportUnitDto>,
                         steps: &mut Vec<AgentRetrievalStepDto>,
                         citation_index: usize,
                         duration_ms: u32,
                         source: CarrierSourceRange|
     -> bool {
        promote_verified_bounded_source_citation(&mut answer.citations[citation_index]);
        let citation = answer.citations[citation_index].clone();
        let body = source.body.trim_end();
        if body.is_empty() {
            return true;
        }
        let retained_body = truncate_carrier_source(body, CARRIER_SOURCE_MAX_SNIPPET_BYTES);
        let entry = format!("### {}\n\n{}\n\n", citation.display_name, retained_body);
        if rendered.len() + entry.len() > CARRIER_SOURCE_MAX_TOTAL_BYTES {
            return false;
        }
        rendered.push_str(&entry);
        let citation = &answer.citations[citation_index];
        if crate::agent::packet_evidence::citation_sufficiency_eligible(citation) {
            let path = packet_project_relative_display_path(project_root.as_deref(), &source.path);
            let (start_line, end_line) =
                source_receipt_line_range(retained_body, source.start_line);
            source_support.push(SupportUnitDto {
                id: format!("source:{}:{start_line}", citation.node_id.0),
                kind: SupportUnitKindDto::SourceRange,
                summary: format!(
                    "source for {} at {path}:{start_line}-{end_line}",
                    carrier_source_identity(&citation.display_name, citation.line, end_line,),
                ),
                path: Some(path),
                symbol_id: Some(citation.node_id.0.clone()),
                start_line: Some(start_line),
                end_line: Some(end_line),
                snippet: Some(retained_body.to_string()),
                edge_kind: None,
                from_symbol: None,
                to_symbol: None,
                query: None,
            });
        }
        steps.push(AgentRetrievalStepDto {
            kind: AgentRetrievalStepKindDto::SourceRead,
            status: AgentRetrievalStepStatusDto::Ok,
            duration_ms,
            input: Vec::new(),
            output: Vec::new(),
            message: Some(format!("carrier source for {}", citation.display_name)),
        });
        true
    };

    let protected = protected_carrier_node_ids.iter().collect::<HashSet<_>>();
    let mut attempted = HashSet::new();
    let mut deferred = HashMap::new();
    let mut source_read_citations = HashSet::new();
    let mut source_budget_exhausted = false;
    for citation_index in 0..answer.citations.len() {
        if steps.len() >= limits.max_snippets as usize {
            break;
        }
        let citation = answer.citations[citation_index].clone();
        if !protected.contains(&citation.node_id) {
            continue;
        }
        attempted.insert(citation_index);
        debug_assert!(steps.len() < limits.max_snippets as usize);
        debug_assert!(source_read_citations.insert(citation_index));
        let started = Instant::now();
        let mut sources = load_sources(&citation);
        let duration_ms = started.elapsed().as_millis().try_into().unwrap_or(u32::MAX);
        if sources.is_empty() {
            continue;
        }
        let first = sources.remove(0);
        if !append_source(
            answer,
            &mut rendered,
            &mut source_support,
            &mut steps,
            citation_index,
            duration_ms,
            first,
        ) {
            source_budget_exhausted = true;
            break;
        }
        deferred.insert(citation_index, (duration_ms, sources));
    }

    if !source_budget_exhausted {
        'citations: for citation_index in 0..answer.citations.len() {
            if steps.len() >= limits.max_snippets as usize {
                break;
            }
            let citation = answer.citations[citation_index].clone();
            let (duration_ms, sources) = if attempted.contains(&citation_index) {
                deferred
                    .remove(&citation_index)
                    .unwrap_or_else(|| (0, Vec::new()))
            } else {
                debug_assert!(steps.len() < limits.max_snippets as usize);
                debug_assert!(source_read_citations.insert(citation_index));
                let started = Instant::now();
                let sources = load_sources(&citation);
                (
                    started.elapsed().as_millis().try_into().unwrap_or(u32::MAX),
                    sources,
                )
            };
            for source in sources {
                if steps.len() >= limits.max_snippets as usize {
                    break 'citations;
                }
                if !append_source(
                    answer,
                    &mut rendered,
                    &mut source_support,
                    &mut steps,
                    citation_index,
                    duration_ms,
                    source,
                ) {
                    break 'citations;
                }
            }
        }
    }

    // R4: atom-anchored bounded provisional verification for the carriers
    // partially matched atoms name, sharing the same snippet-count and
    // total-byte budgets as the ordinary carrier reads above. Verified
    // windows are also recorded as structured SourceRange units in
    // `source_support` (F3 finding 9), so the anchored bytes ride the typed
    // packet support rather than only the byte-truncatable markdown section.
    let anchor_outcome = append_packet_atom_anchor_sources(
        controller,
        flow_requirements,
        answer,
        limits,
        proof_session,
        &mut source_support,
        &mut rendered,
        &mut steps,
    );
    if anchor_outcome.planned > 0 {
        // Step-trace visibility (F2 telemetry note): make cap-starved anchors
        // distinguishable from honestly unproven atoms when a formula
        // obligation later reports flow_proof_atoms_unproven.
        answer
            .retrieval_trace
            .annotations
            .push(RetrievalAnnotationDto::observation(format!(
                "packet_atom_anchor_sources planned={} produced={} budget_dropped={}",
                anchor_outcome.planned,
                anchor_outcome.receipts.len(),
                anchor_outcome.budget_dropped,
            )));
    }

    if rendered.is_empty() {
        return (source_support, anchor_outcome.receipts);
    }
    answer.retrieval_trace.steps.extend(steps);
    answer.sections.push(AgentResponseSectionDto {
        id: "packet-carrier-source".to_string(),
        title: "Carrier Source".to_string(),
        blocks: vec![AgentResponseBlockDto::Markdown { markdown: rendered }],
    });
    (source_support, anchor_outcome.receipts)
}

/// The outcome of the R4 anchoring phase: the real anchored receipts plus the
/// planning counters the step trace reports.
#[derive(Debug)]
struct PacketAtomAnchorOutcome {
    receipts: Vec<VerifiedSourceAspectReceipt>,
    planned: usize,
    budget_dropped: usize,
}

/// R4 phase — verifies the anchors a partial-atom proof would actually use.
///
/// Planning first (rule 4: provenance decides which receipts get produced):
/// the atom matcher runs over the post-cap evidence enriched with PROVISIONAL
/// anchored receipts derived from live-graph structure and storage
/// declaration lines; only the provisional receipts a proof discharged are
/// then verified for real — a bounded window read anchored to START at the
/// receipt-carried declaration line. A read that fails, returns different
/// lines than the anchor, or exceeds the shared snippet/byte budget produces
/// NO receipt, and the atom fails closed at the finalize matcher.
#[allow(clippy::too_many_arguments)]
fn append_packet_atom_anchor_sources(
    controller: &AppController,
    flow_requirements: &[FlowRequirement],
    answer: &AgentAnswerDto,
    limits: &PacketBudgetLimitsDto,
    proof_session: &PacketProofSession,
    source_support: &mut Vec<SupportUnitDto>,
    rendered: &mut String,
    steps: &mut Vec<AgentRetrievalStepDto>,
) -> PacketAtomAnchorOutcome {
    let mut outcome = PacketAtomAnchorOutcome {
        receipts: Vec::new(),
        planned: 0,
        budget_dropped: 0,
    };
    let formulas = packet_flow_proof_formulas(flow_requirements);
    if formulas.is_empty() {
        return outcome;
    }
    let coverage_extras = build_packet_proof_evidence_extras(answer, proof_session, Vec::new());
    let planned = planned_atom_anchor_candidates(controller, answer, &formulas);
    if planned.is_empty() {
        return outcome;
    }
    let mut evidence = packet_proof_receipts_view(source_support, answer, &coverage_extras);
    evidence
        .source_aspects
        .extend(planned.iter().map(|anchor| anchor.receipt.clone()));
    let mut selected: Vec<&PlannedAtomAnchor> = Vec::new();
    for formula in &formulas {
        for (_, requirement_outcome) in match_flow_requirements(formula, &evidence) {
            let FlowProofOutcome::Proved(proof) = requirement_outcome else {
                continue;
            };
            for atom in &proof.atoms {
                for fact in &atom.facts {
                    let matched = match fact {
                        DischargedFact::SourceAspect {
                            owner,
                            symbol_id,
                            start_line,
                            ..
                        } => planned.iter().find(|anchor| {
                            anchor.atom == atom.atom
                                && anchor.owner == *owner
                                && symbol_id.as_ref() == Some(&anchor.symbol)
                                && *start_line == Some(anchor.line)
                        }),
                        DischargedFact::AnchoredLineContainment {
                            line, window_owner, ..
                        } => planned.iter().find(|anchor| {
                            anchor.atom == atom.atom
                                && anchor.owner == *window_owner
                                && anchor.line == *line
                        }),
                        DischargedFact::TypedRelation { .. }
                        | DischargedFact::CoveredAbsence { .. } => None,
                    };
                    if let Some(anchor) = matched
                        && !selected.contains(&anchor)
                    {
                        selected.push(anchor);
                    }
                }
            }
        }
    }
    if selected.is_empty() {
        return outcome;
    }
    let node_labels: HashMap<String, String> = answer
        .graphs
        .iter()
        .filter_map(|artifact| match artifact {
            GraphArtifactDto::Uml { graph, .. } => Some(graph.nodes.iter()),
            GraphArtifactDto::Mermaid { .. } => None,
        })
        .flatten()
        .map(|node| (node.id.0.clone(), node.label.clone()))
        .collect();
    let Ok(storage) = controller.open_storage() else {
        outcome.planned = selected.len();
        outcome.budget_dropped = selected.len();
        return outcome;
    };
    let project_root = controller.require_project_root().ok();
    verify_planned_atom_anchors(
        &selected,
        project_root.as_deref(),
        &node_labels,
        limits,
        rendered,
        steps,
        source_support,
        &mut outcome,
        |anchor| {
            let path = anchor
                .symbol
                .0
                .parse::<i64>()
                .ok()
                .and_then(|core_id| {
                    storage
                        .get_node(codestory_contracts::graph::NodeId(core_id))
                        .ok()
                        .flatten()
                })
                .and_then(|node| {
                    AppController::file_path_for_node(&storage, &node)
                        .ok()
                        .flatten()
                })?;
            controller
                .bounded_file_snippet_range(
                    &path,
                    crate::BoundedSnippetRangeOptions {
                        focus_line: anchor.line,
                        start_line: anchor.line,
                        end_line: anchor.line.saturating_add(ATOM_ANCHOR_WINDOW_LINES),
                        context_lines: 0,
                        max_bytes: CARRIER_SOURCE_MAX_SNIPPET_BYTES,
                        truncation_suffix: SOURCE_SNIPPET_TRUNCATION_SUFFIX,
                    },
                )
                .ok()
                .map(|(resolved_path, snippet)| (resolved_path, snippet.markdown))
        },
    );
    outcome
}

/// The bounded verification loop of R4, parameterized on the window reader so
/// the shared-budget accounting is testable: each verified anchor spends one
/// snippet slot and its rendered bytes from the SAME budgets the ordinary
/// carrier reads use; a read that fails, returns an empty body, or does not
/// start at the anchored declaration line yields no receipt (fail closed);
/// budget-dropped anchors are counted for the step-trace annotation.
#[allow(clippy::too_many_arguments)]
fn verify_planned_atom_anchors(
    selected: &[&PlannedAtomAnchor],
    project_root: Option<&Path>,
    node_labels: &HashMap<String, String>,
    limits: &PacketBudgetLimitsDto,
    rendered: &mut String,
    steps: &mut Vec<AgentRetrievalStepDto>,
    source_support: &mut Vec<SupportUnitDto>,
    outcome: &mut PacketAtomAnchorOutcome,
    mut read_window: impl FnMut(&PlannedAtomAnchor) -> Option<(String, String)>,
) {
    outcome.planned = selected.len();
    for anchor in selected {
        if steps.len() >= limits.max_snippets as usize {
            outcome.budget_dropped += 1;
            continue;
        }
        let started = Instant::now();
        let Some((path, markdown)) = read_window(anchor) else {
            continue;
        };
        let body = markdown.trim_end();
        if body.is_empty() {
            continue;
        }
        let (start_line, end_line) = source_receipt_line_range(body, anchor.line);
        if start_line != anchor.line {
            // The read did not return the anchored declaration line — the
            // window cannot honestly claim the anchor. Fail closed.
            continue;
        }
        let label = node_labels
            .get(anchor.symbol.0.as_str())
            .map(String::as_str)
            .unwrap_or(anchor.symbol.0.as_str());
        let entry = format!("### {label} (atom-anchored)\n\n{body}\n\n");
        if rendered.len() + entry.len() > CARRIER_SOURCE_MAX_TOTAL_BYTES {
            outcome.budget_dropped += 1;
            continue;
        }
        rendered.push_str(&entry);
        outcome.receipts.push(VerifiedSourceAspectReceipt {
            kind: anchor.receipt.kind,
            owner: anchor.owner.clone(),
            symbol_id: Some(anchor.symbol.clone()),
            start_line: Some(start_line),
            end_line: Some(end_line),
            atom_anchor: Some(anchor.atom),
        });
        // F3 finding 9: the verified window also rides `packet.support` as a
        // structured SourceRange unit, so the anchored bytes are part of the
        // typed payload rather than only the byte-truncatable markdown
        // section. (Compile keeps SourceRange units whose symbol matches a
        // retained citation; for structural carriers without citations the
        // unit is display-honesty pre-compile — the residual is recorded in
        // the delivery report.)
        let display_path = packet_project_relative_display_path(project_root, &path);
        source_support.push(SupportUnitDto {
            id: format!(
                "atom-anchor:{:?}:{}:{start_line}",
                anchor.atom, anchor.symbol.0
            ),
            kind: SupportUnitKindDto::SourceRange,
            summary: format!(
                "atom-anchored source for {label} at {display_path}:{start_line}-{end_line}"
            ),
            path: Some(display_path),
            symbol_id: Some(anchor.symbol.0.clone()),
            start_line: Some(start_line),
            end_line: Some(end_line),
            snippet: Some(body.to_string()),
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
            message: Some(format!("atom-anchored source for {label}")),
        });
    }
}

/// A file-level lexical hit often identifies the right file through its path or title while its
/// reported range remains at the prologue. Spend the same fixed snippet budget on up to two
/// distinct windows that match the material queries the packet actually executed. This is
/// local representation of an already selected carrier, not another repository search.
fn focused_file_source_ranges(
    controller: &AppController,
    focus_text: &str,
    path: &str,
) -> Vec<CarrierSourceRange> {
    let Ok(candidate) = controller.resolve_project_file_path(path, false) else {
        return Vec::new();
    };
    if std::fs::metadata(candidate)
        .is_ok_and(|metadata| metadata.len() > CARRIER_SOURCE_MAX_FOCUS_FILE_BYTES)
    {
        return Vec::new();
    }
    let Ok(source) = controller.read_file_text(codestory_contracts::api::ReadFileTextRequest {
        path: path.to_string(),
    }) else {
        return Vec::new();
    };
    let lines = source.text.lines().collect::<Vec<_>>();
    focused_file_source_lines(&lines, focus_text)
        .into_iter()
        .filter_map(|focus_line| {
            let range_start = focus_line
                .saturating_sub(CARRIER_SOURCE_FILE_FOCUS_CONTEXT_LINES as u32)
                .max(1);
            controller
                .bounded_file_snippet(
                    path,
                    focus_line,
                    CARRIER_SOURCE_FILE_FOCUS_CONTEXT_LINES,
                    CARRIER_SOURCE_FOCUSED_SNIPPET_BYTES,
                    SOURCE_SNIPPET_TRUNCATION_SUFFIX,
                )
                .ok()
                .map(|(path, snippet)| CarrierSourceRange {
                    path,
                    start_line: range_start,
                    body: snippet.markdown,
                })
        })
        .collect()
}

fn packet_file_source_focus_text(question: &str, answer: &AgentAnswerDto) -> String {
    let mut focus = question.trim().to_string();
    let mut seen = HashSet::new();
    seen.insert(normalize_identifier(question));
    for diagnostic in &answer.retrieval_trace.packet_sidecar_diagnostics {
        let query = diagnostic.query.trim();
        let key = normalize_identifier(query);
        if query.is_empty() || key.is_empty() || !seen.insert(key) {
            continue;
        }
        focus.push('\n');
        focus.push_str(query);
    }
    focus
}

fn focused_file_source_lines(lines: &[&str], focus_text: &str) -> Vec<u32> {
    if lines.is_empty() {
        return Vec::new();
    }
    let clauses = source_focus_clauses(focus_text);
    if clauses.is_empty() {
        return Vec::new();
    }
    let mut candidates = (0..lines.len())
        .filter(|center| !source_focus_file_scaffolding_line(lines[*center]))
        .filter_map(|center| {
            let window_start = center.saturating_sub(CARRIER_SOURCE_FOCUS_CONTEXT_LINES);
            let window_end = (center + CARRIER_SOURCE_FOCUS_CONTEXT_LINES + 1).min(lines.len());
            if source_focus_file_window_is_scaffolding(&lines[window_start..window_end]) {
                return None;
            }
            let source_terms = source_focus_terms(&lines[window_start..window_end].join(" "));
            let center_terms = source_focus_terms(lines[center]);
            let mut best_clause_score = 0;
            let mut best_center_score = 0;
            let mut covered_clauses = 0;
            for clause_terms in &clauses {
                let score = clause_terms
                    .iter()
                    .filter(|term| {
                        source_terms
                            .iter()
                            .any(|source_term| source_focus_terms_match(term, source_term))
                    })
                    .count();
                let center_score = clause_terms
                    .iter()
                    .filter(|term| {
                        center_terms
                            .iter()
                            .any(|source_term| source_focus_terms_match(term, source_term))
                    })
                    .count();
                best_clause_score = best_clause_score.max(score);
                best_center_score = best_center_score.max(center_score);
                covered_clauses += usize::from(score >= 2);
            }
            (best_clause_score > 0).then_some((
                best_clause_score,
                covered_clauses,
                best_center_score,
                center,
            ))
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        right
            .0
            .cmp(&left.0)
            .then_with(|| right.1.cmp(&left.1))
            .then_with(|| right.2.cmp(&left.2))
            .then_with(|| left.3.cmp(&right.3))
    });

    let mut selected = Vec::new();
    for (_, _, _, center) in candidates {
        let focus_line = (center + 1) as u32;
        let overlaps_existing = selected.iter().any(|selected_line: &u32| {
            selected_line.abs_diff(focus_line) <= (CARRIER_SOURCE_FOCUS_CONTEXT_LINES * 2) as u32
        });
        if !overlaps_existing {
            selected.push(focus_line);
        }
        if selected.len() >= CARRIER_SOURCE_MAX_FOCUSED_WINDOWS {
            break;
        }
    }
    selected.sort_unstable();
    selected
}

fn source_focus_file_scaffolding_line(line: &str) -> bool {
    let line = line.trim_start().to_ascii_lowercase();
    line.is_empty()
        || line.starts_with("<!doctype")
        || line.starts_with("<head")
        || line.starts_with("</head")
        || line.starts_with("<meta")
        || line.starts_with("<title")
}

fn source_focus_file_window_is_scaffolding(lines: &[&str]) -> bool {
    let mut saw_content = false;
    for line in lines {
        let line = line.trim().to_ascii_lowercase();
        if line.is_empty() {
            continue;
        }
        saw_content = true;
        if !(source_focus_file_scaffolding_line(&line)
            || line.starts_with("<html")
            || line.starts_with("</html")
            || line.starts_with("<style")
            || line.starts_with("</style"))
        {
            return false;
        }
    }
    saw_content
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

const CARRIER_SOURCE_OWNER_CONTEXT_LOOKBACK_LINES: usize = 32;
const CARRIER_SOURCE_OWNER_SEPARATE_GAP_LINES: u32 = 12;

/// Keep the source-local type declaration with an owner-qualified method when it fits the same
/// bounded window. A method body proves mechanics, while the adjacent declaration and comments
/// often establish what kind of implementation owns those mechanics. This is a source expansion
/// for every qualified method shape; it does not consult the prompt, requirement ids, or fixture
/// vocabulary.
fn attach_enclosing_type_source_context(
    controller: &AppController,
    citation: &AgentCitationDto,
    mut sources: Vec<CarrierSourceRange>,
) -> Vec<CarrierSourceRange> {
    if citation.kind != NodeKind::METHOD || sources.is_empty() {
        return sources;
    }
    let owner_words = citation
        .display_name
        .rsplit(['.', ':', '#', '/', '\\'])
        .filter(|segment| !segment.is_empty())
        .nth(1)
        .map(source_identifier_words)
        .unwrap_or_default();
    if owner_words.len() < 2 {
        return sources;
    }
    let Some(anchor_line) = citation.line else {
        return sources;
    };
    let first = &sources[0];
    let (_, end_line) = source_receipt_line_range(&first.body, first.start_line);
    let Ok(source) = controller.read_file_text(codestory_contracts::api::ReadFileTextRequest {
        path: first.path.clone(),
    }) else {
        return sources;
    };
    let lines = source.text.lines().collect::<Vec<_>>();
    let owner_end_line = anchor_line.saturating_sub(1);
    let Some(owner_start_line) = enclosing_type_context_start_line(
        &lines,
        anchor_line,
        owner_end_line,
        &citation.display_name,
        CARRIER_SOURCE_MAX_SNIPPET_BYTES,
    ) else {
        return sources;
    };
    if anchor_line.saturating_sub(owner_start_line) > CARRIER_SOURCE_OWNER_SEPARATE_GAP_LINES {
        let Ok((path, bounded)) = controller.bounded_file_snippet_range(
            &first.path,
            crate::BoundedSnippetRangeOptions {
                focus_line: owner_end_line,
                start_line: owner_start_line,
                end_line: owner_end_line,
                context_lines: 0,
                max_bytes: CARRIER_SOURCE_MAX_SNIPPET_BYTES,
                truncation_suffix: SOURCE_SNIPPET_TRUNCATION_SUFFIX,
            },
        ) else {
            return sources;
        };
        let rendered_range = source_receipt_line_range(&bounded.markdown, owner_start_line);
        if rendered_range.0 > owner_start_line || rendered_range.1 < owner_end_line {
            return sources;
        }
        sources.insert(
            0,
            CarrierSourceRange {
                path,
                start_line: owner_start_line,
                body: bounded.markdown,
            },
        );
        return sources;
    }
    let Some(start_line) = enclosing_type_context_start_line(
        &lines,
        anchor_line,
        end_line,
        &citation.display_name,
        CARRIER_SOURCE_MAX_SNIPPET_BYTES,
    ) else {
        return sources;
    };
    let Ok((path, bounded)) = controller.bounded_file_snippet_range(
        &first.path,
        crate::BoundedSnippetRangeOptions {
            focus_line: anchor_line,
            start_line,
            end_line,
            context_lines: 0,
            max_bytes: CARRIER_SOURCE_MAX_SNIPPET_BYTES,
            truncation_suffix: SOURCE_SNIPPET_TRUNCATION_SUFFIX,
        },
    ) else {
        return sources;
    };
    let rendered_range = source_receipt_line_range(&bounded.markdown, start_line);
    if rendered_range.0 > start_line || rendered_range.1 < end_line {
        return sources;
    }
    sources[0] = CarrierSourceRange {
        path,
        start_line,
        body: bounded.markdown,
    };
    sources
}

fn enclosing_type_context_start_line(
    lines: &[&str],
    anchor_line: u32,
    end_line: u32,
    display_name: &str,
    maximum_bytes: usize,
) -> Option<u32> {
    let owner = display_name
        .rsplit(['.', ':', '#', '/', '\\'])
        .filter(|segment| !segment.is_empty())
        .nth(1)
        .map(source_identifier_key)
        .filter(|owner| !owner.is_empty())?;
    let anchor_index = usize::try_from(anchor_line.saturating_sub(1)).ok()?;
    let end_index = usize::try_from(end_line).ok()?.min(lines.len());
    if anchor_index >= lines.len() || end_index == 0 {
        return None;
    }
    let search_start = anchor_index.saturating_sub(CARRIER_SOURCE_OWNER_CONTEXT_LOOKBACK_LINES);
    let declaration_index = (search_start..anchor_index)
        .rev()
        .find(|index| source_line_declares_type_owner(lines[*index], &owner))?;
    if declaration_index >= end_index {
        return None;
    }

    let mut comment_start = declaration_index;
    while comment_start > search_start
        && source_owner_comment_line(lines[comment_start.saturating_sub(1)])
    {
        comment_start -= 1;
    }
    for candidate in [comment_start, declaration_index] {
        let line_count = end_index.saturating_sub(candidate);
        let bytes = lines[candidate..end_index]
            .iter()
            .map(|line| line.len().saturating_add(1))
            .sum::<usize>()
            // `bounded_file_snippet_range` renders line numbers and a focus marker beside the
            // source. Budget that framing here so owner context never truncates bytes already
            // retained by the method's declaration window.
            .saturating_add(line_count.saturating_mul(10))
            .saturating_add(16);
        if bytes <= maximum_bytes {
            return u32::try_from(candidate.saturating_add(1)).ok();
        }
    }
    None
}

fn source_line_declares_type_owner(line: &str, expected_owner: &str) -> bool {
    let trimmed = line.trim_start();
    if trimmed.starts_with("//")
        || trimmed.starts_with('#')
        || trimmed.starts_with('*')
        || trimmed.starts_with("/*")
    {
        return false;
    }
    let tokens = line
        .split(|character: char| !character.is_ascii_alphanumeric() && character != '_')
        .filter(|token| !token.is_empty())
        .map(|token| token.to_ascii_lowercase())
        .collect::<Vec<_>>();
    tokens.iter().any(|token| token == expected_owner)
        && tokens.iter().any(|token| {
            matches!(
                token.as_str(),
                "class" | "struct" | "interface" | "trait" | "record" | "object" | "impl"
            )
        })
}

fn source_owner_comment_line(line: &str) -> bool {
    let line = line.trim();
    !line.is_empty()
        && (line.starts_with("//")
            || line.starts_with("/*")
            || line.starts_with('*')
            || line.ends_with("*/"))
}

fn source_identifier_key(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || *character == '_')
        .flat_map(char::to_lowercase)
        .collect()
}

fn source_identifier_words(value: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let characters = value.chars().collect::<Vec<_>>();
    for (index, character) in characters.iter().copied().enumerate() {
        if !character.is_ascii_alphanumeric() {
            if !word.is_empty() {
                words.push(std::mem::take(&mut word));
            }
            continue;
        }
        let previous = index.checked_sub(1).and_then(|index| characters.get(index));
        let next = characters.get(index.saturating_add(1));
        let begins_word = !word.is_empty()
            && ((character.is_ascii_uppercase()
                && previous.is_some_and(|previous| previous.is_ascii_lowercase()))
                || (character.is_ascii_uppercase()
                    && previous.is_some_and(|previous| previous.is_ascii_uppercase())
                    && next.is_some_and(|next| next.is_ascii_lowercase())));
        if begins_word {
            words.push(std::mem::take(&mut word));
        }
        word.push(character.to_ascii_lowercase());
    }
    if !word.is_empty() {
        words.push(word);
    }
    words
}

fn carrier_source_identity(display_name: &str, anchor_line: Option<u32>, end_line: u32) -> String {
    if anchor_line.is_some_and(|anchor_line| end_line < anchor_line)
        && let Some(owner) = display_name
            .rsplit(['.', ':', '#', '/', '\\'])
            .filter(|segment| !segment.is_empty())
            .nth(1)
    {
        return format!(
            "implementation context of enclosing {owner} type that owns {display_name}"
        );
    }
    display_name.to_owned()
}

/// A long function is rarely best represented by its prologue. Select at most three bounded,
/// distinct windows whose source words match separate action clauses from the question.
/// The ranges remain exact source receipts for the already selected symbol; this changes only
/// which verified bytes spend the packet's fixed source budget.
fn focused_function_source_ranges(
    controller: &AppController,
    question: &str,
    path: &str,
    start_line: u32,
    end_line: Option<u32>,
) -> Vec<CarrierSourceRange> {
    let Some(end_line) = end_line.filter(|end_line| *end_line > start_line) else {
        return Vec::new();
    };
    let Ok(source) = controller.read_file_text(codestory_contracts::api::ReadFileTextRequest {
        path: path.to_string(),
    }) else {
        return Vec::new();
    };
    let lines = source.text.lines().collect::<Vec<_>>();
    let focus_lines = focused_function_source_lines(&lines, start_line, end_line, question);
    focus_lines
        .into_iter()
        .filter_map(|focus_line| {
            let range_start = focus_line
                .saturating_sub(CARRIER_SOURCE_FOCUS_CONTEXT_LINES as u32)
                .max(start_line);
            controller
                .bounded_file_snippet(
                    path,
                    focus_line,
                    CARRIER_SOURCE_FOCUS_CONTEXT_LINES,
                    CARRIER_SOURCE_FOCUSED_SNIPPET_BYTES,
                    SOURCE_SNIPPET_TRUNCATION_SUFFIX,
                )
                .ok()
                .map(|(path, snippet)| CarrierSourceRange {
                    path,
                    start_line: range_start,
                    body: snippet.markdown,
                })
        })
        .collect()
}

/// A parser may return declaration-only context even when the source contains an exact short
/// declaration or a long body. Keep the exact short range. For a long callable, retain its
/// declaration, one material internal callsite, and its positive terminal behavior instead of
/// treating the first 24 lines as the whole implementation.
fn long_behavioral_function_source_ranges(
    controller: &AppController,
    question: &str,
    path: &str,
    start_line: u32,
    end_line: Option<u32>,
) -> Vec<CarrierSourceRange> {
    let Ok(source) = controller.read_file_text(codestory_contracts::api::ReadFileTextRequest {
        path: path.to_string(),
    }) else {
        return Vec::new();
    };
    let lines = source.text.lines().collect::<Vec<_>>();
    let Some(end_line) = discover_callable_end_line(&lines, start_line)
        .or_else(|| end_line.filter(|end_line| *end_line >= start_line))
    else {
        return Vec::new();
    };
    if end_line
        <= start_line.saturating_add(CARRIER_SOURCE_DECLARATION_FALLBACK_LINES.saturating_sub(1))
    {
        return controller
            .bounded_file_snippet_range(
                path,
                crate::BoundedSnippetRangeOptions {
                    focus_line: start_line,
                    start_line,
                    end_line,
                    context_lines: 0,
                    max_bytes: CARRIER_SOURCE_MAX_SNIPPET_BYTES,
                    truncation_suffix: SOURCE_SNIPPET_TRUNCATION_SUFFIX,
                },
            )
            .ok()
            .map(|(path, snippet)| {
                vec![CarrierSourceRange {
                    path,
                    start_line,
                    body: snippet.markdown,
                }]
            })
            .unwrap_or_default();
    }
    long_behavioral_function_source_lines(&lines, start_line, end_line, question)
        .into_iter()
        .filter_map(|focus_line| {
            let range_start = focus_line
                .saturating_sub(CARRIER_SOURCE_FOCUS_CONTEXT_LINES as u32)
                .max(start_line);
            controller
                .bounded_file_snippet(
                    path,
                    focus_line,
                    CARRIER_SOURCE_FOCUS_CONTEXT_LINES,
                    CARRIER_SOURCE_FOCUSED_SNIPPET_BYTES,
                    SOURCE_SNIPPET_TRUNCATION_SUFFIX,
                )
                .ok()
                .map(|(path, snippet)| CarrierSourceRange {
                    path,
                    start_line: range_start,
                    body: snippet.markdown,
                })
        })
        .collect()
}

fn discover_callable_end_line(lines: &[&str], start_line: u32) -> Option<u32> {
    let mut depth = 0_u32;
    let mut parenthesis_depth = 0_u32;
    let mut bracket_depth = 0_u32;
    let mut saw_open = false;
    let mut in_block_comment = false;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for (index, line) in lines
        .iter()
        .enumerate()
        .skip(start_line.saturating_sub(1) as usize)
    {
        let mut chars = line.chars().peekable();
        while let Some(ch) = chars.next() {
            if in_block_comment {
                if ch == '*' && chars.peek() == Some(&'/') {
                    chars.next();
                    in_block_comment = false;
                }
                continue;
            }
            if let Some(delimiter) = quote {
                if escaped {
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == delimiter {
                    quote = None;
                }
                continue;
            }
            if ch == '/' && chars.peek() == Some(&'/') {
                break;
            }
            if ch == '/' && chars.peek() == Some(&'*') {
                chars.next();
                in_block_comment = true;
                continue;
            }
            if ch == '\'' || ch == '"' {
                quote = Some(ch);
                continue;
            }
            match ch {
                '(' if !saw_open => parenthesis_depth = parenthesis_depth.saturating_add(1),
                ')' if !saw_open => parenthesis_depth = parenthesis_depth.saturating_sub(1),
                '[' if !saw_open => bracket_depth = bracket_depth.saturating_add(1),
                ']' if !saw_open => bracket_depth = bracket_depth.saturating_sub(1),
                ';' if !saw_open && parenthesis_depth == 0 && bracket_depth == 0 => {
                    return Some((index + 1) as u32);
                }
                '{' if saw_open || (parenthesis_depth == 0 && bracket_depth == 0) => {
                    saw_open = true;
                    depth = depth.saturating_add(1);
                }
                '}' if saw_open => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return Some((index + 1) as u32);
                    }
                }
                _ => {}
            }
        }
    }
    None
}

fn long_behavioral_function_source_lines(
    lines: &[&str],
    start_line: u32,
    end_line: u32,
    question: &str,
) -> Vec<u32> {
    if lines.is_empty() || start_line == 0 || end_line <= start_line {
        return Vec::new();
    }
    let start_index = start_line.saturating_sub(1) as usize;
    let end_index = (end_line as usize).min(lines.len());
    if start_index >= end_index {
        return Vec::new();
    }
    let context = CARRIER_SOURCE_FOCUS_CONTEXT_LINES as u32;
    let declaration_focus = start_line.saturating_add(context).min(end_line);
    let terminal_focus = (start_index..end_index)
        .rev()
        .find(|index| lines[*index].trim_start().starts_with("return "))
        .or_else(|| {
            (start_index..end_index).rev().find(|index| {
                let line = lines[*index].trim_start();
                line.starts_with("throw ")
                    || line.starts_with("raise ")
                    || line.starts_with("yield ")
            })
        })
        .map(|index| (index + 1) as u32);
    let question_terms = source_focus_terms(question);
    let material_focus = (start_index..end_index)
        .filter_map(|index| {
            let line = lines[index];
            let line_number = (index + 1) as u32;
            if declaration_focus.abs_diff(line_number) <= context * 2
                || terminal_focus
                    .is_some_and(|terminal| terminal.abs_diff(line_number) <= context * 2)
            {
                return None;
            }
            let lower = line.to_ascii_lowercase();
            let line_terms = source_focus_terms(line);
            let question_overlap = question_terms
                .iter()
                .filter(|term| {
                    line_terms
                        .iter()
                        .any(|line_term| source_focus_terms_match(term, line_term))
                })
                .count();
            let awaited = usize::from(lower.contains("await "));
            let call_shape = usize::from(lower.contains('(') && lower.contains(')'));
            let material_action = usize::from(
                [
                    "commit", "connect", "dispatch", "execute", "finalize", "listen", "load",
                    "open", "persist", "pipe", "publish", "read", "request", "response", "save",
                    "send", "write",
                ]
                .iter()
                .any(|action| line_terms.iter().any(|term| term == action)),
            );
            let score = question_overlap * 4 + awaited * 6 + material_action * 3 + call_shape;
            (score > 0).then_some((score, index))
        })
        .max_by(|left, right| left.0.cmp(&right.0).then_with(|| right.1.cmp(&left.1)))
        .map(|(_, index)| (index + 1) as u32);

    let mut selected = vec![declaration_focus];
    if let Some(material_focus) = material_focus {
        selected.push(material_focus);
    }
    if let Some(terminal_focus) = terminal_focus
        && selected
            .iter()
            .all(|selected| selected.abs_diff(terminal_focus) > context * 2)
    {
        selected.push(terminal_focus);
    }
    selected.truncate(CARRIER_SOURCE_MAX_FOCUSED_WINDOWS);
    selected.sort_unstable();
    selected
}

fn focused_function_source_lines(
    lines: &[&str],
    start_line: u32,
    end_line: u32,
    question: &str,
) -> Vec<u32> {
    if lines.is_empty() || start_line == 0 {
        return Vec::new();
    }
    let start_index = start_line.saturating_sub(1) as usize;
    let end_index = (end_line as usize).min(lines.len());
    if start_index >= end_index {
        return Vec::new();
    }
    let clauses = source_focus_clauses(question);
    let mut selected = Vec::new();
    for clause_terms in clauses {
        let best = (start_index..end_index)
            .filter_map(|center| {
                let window_start = center.saturating_sub(CARRIER_SOURCE_FOCUS_CONTEXT_LINES);
                let window_end = (center + CARRIER_SOURCE_FOCUS_CONTEXT_LINES + 1).min(end_index);
                let source_terms = source_focus_terms(&lines[window_start..window_end].join(" "));
                let center_terms = source_focus_terms(lines[center]);
                let score = clause_terms
                    .iter()
                    .filter(|term| {
                        source_terms
                            .iter()
                            .any(|source_term| source_focus_terms_match(term, source_term))
                    })
                    .count();
                let center_score = clause_terms
                    .iter()
                    .filter(|term| {
                        center_terms
                            .iter()
                            .any(|source_term| source_focus_terms_match(term, source_term))
                    })
                    .count();
                (score >= 2).then_some((score, center_score, center))
            })
            .max_by(|left, right| {
                left.0
                    .cmp(&right.0)
                    .then_with(|| left.1.cmp(&right.1))
                    .then_with(|| right.2.cmp(&left.2))
            });
        let Some((_, _, center)) = best else {
            continue;
        };
        let focus_line = (center + 1) as u32;
        if !selected.contains(&focus_line) {
            selected.push(focus_line);
        }
        if selected.len() >= CARRIER_SOURCE_MAX_FOCUSED_WINDOWS {
            break;
        }
    }
    selected.sort_unstable();
    selected
}

fn source_focus_clauses(question: &str) -> Vec<Vec<String>> {
    let mut clauses: Vec<Vec<String>> = Vec::new();
    let mut active_list: Option<Vec<String>> = None;
    for clause in question.split([',', ';', '.', '?', '!', '\n', '\r']) {
        let clause = clause.trim();
        let continuation = clause.strip_prefix("and ").unwrap_or(clause);
        let combined_terms = source_focus_terms(continuation);
        if (combined_terms.len() <= 1
            || (continuation.len() != clause.len() && combined_terms.len() <= 2))
            && let Some(previous) = clauses.last_mut()
        {
            let list = active_list
                .get_or_insert_with(|| previous.last().cloned().into_iter().collect::<Vec<_>>());
            for term in combined_terms {
                if !previous.contains(&term) {
                    previous.push(term.clone());
                }
                if !list.contains(&term) {
                    list.push(term);
                }
            }
            continue;
        }
        if let Some(list) = active_list.take()
            && list.len() >= 2
            && !clauses.contains(&list)
        {
            clauses.push(list);
        }
        for action in continuation.split(" and ") {
            let terms = source_focus_terms(action);
            if terms.len() >= 2 && !clauses.contains(&terms) {
                clauses.push(terms);
            }
        }
    }
    if let Some(list) = active_list
        && list.len() >= 2
        && !clauses.contains(&list)
    {
        clauses.push(list);
    }
    clauses
}

fn source_focus_terms(value: &str) -> Vec<String> {
    symbol_query_tokens(value)
        .into_iter()
        .filter(|term| term.len() >= 3 && !source_focus_scaffolding(term))
        .map(|term| source_focus_stem(&term).to_string())
        .collect()
}

fn source_focus_scaffolding(term: &str) -> bool {
    matches!(
        term,
        "cite"
            | "cites"
            | "across"
            | "describe"
            | "explain"
            | "file"
            | "files"
            | "from"
            | "how"
            | "into"
            | "its"
            | "name"
            | "names"
            | "source"
            | "sources"
            | "supporting"
            | "symbol"
            | "symbols"
            | "that"
            | "the"
            | "their"
            | "then"
            | "this"
            | "through"
            | "trace"
            | "with"
    )
}

fn source_focus_stem(term: &str) -> &str {
    for suffix in ["ingly", "edly", "ation", "ment", "ing", "ed", "s"] {
        if let Some(stem) = term.strip_suffix(suffix)
            && stem.len() >= 4
        {
            return stem;
        }
    }
    term
}

fn source_focus_terms_match(left: &str, right: &str) -> bool {
    if left == right {
        return true;
    }
    fn role_noun_base(term: &str) -> Option<&str> {
        term.strip_suffix("er").filter(|base| base.len() >= 4)
    }
    if role_noun_base(left) == Some(right) || role_noun_base(right) == Some(left) {
        return false;
    }
    left.len().min(right.len()) >= 4 && (left.starts_with(right) || right.starts_with(left))
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
    // `display_name` is a host path for FILE-kind citations, so it needs the same
    // normalisation as `file_path` -- otherwise the row prints an absolute checkout path
    // next to the relative path it duplicates, and the reader cannot use either as an anchor.
    let name = packet_display_path(&citation.display_name);
    format!(
        "- `{}` ({:?}) - `{}`{} - {} - score {:.3}",
        name, citation.kind, path, line, role, citation.score
    )
}

#[cfg(test)]
mod legacy_source_scans {
    use super::*;

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
        (!token.is_empty()).then(|| token.to_string())
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

    pub(super) fn maybe_append_sql_schema_file_citations(
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
                    target: None,
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
                    source_excerpt: None,
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
                    target: None,
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
                    source_excerpt: None,
                });
                appended_anchors += 1;
            }
        }

        if appended_files > 0 || appended_anchors > 0 {
            answer
            .retrieval_trace
            .annotations
            .push(RetrievalAnnotationDto::observation(format!(
                "packet_generic_sql_schema_file_citations files={appended_files} anchors={appended_anchors}"
            )));
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

    pub(super) fn maybe_append_generic_source_shape_citations(
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
                && packet_terms_have_any(
                    &terms,
                    &["variable", "variables", "keyframe", "keyframes"],
                ));
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
            collect_cited_request_validation_shape_candidates(
                project_root,
                answer,
                &mut candidates,
            );
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
                target: None,
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
                source_excerpt: None,
            });
            appended = appended.saturating_add(1);
        }

        if appended > 0 || skipped_existing > 0 {
            // Counters, not evidence: `skipped_existing` here means "already cited", and the legacy
            // prose heuristic read the word `skipped` as an evidence gap on every packet that
            // appended generic source-shape citations.
            answer
            .retrieval_trace
            .annotations
            .push(RetrievalAnnotationDto::observation(format!(
                "packet_generic_source_shape_citations appended={appended} skipped_existing={skipped_existing}"
            )));
        }
    }

    #[allow(clippy::too_many_arguments)]
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
                line: packet_first_line_containing(source, &["Future<Response>", " get("])
                    .unwrap_or(1),
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
                line: packet_first_line_containing(source, &["Future<Response>", " get("])
                    .unwrap_or(1),
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
                line: packet_first_line_containing(source, &["ByteStream", " finalize("])
                    .unwrap_or(1),
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
            && let Some((display_name, line)) =
                packet_first_exported_name_near(source, &["handler"])
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
            && let Some((display_name, line)) =
                packet_first_exported_name_near(source, &["middleware"])
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
            push_command_shape_candidate(
                path,
                candidates,
                name,
                line,
                "command_network_input",
                130.0,
            );
        }
        if has_command {
            if let Some((name, line)) =
                packet_best_c_function_with_words(&functions, &["process", "command"], None)
            {
                push_command_shape_candidate(
                    path,
                    candidates,
                    name,
                    line,
                    "command_dispatch",
                    126.0,
                );
            }
            if let Some((name, line)) = packet_c_function_exact(&functions, "call") {
                push_command_shape_candidate(
                    path,
                    candidates,
                    name,
                    line,
                    "command_dispatch",
                    125.0,
                );
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

    fn packet_c_function_exact(
        functions: &[(String, u32)],
        expected: &str,
    ) -> Option<(String, u32)> {
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
        project_root: &Path,
        answer: &AgentAnswerDto,
        candidates: &mut Vec<PacketGenericSourceShapeCandidate>,
    ) {
        for citation in &answer.citations {
            let Some(path) = citation.file_path.as_deref().map(Path::new) else {
                continue;
            };
            let source_path = if path.is_absolute() {
                path.to_path_buf()
            } else {
                project_root.join(path)
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
            let Ok(source) = std::fs::read_to_string(&source_path) else {
                continue;
            };
            if !packet_swift_request_validation_source(&source) {
                continue;
            }
            candidates.push(PacketGenericSourceShapeCandidate {
                path: source_path,
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
                        && (normalized_method.contains("map")
                            || source_lower.contains("mapexpression"))
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
    pub(super) fn maybe_append_required_file_scoped_source_citations(
        project_root: &Path,
        question: &str,
        task_class: PacketTaskClassDto,
        extra_probes: &[String],
        explicit_probes: &[String],
        answer: &mut AgentAnswerDto,
    ) {
        let required_queries = packet_sufficiency_required_probe_queries_with_extra(
            question,
            task_class,
            extra_probes,
        );
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
            let Some(path) =
                packet_required_probe_source_path(project_root, &parts, &answer.citations)
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
            let explicit = explicit_probes
                .iter()
                .any(|probe| probe.eq_ignore_ascii_case(&query));
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
                target: None,
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
                coverage_role: Some(if explicit {
                    "explicit source probe".to_string()
                } else {
                    "required source probe".to_string()
                }),
                eligible_for_sufficiency: Some(!explicit),
                source_excerpt: None,
            });
            appended += 1;
        }

        if appended > 0 || file_scoped > 0 {
            answer
            .retrieval_trace
            .annotations
            .push(RetrievalAnnotationDto::observation(format!(
                "packet_required_file_scoped_source_citations file_scoped={file_scoped} appended={appended} already_cited={already_cited} no_path={no_path} too_large={too_large} read_failed={read_failed} no_anchor={no_anchor}"
            )));
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
        if normalized_terminal.is_empty()
            || !normalize_identifier(line).contains(&normalized_terminal)
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
            return lower.contains("<input")
                && packet_html_line_has_attribute_value(&lower, "id", id);
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

    fn packet_html_line_has_attribute_value(
        line_lower: &str,
        attribute: &str,
        value: &str,
    ) -> bool {
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

    fn packet_source_probe_anchor_kind(
        line: &str,
        parts: &PacketFileScopedSymbolProbe,
    ) -> NodeKind {
        let lower = line.to_ascii_lowercase();
        if parts.raw_symbols.join(" ").starts_with("input#")
            || (parts.raw_symbols.len() == 1 && lower.contains('<'))
            || (parts.symbols.len() >= 2
                && parts.symbols[0] == "foreign"
                && parts.symbols[1] == "key")
            || (parts.symbols.len() >= 3
                && parts.symbols[0] == "create"
                && parts.symbols[1] == "table")
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
}

#[cfg(test)]
use legacy_source_scans::*;

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
    is_drill_continuation: bool,
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
    let literal_diagnostic_signal = has_literal_diagnostic_signal(prompt);
    let promotable_focus_available =
        req.focus_node_id.is_some() || investigation_focus_anchor(prompt, &hits).is_some();
    let mut expansion_added_hits = false;
    let block_nucleo_supplement =
        sidecar_retrieval_blocks_nucleo_supplement(controller, hits.len());
    if block_nucleo_supplement && weak_initial_hits(prompt, &hits) {
        trace.annotate_gap(
            "retrieval_primary skipped local nucleo investigation supplement on weak hits",
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
        } else if weak_initial_hits(prompt, &hits) && !is_repo_explanation_prompt(prompt) {
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
        } else if weak_initial_hits(prompt, &hits) {
            trace.observe(
                "Investigation deferred a broad repo explanation prompt to sidecar evidence only.",
            );
        }

        if weak_initial_hits(prompt, &hits) && !is_repo_explanation_prompt(prompt) {
            trace.annotate_gap("Investigation low confidence gap after sidecar query expansion.");
        }
    } else if should_investigate(resolved_profile)
        && weak_initial_hits(prompt, &hits)
        && promotable_focus_available
    {
        trace.observe(
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
            trace.observe(
                "Investigation used grounding snapshot diagnostic supplement for a broad repo explanation prompt.",
            );
        }
    } else if should_investigate(resolved_profile)
        && weak_initial_hits(prompt, &hits)
        && is_repo_explanation_prompt(prompt)
        && block_nucleo_supplement
    {
        trace.annotate_gap(
            "Grounding snapshot supplement skipped because sidecar-primary retrieval is mandatory.",
        );
    }

    let focus_node_id = investigation_focus_node(req, prompt, &hits);

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
        coverage_role: scored.hit.coverage_role.clone(),
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
    prompt: &str,
    hits: &[SearchHit],
) -> Option<NodeId> {
    if !matches!(
        &req.retrieval_profile,
        AgentRetrievalProfileSelectionDto::Custom { .. }
    ) || !packet_terms_indicate_search_execution_flow(&packet_probe_terms(prompt))
    {
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
        target: None,
        match_quality: None,
        resolvable: true,
        evidence_tier: Some(codestory_contracts::api::PacketEvidenceTierDto::ResolvedGraph),
        evidence_producer: Some("test_grounding_symbol".to_string()),
        resolution_status: Some(codestory_contracts::api::PacketEvidenceResolutionDto::Resolved),
        loss_reason: None,
        coverage_role: None,
        eligible_for_sufficiency: Some(true),
        source_excerpt: None,
        verification_targets: Vec::new(),
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
    if bundle.repo_explanation_supplement_used {
        provenance.push_str(
            "- Grounding snapshot diagnostic supplement for broad repo overview evidence.\n",
        );
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::eval_probes::{
        EVAL_PROBES_ENV, pop_eval_probes_test_override, push_eval_probes_test_override,
    };
    use crate::agent::planning::packet_plan_query_is_exact_symbol_identity;
    use crate::agent::profiles::ResolvedProfile;

    #[test]
    fn packet_sections_lead_with_relations_and_carrier_source() {
        let section = |id: &str| AgentResponseSectionDto {
            id: id.to_string(),
            title: id.to_string(),
            blocks: Vec::new(),
        };
        let mut sections = vec![
            section("retrieval-evidence"),
            section("packet-evidence-ledger"),
            section("analysis"),
            section("packet-carrier-source"),
            section(PACKET_RESOLVED_RELATIONS_SECTION_ID),
            section("packet-flow-claims"),
        ];

        order_packet_sections(&mut sections);

        assert_eq!(
            sections
                .iter()
                .map(|section| section.id.as_str())
                .collect::<Vec<_>>(),
            vec![
                PACKET_RESOLVED_RELATIONS_SECTION_ID,
                "packet-carrier-source",
                "retrieval-evidence",
                "analysis",
                "packet-flow-claims",
                "packet-evidence-ledger",
            ]
        );
    }

    #[test]
    fn long_function_focus_covers_separate_question_actions() {
        let source = [
            "int run(void) {",
            "    if (testing) return run_tests();",
            "    parse_options();",
            "    load_config();",
            "    initializeRuntime();",
            "    start_listeners();",
            "    report_ready();",
            "    flush_logs();",
            "    update_metrics();",
            "    collect_stats();",
            "    flush_stats();",
            "    check_health();",
            "    report_health();",
            "    flush_reports();",
            "    update_clock();",
            "    /* Enter the event loop. */",
            "    eventLoopRun(runtime.loop);",
            "    return 0;",
            "}",
        ];

        let focus = focused_function_source_lines(
            &source,
            1,
            source.len() as u32,
            "Trace how the service initializes the runtime, then enters the event loop.",
        );

        assert_eq!(focus, [5, 16]);
    }

    #[test]
    fn long_function_focus_requires_two_query_terms() {
        let source = ["fn run() {", "    service();", "    unrelated_work();", "}"];

        assert!(
            focused_function_source_lines(
                &source,
                1,
                source.len() as u32,
                "Explain how the service enters its event loop.",
            )
            .is_empty()
        );
    }

    #[test]
    fn declaration_only_long_callable_keeps_declaration_callsite_and_terminal() {
        let source = [
            "Future<Result> execute(Request request) async {",
            "  validate(request);",
            "  configure(request);",
            "  prepare(request);",
            "  final stream = request.finalize();",
            "  final connection = await transport.open(request.url);",
            "  configureHeaders(connection);",
            "  installCancellation(connection);",
            "  recordAttempt();",
            "  recordQueueDepth();",
            "  prepareTrace();",
            "  attachContext();",
            "  updateDeadline();",
            "  installHooks();",
            "  await beforeSend();",
            "  final response = await stream.pipe(connection);",
            "  observe(response);",
            "  transform(response);",
            "  attachListeners(response);",
            "  recordHeaders(response);",
            "  finalizeMetrics(response);",
            "  notifyObservers(response);",
            "  updatePool(response);",
            "  collectTrailers(response);",
            "  recordCompletion(response);",
            "  closeSpan(response);",
            "  settleAccounting(response);",
            "  return Result.from(response);",
            "}",
        ];

        let focus = long_behavioral_function_source_lines(
            &source,
            1,
            source.len() as u32,
            "Explain execute send behavior.",
        );

        assert_eq!(focus, [6, 17, 28]);
    }

    #[test]
    fn declaration_only_callable_end_ignores_comment_and_string_braces() {
        let source = [
            "Future<Result> execute() async {",
            "  // A comment with } does not end the callable.",
            "  final marker = '{';",
            "  if (ready) {",
            "    await run();",
            "  }",
            "  return Result();",
            "}",
            "void next() {}",
        ];

        assert_eq!(discover_callable_end_line(&source, 1), Some(8));
    }

    #[test]
    fn declaration_only_callable_end_stops_at_the_first_real_semicolon() {
        let source = [
            "Future<Response> get(Uri url, {Map<String, String>? headers}) =>",
            "    _withClient((client) => client.get(url, headers: headers));",
            "/// unrelated docs with a ; inside the comment",
            "Future<Response> post(Uri url);",
        ];

        assert_eq!(discover_callable_end_line(&source, 1), Some(2));
    }

    #[test]
    fn callable_end_ignores_named_and_optional_parameter_delimiters() {
        let source = [
            "Future<Response> send(String method, {Map<String, String>? headers},",
            "    [Object? body, Encoding? encoding]) async {",
            "  if (body != null) {",
            "    prepare(body);",
            "  }",
            "  return await transport.send(body);",
            "}",
            "void next() {}",
        ];

        assert_eq!(discover_callable_end_line(&source, 1), Some(7));
    }

    #[test]
    fn source_focus_does_not_treat_control_flow_as_a_component_owner() {
        assert!(!source_focus_terms_match("matcher", "match"));
        assert!(!source_focus_terms_match("searcher", "search"));
        assert!(!source_focus_terms_match("printer", "print"));
        assert!(source_focus_terms_match("matcher", "matcher"));
    }

    #[test]
    fn long_function_focus_keeps_component_list_and_parallel_dispatch_together() {
        let source = [
            "fn search_parallel(args: &HiArgs) {",
            "    let started_at = Instant::now();",
            "    let buffer = args.buffer_writer();",
            "    let stats = args.stats();",
            "    let matched = AtomicBool::new(false);",
            "    let searched = AtomicBool::new(false);",
            "    let haystack_builder = args.haystack_builder();",
            "    let mut worker = args.search_worker(",
            "        args.matcher()?,",
            "        args.searcher()?,",
            "        args.printer(),",
            "    )?;",
            "    args.walk_builder()?.build_parallel().run(|| {",
            "        let haystack = haystack_builder.build_from_result(result);",
            "        worker.search(&haystack);",
            "    });",
            "}",
        ];

        let focus = focused_function_source_lines(
            &source,
            1,
            source.len() as u32,
            "Explain how the program walks candidate files and executes a search over each haystack through matcher, searcher, and printer components.\nparallel search walk builder",
        );

        let covers = |line: u32| {
            focus.iter().any(|focus_line| {
                focus_line.saturating_sub(CARRIER_SOURCE_FOCUS_CONTEXT_LINES as u32) <= line
                    && focus_line + CARRIER_SOURCE_FOCUS_CONTEXT_LINES as u32 >= line
            })
        };
        assert!(
            covers(8) && covers(13),
            "the focused ranges must keep both component construction and parallel dispatch: {focus:?}",
        );
    }

    #[test]
    fn file_focus_skips_title_and_covers_structure_and_behavior() {
        let source = [
            "<!DOCTYPE html>",
            "<html>",
            "<head>",
            "<title>Detailed custom validation</title>",
            "</head>",
            "<body>",
            "<form novalidate>",
            "<input type=\"email\" required minlength=\"8\">",
            "</form>",
            "const form = document.querySelector('form');",
            "form.addEventListener('submit', function (event) {",
            "if (!email.validity.valid) {",
            "showError();",
            "event.preventDefault();",
            "}",
            "});",
        ];

        let focus = focused_file_source_lines(
            &source,
            "Explain how native form constraints combine with custom validation.\nsubmit prevent default\nvalidity state",
        );

        let covers = |line: u32| {
            focus.iter().any(|focus_line| {
                focus_line.saturating_sub(CARRIER_SOURCE_FOCUS_CONTEXT_LINES as u32) <= line
                    && focus_line + CARRIER_SOURCE_FOCUS_CONTEXT_LINES as u32 >= line
            })
        };
        assert!(
            covers(8) && covers(12),
            "the focused ranges must keep both native constraints and custom validation: {focus:?}",
        );
        assert!(!focus.contains(&4), "the title is not behavioral evidence");
    }

    #[test]
    fn file_focus_returns_no_window_without_a_material_term_match() {
        let source = ["<title>Unrelated</title>", "plain content", "more content"];

        assert!(focused_file_source_lines(&source, "network dispatch").is_empty());
    }

    #[test]
    fn file_focus_does_not_promote_a_matching_document_header() {
        let source = [
            "<!DOCTYPE html>",
            "<html>",
            "<head>",
            "<meta charset=\"utf-8\">",
            "<title>Detailed custom validation</title>",
            "</head>",
            "</html>",
        ];

        assert!(focused_file_source_lines(&source, "custom validation").is_empty());
    }

    #[test]
    fn source_receipt_range_uses_rendered_source_lines_not_markdown_lines() {
        let markdown = "```text\n  41 | before();\n> 42 | focus();\n  43 | after();\n```";

        assert_eq!(source_receipt_line_range(markdown, 42), (41, 43));
        assert_eq!(source_receipt_line_range("plain text", 42), (42, 42));
    }

    #[test]
    fn qualified_method_source_keeps_a_fitting_owner_declaration_and_comments() {
        let source = [
            "import helper;",
            "",
            "/// Default sessions implement convenience operations through dispatch.",
            "class DefaultSession implements Session {",
            "  Result perform(Request request) {",
            "    return dispatch(request);",
            "  }",
        ];

        assert_eq!(
            enclosing_type_context_start_line(
                &source,
                5,
                7,
                "DefaultSession.perform",
                CARRIER_SOURCE_MAX_SNIPPET_BYTES,
            ),
            Some(3)
        );
        assert_eq!(
            source_identifier_words("DefaultSession"),
            ["default", "session"]
        );
        assert_eq!(source_identifier_words("IOClient"), ["io", "client"]);
        assert_eq!(source_identifier_words("Client"), ["client"]);
        assert_eq!(
            carrier_source_identity("DefaultSession.perform", Some(5), 4),
            "implementation context of enclosing DefaultSession type that owns DefaultSession.perform"
        );
        assert_eq!(
            carrier_source_identity("DefaultSession.perform", Some(5), 7),
            "DefaultSession.perform"
        );
    }

    #[test]
    fn qualified_method_source_falls_back_to_the_owner_declaration_when_docs_do_not_fit() {
        let long_docs = format!("/// {}", "context ".repeat(300));
        let source = [
            long_docs.as_str(),
            "class NativeSession {",
            "  Result send(Request request) {",
            "    return transport.send(request);",
            "  }",
        ];

        assert_eq!(
            enclosing_type_context_start_line(
                &source,
                3,
                5,
                "NativeSession.send",
                CARRIER_SOURCE_MAX_SNIPPET_BYTES,
            ),
            Some(2)
        );
    }

    #[test]
    fn qualified_method_source_never_borrows_an_unrelated_owner_declaration() {
        let source = [
            "class CacheClient {",
            "  Result send(Request request) {",
            "    return write(request);",
            "  }",
        ];

        assert_eq!(
            enclosing_type_context_start_line(
                &source,
                2,
                4,
                "NetworkClient.send",
                CARRIER_SOURCE_MAX_SNIPPET_BYTES,
            ),
            None
        );
    }

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
            target: None,
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
            source_excerpt: None,
            verification_targets: Vec::new(),
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

    #[test]
    fn packet_only_primary_hits_follow_exact_final_search_identity() {
        let mut retained = test_search_hit("session-request", 0.9);
        retained.file_path = Some("src/requests/sessions.py".into());
        retained.line = Some(557);
        let mut stale_location = retained.clone();
        stale_location.line = Some(558);
        let mut discarded = test_search_hit("telemetry-request", 0.8);
        discarded.file_path = Some("src/telemetry.py".into());
        discarded.line = Some(10);

        let packet_hits = vec![
            PacketSearchHit::without_graph(retained.clone()),
            PacketSearchHit::without_graph(stale_location),
            PacketSearchHit::without_graph(discarded),
        ];
        let kept = retain_packet_hits_for_final_hits(packet_hits, &[retained.clone()]);

        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].hit.node_id, retained.node_id);
        assert_eq!(kept[0].hit.file_path, retained.file_path);
        assert_eq!(kept[0].hit.line, retained.line);
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
            target: None,
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
            source_excerpt: None,
        }
    }

    #[test]
    fn lexical_source_range_citation_gets_a_bounded_source_receipt() {
        let mut citation = test_packet_citation("include/fmt/args.h", "include/fmt/args.h", 0.9);
        citation.kind = NodeKind::FILE;
        citation.evidence_tier = Some(PacketEvidenceTierDto::LexicalSource);
        citation.resolution_status = Some(PacketEvidenceResolutionDto::SourceRangeOnly);

        assert!(citation_needs_bounded_source_read(&citation));

        citation.resolution_status = Some(PacketEvidenceResolutionDto::Unresolved);
        assert!(!citation_needs_bounded_source_read(&citation));
    }

    #[test]
    fn internal_synthetic_source_scan_uses_its_path_and_line_for_the_receipt() {
        let mut citation = test_packet_citation("format_arg_store", "include/fmt/base.h", 46.0);
        citation.kind = NodeKind::STRUCT;
        citation.evidence_tier = Some(PacketEvidenceTierDto::SyntheticSourceScan);
        citation.resolution_status = Some(PacketEvidenceResolutionDto::SourceRangeOnly);
        citation.evidence_producer = Some("packet_cited_formatting_type".to_owned());

        assert!(citation_needs_bounded_source_read(&citation));
        assert!(citation_is_internal_synthetic_source_range(&citation));
        assert!(promote_verified_bounded_source_citation(&mut citation));
        assert_eq!(
            citation.evidence_tier,
            Some(PacketEvidenceTierDto::ExactSource)
        );
        assert_eq!(
            citation.evidence_producer.as_deref(),
            Some("verified_packet_cited_formatting_type_source_read")
        );
        assert!(crate::agent::packet_evidence::citation_sufficiency_eligible(&citation));

        citation.evidence_producer = Some("external_source_scan".to_owned());
        citation.evidence_tier = Some(PacketEvidenceTierDto::SyntheticSourceScan);
        assert!(!citation_needs_bounded_source_read(&citation));
        assert!(!citation_is_internal_synthetic_source_range(&citation));
        assert!(!promote_verified_bounded_source_citation(&mut citation));
    }

    #[test]
    fn declaration_only_behavioral_source_gets_a_bounded_forward_window() {
        assert_eq!(
            carrier_source_line_context_end_line(SnippetScopeDto::LineContext, 100),
            Some(123)
        );
        assert_eq!(
            carrier_source_line_context_end_line(SnippetScopeDto::FunctionBody, 100),
            None
        );
        assert_eq!(
            carrier_source_line_context_end_line(SnippetScopeDto::LineContext, u32::MAX - 3),
            Some(u32::MAX)
        );
    }

    #[test]
    fn parser_partial_citation_without_a_retained_receipt_is_not_support() {
        let mut retained =
            test_packet_citation("args-file", "/checkout/repos/fmt/include/fmt/args.h", 0.9);
        retained.kind = NodeKind::FILE;
        retained.evidence_tier = Some(PacketEvidenceTierDto::LexicalSource);
        retained.resolution_status = Some(PacketEvidenceResolutionDto::SourceRangeOnly);
        let mut omitted = retained.clone();
        omitted.node_id = NodeId("format-file".to_string());
        omitted.display_name = "format-file".to_string();
        omitted.file_path = Some("include/fmt/format.h".to_string());
        omitted.resolution_status = Some(PacketEvidenceResolutionDto::Resolved);
        let mut indexed = omitted.clone();
        indexed.node_id = NodeId("indexed-file".to_string());
        indexed.display_name = "indexed-file".to_string();
        indexed.file_path = Some("include/fmt/indexed.h".to_string());
        let source_support = vec![SupportUnitDto {
            id: "source:args-file:24".to_string(),
            kind: SupportUnitKindDto::SourceRange,
            summary: "source for args-file".to_string(),
            path: Some("include/fmt/args.h".to_string()),
            symbol_id: Some("args-file".to_string()),
            start_line: Some(24),
            end_line: Some(25),
            snippet: Some("source".to_string()),
            edge_kind: None,
            from_symbol: None,
            to_symbol: None,
            query: None,
        }];
        let coverage = vec![
            SourceCoverageObservationDto {
                path: "/checkout/repos/fmt/include/fmt/args.h".to_string(),
                status: SourceCoverageStatusDto::Incomplete,
                reason: Some(FileCoverageReason::ParserPartial),
                not_established_cause: None,
                observed_size: None,
                byte_cap: None,
            },
            SourceCoverageObservationDto {
                path: "include/fmt/format.h".to_string(),
                status: SourceCoverageStatusDto::Incomplete,
                reason: Some(FileCoverageReason::ParserPartial),
                not_established_cause: None,
                observed_size: None,
                byte_cap: None,
            },
            SourceCoverageObservationDto {
                path: "include/fmt/indexed.h".to_string(),
                status: SourceCoverageStatusDto::Indexed,
                reason: None,
                not_established_cause: None,
                observed_size: None,
                byte_cap: None,
            },
        ];
        let mut citations = vec![retained, omitted, indexed];

        demote_parser_partial_citations_without_source_receipts(
            None,
            &mut citations,
            &source_support,
            &coverage,
        );

        assert_eq!(citations[0].eligible_for_sufficiency, Some(true));
        assert_eq!(citations[0].loss_reason, None);
        assert_eq!(citations[1].eligible_for_sufficiency, Some(false));
        assert_eq!(
            citations[1].loss_reason.as_deref(),
            Some(SOURCE_RECEIPT_NOT_RETAINED)
        );
        assert_eq!(citations[2].eligible_for_sufficiency, Some(true));
        assert_eq!(citations[2].loss_reason, None);
    }

    fn packet_answer_fixture(question: &str, citations: Vec<AgentCitationDto>) -> AgentAnswerDto {
        AgentAnswerDto {
            source_coverage: Vec::new(),
            answer_id: "packet-fixture".to_string(),
            prompt: question.to_string(),
            summary: "Fixture packet is covered by cited anchors.".to_string(),
            freshness: Some(crate::agent::packet_freshness::fresh_index_observation()),
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
                source_freshness_telemetry: None,
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
    fn packet_budget_pins_material_command_root_before_marginal_fill() {
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

        let command_root = answer
            .citations
            .iter()
            .find(|citation| citation.display_name == "acme_deploy::run_main")
            .expect("command root")
            .node_id
            .clone();
        cap_packet_citations_with_obligation_carriers(
            &mut answer,
            &limits,
            &["acme_deploy::run_main".to_string()],
            &[command_root],
        );

        let paths = answer
            .citations
            .iter()
            .filter_map(|citation| citation.file_path.as_deref())
            .collect::<Vec<_>>();
        assert_eq!(
            answer.citations[0].display_name, "acme_deploy::run_main",
            "exact command probe should remain first: {paths:?}"
        );
        assert_eq!(answer.citations.len(), limits.max_anchors as usize);
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
    fn packet_project_relative_path_preserves_nested_monorepo_packages() {
        let project_root = Path::new("/private/tmp/fixtures/dart-lang-http");
        assert_eq!(
            packet_project_relative_display_path(
                Some(project_root),
                "/private/tmp/fixtures/dart-lang-http/pkgs/http/lib/src/client.dart",
            ),
            "pkgs/http/lib/src/client.dart"
        );

        let mut citation = test_packet_citation(
            "Client.get",
            "/private/tmp/fixtures/dart-lang-http/pkgs/http/lib/src/client.dart",
            1.0,
        );
        normalize_packet_citation_paths(project_root, std::slice::from_mut(&mut citation));
        assert_eq!(
            citation.file_path.as_deref(),
            Some("pkgs/http/lib/src/client.dart")
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
    fn packet_ranking_prefers_inbound_dispatch_callable_over_route_group() {
        let question = "Trace how an HTTP server routes an incoming request through route registration, request handler dispatch, and response finalization.";
        let mut answer = packet_answer_fixture(
            question,
            vec![
                test_packet_citation("RouteGroup.Group", "src/http/group.go", 0.95),
                test_packet_citation("ServerEngine.handleHTTPRequest", "src/http/server.go", 0.40),
            ],
        );

        rank_packet_evidence(question, &mut answer);

        assert_eq!(
            answer.citations[0].display_name,
            "ServerEngine.handleHTTPRequest"
        );
    }

    #[test]
    fn packet_ranking_drops_unrequested_wide_char_siblings() {
        let question = "Explain how a formatter turns arguments into type-erased format args and reaches format_to output paths.";
        let mut answer = packet_answer_fixture(
            question,
            vec![
                test_packet_citation("vformat_to", "include/tool/xchar.h", 0.90),
                test_packet_citation("format_to", "include/tool/format.h", 0.70),
            ],
        );

        rank_packet_evidence(question, &mut answer);

        assert!(
            answer.citations.iter().all(|citation| !citation
                .file_path
                .as_deref()
                .unwrap_or_default()
                .contains("xchar")),
            "wide-char siblings should leave the window when the question did not ask for them: {:?}",
            answer
                .citations
                .iter()
                .map(|citation| citation.file_path.as_deref())
                .collect::<Vec<_>>()
        );
        assert_eq!(answer.citations[0].display_name, "format_to");
    }

    #[test]
    fn packet_ranking_drops_unrequested_windows_and_formatting_extensions() {
        let question = "Explain how a formatter turns arguments into type-erased format args and reaches format_to output paths.";
        let mut answer = packet_answer_fixture(
            question,
            vec![
                test_packet_citation("detail::format_windows_error", "src/os.cc", 0.90),
                test_packet_citation("vformat_to", "include/tool/color.h", 0.80),
                test_packet_citation("T", "include/tool/args.h", 0.75),
                test_packet_citation("format_to", "include/tool/format.h", 0.70),
                test_packet_citation("format_error", "include/tool/format.h", 0.68),
                test_packet_citation("format_arg_store", "include/tool/base.h", 0.65),
            ],
        );

        rank_packet_evidence(question, &mut answer);

        let names: Vec<&str> = answer
            .citations
            .iter()
            .map(|citation| citation.display_name.as_str())
            .collect();
        assert!(
            !names
                .iter()
                .any(|name| name.contains("windows") || *name == "T" || *name == "vformat_to"),
            "unrequested windows, extension, and single-letter hits should leave: {names:?}"
        );
        assert!(names.contains(&"format_to"));
        assert!(names.contains(&"format_arg_store"));
        assert!(names.contains(&"format_error"));
    }

    #[test]
    fn packet_ranking_drops_unrequested_client_adapters_and_mapper_annotations() {
        let client_question = "Explain how an HTTP client exposes helpers, BaseClient convenience methods, and IOClient send behavior.";
        let mut client_answer = packet_answer_fixture(
            client_question,
            vec![
                test_packet_citation("IOClient.send", "src/http/io_client.dart", 0.90),
                test_packet_citation("CronetClient.send", "src/http/cronet_client.dart", 0.80),
                test_packet_citation("main", "flutter_http_example/lib/main.dart", 0.70),
                test_packet_citation("Client.get", "src/http/client.dart", 0.60),
            ],
        );
        rank_packet_evidence(client_question, &mut client_answer);
        assert!(client_answer.citations.iter().all(|citation| {
            let path = citation.file_path.as_deref().unwrap_or_default();
            !path.contains("cronet") && !path.contains("example")
        }));

        let mapper_question = "Explain how mapper configuration and runtime mapper APIs cooperate to map source objects to destination objects.";
        let mut mapper_answer = packet_answer_fixture(
            mapper_question,
            vec![
                test_packet_citation(
                    "MapAtRuntimeAttribute.ApplyConfiguration",
                    "src/ObjectMapping/Configuration/Annotations/MapAtRuntimeAttribute.cs",
                    0.90,
                ),
                test_packet_citation(
                    "When_mapping_to_a_destination",
                    "src/UnitTests/Indexers.cs",
                    0.80,
                ),
                test_packet_citation("Mapper.MapCore", "src/ObjectMapping/Mapper.cs", 0.70),
            ],
        );
        rank_packet_evidence(mapper_question, &mut mapper_answer);
        assert_eq!(mapper_answer.citations.len(), 1);
        assert_eq!(mapper_answer.citations[0].display_name, "Mapper.MapCore");
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
    fn packet_budget_protects_material_carriers_from_compact_cap() {
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
        let material_carriers = answer
            .citations
            .iter()
            .filter(|citation| {
                matches!(
                    citation.display_name.as_str(),
                    "run_exec_session"
                        | "ExecSharedCliOptions"
                        | "Subcommand::Exec"
                        | "codex_exec::run_main"
                        | "codex_exec::Cli"
                        | "EventProcessorWithJsonOutput"
                        | "codex_protocol::models::WebSearchAction"
                        | "ThreadStartParams"
                        | "TurnStartParams"
                )
            })
            .map(|citation| citation.node_id.clone())
            .collect::<Vec<_>>();

        rank_packet_evidence(question, &mut answer);
        let budget = apply_packet_budget_with_extra_and_obligation_carriers(
            packet_fixture_project_root(),
            question,
            PacketTaskClassDto::ArchitectureExplanation,
            PacketBudgetModeDto::Compact,
            packet_budget_limits(PacketBudgetModeDto::Compact),
            &mut answer,
            &[],
            &material_carriers,
            &[],
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
                "compact packet cap should protect material carrier citations before high-ranking distractors: {paths:?}"
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
    fn packet_budget_protects_generic_indexing_material_carriers() {
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
        let material_carriers = answer
            .citations
            .iter()
            .filter(|citation| {
                matches!(
                    citation.display_name.as_str(),
                    "indexing entrypoint"
                        | "file discovery"
                        | "symbol extraction"
                        | "storage persistence"
                        | "search projection"
                        | "snapshot refresh"
                )
            })
            .map(|citation| citation.node_id.clone())
            .collect::<Vec<_>>();

        rank_packet_evidence(question, &mut answer);
        let budget = apply_packet_budget_with_extra_and_obligation_carriers(
            packet_fixture_project_root(),
            question,
            PacketTaskClassDto::ArchitectureExplanation,
            PacketBudgetModeDto::Compact,
            packet_budget_limits(PacketBudgetModeDto::Compact),
            &mut answer,
            &[],
            &material_carriers,
            &[],
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
                "compact packet cap should protect generic indexing carrier {expected}: {display_names:?}"
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
    fn packet_budget_replaces_test_evidence_with_late_definition_files() {
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
    fn packet_budget_protects_storage_material_carriers() {
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
        let material_carriers = answer
            .citations
            .iter()
            .filter(|citation| {
                matches!(
                    citation.file_path.as_deref(),
                    Some("src/lib/data/storage/StorageAccess.h")
                        | Some("src/lib/data/storage/PersistentStorage.h")
                        | Some("src/lib/data/storage/PersistentStorage.cpp")
                )
            })
            .map(|citation| citation.node_id.clone())
            .collect::<Vec<_>>();
        let mut limits = packet_budget_limits(PacketBudgetModeDto::Compact);
        limits.max_anchors = 5;
        limits.max_files = 5;

        rank_packet_evidence(question, &mut answer);
        let budget = apply_packet_budget_with_extra_and_obligation_carriers(
            packet_fixture_project_root(),
            question,
            PacketTaskClassDto::ArchitectureExplanation,
            PacketBudgetModeDto::Compact,
            limits,
            &mut answer,
            &[],
            &material_carriers,
            &[],
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
                "storage-flow obligations should protect exact contract and implementation paths: {paths:?}"
            );
        }
        assert!(
            !paths.contains(&"src/test/StorageRegression.cpp"),
            "test filler should not displace protected production storage paths: {paths:?}"
        );
    }

    #[test]
    fn packet_budget_protects_indexing_material_carriers() {
        let question = "Explain how project/source-group configuration becomes indexing work, then how command providers create C++ and Java work items.";
        let mut citations = (0..10)
            .map(|index| {
                test_packet_citation(
                    &format!("UnrelatedNoise{index}"),
                    &format!("src/lib/noise/UnrelatedNoise{index}.h"),
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
        let material_carriers = answer
            .citations
            .iter()
            .filter(|citation| {
                matches!(
                    citation.file_path.as_deref(),
                    Some("src/lib/project/Project.cpp")
                        | Some("src/lib_cxx/project/SourceGroupCxxCdb.h")
                        | Some("src/lib_cxx/data/indexer/IndexerCommandCxx.h")
                )
            })
            .map(|citation| citation.node_id.clone())
            .collect::<Vec<_>>();
        let mut limits = packet_budget_limits(PacketBudgetModeDto::Compact);
        limits.max_anchors = 8;
        limits.max_files = 8;

        rank_packet_evidence(question, &mut answer);
        let budget = apply_packet_budget_with_extra_and_obligation_carriers(
            packet_fixture_project_root(),
            question,
            PacketTaskClassDto::ArchitectureExplanation,
            PacketBudgetModeDto::Compact,
            limits,
            &mut answer,
            &[],
            &material_carriers,
            &[],
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
                "indexing obligations should protect exact source-group and work-queue paths: {paths:?}"
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
    fn compact_flow_focus_prefers_executable_hits_over_generic_modules() {
        let mut generic_module = test_search_hit("cmp", 0.95);
        generic_module.kind = NodeKind::MODULE;
        let executable = test_search_hit("execute_request", 0.60);
        let hits = [generic_module.clone(), executable];
        let compact_request = AgentAskRequest {
            prompt: "Explain how search walks candidate files through the execution flow"
                .to_string(),
            retrieval_profile: AgentRetrievalProfileSelectionDto::Custom {
                config: AgentCustomRetrievalConfigDto::default(),
            },
            focus_node_id: None,
            max_results: None,
            response_mode: AgentResponseModeDto::Structured,
            latency_budget_ms: None,
            include_evidence: true,
            hybrid_weights: None,
        };

        assert_eq!(
            investigation_focus_node(&compact_request, &compact_request.prompt, &hits)
                .expect("compact search flow should prefer executable evidence")
                .0,
            "execute_request"
        );

        let mut ordinary_request = compact_request.clone();
        ordinary_request.retrieval_profile = AgentRetrievalProfileSelectionDto::Preset {
            preset: AgentRetrievalPresetDto::Architecture,
        };
        assert_eq!(
            investigation_focus_node(&ordinary_request, &ordinary_request.prompt, &hits)
                .expect("ordinary asks retain ranked fallback")
                .0,
            "cmp"
        );

        let explicit = NodeId("explicit".to_string());
        ordinary_request.focus_node_id = Some(explicit.clone());
        assert_eq!(
            investigation_focus_node(&ordinary_request, &ordinary_request.prompt, &hits),
            Some(explicit)
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
    fn packet_plan_uses_explicit_request_probes_without_promoting_sufficiency() {
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

        let required = packet_plan_sufficiency_extra_probes(&plan, &extra_probes);
        for expected in &extra_probes {
            assert!(
                !required
                    .iter()
                    .any(|query| query.eq_ignore_ascii_case(expected)),
                "explicit probe {expected} must not become a sufficiency requirement: {required:?}"
            );
        }
    }

    #[test]
    fn budget_values_only_owner_member_probes_with_exact_retained_evidence() {
        let question = "Explain BaseRequest finalization and IOClient send behavior.";
        let mut answer = packet_answer_fixture(
            question,
            vec![
                test_packet_citation(
                    "BaseRequest.finalize",
                    "pkgs/http/lib/src/base_request.dart",
                    0.9,
                ),
                test_packet_citation("IOClient.send", "pkgs/http/lib/src/io_client.dart", 0.9),
                test_packet_citation(
                    "BaseRequest.copy",
                    "pkgs/http/lib/src/base_request.dart",
                    0.8,
                ),
            ],
        );

        let probes = promote_retained_owner_member_probes(question, &mut answer);

        assert!(probes.contains(&"BaseRequest.finalize".to_string()));
        assert!(probes.contains(&"IOClient.send".to_string()));
        assert!(!probes.contains(&"BaseRequest.behavior".to_string()));
        assert!(!probes.contains(&"BaseRequest.copy".to_string()));
        assert!(
            answer
                .citations
                .iter()
                .filter(|citation| matches!(
                    citation.display_name.as_str(),
                    "BaseRequest.finalize" | "IOClient.send"
                ))
                .all(|citation| citation.coverage_role.as_deref()
                    == Some(PACKET_MATERIAL_OWNER_MEMBER_PROBE_ROLE))
        );
    }

    #[test]
    fn budget_protects_exact_named_schema_entities_before_source_verification() {
        let question = "Explain schema relationships between artists, albums, and invoice lines across SQL scripts.";
        let mut answer = packet_answer_fixture(
            question,
            vec![
                test_packet_citation("public.Artist", "db/schema.sql", 0.9),
                test_packet_citation("public.Artist", "db/other.sql", 0.2),
                test_packet_citation("public.InvoiceLine", "db/schema.sql", 0.8),
                test_packet_citation("public.Customer", "db/schema.sql", 0.7),
            ],
        );
        for citation in &mut answer.citations {
            citation.evidence_producer = Some("structural_sql_collector".to_string());
            citation.evidence_tier = Some(PacketEvidenceTierDto::StructuralText);
            citation.eligible_for_sufficiency = Some(false);
        }

        promote_retained_schema_entity_probes(question, &mut answer);

        assert_eq!(
            answer.citations[0].coverage_role.as_deref(),
            Some(PACKET_MATERIAL_SCHEMA_ENTITY_ROLE)
        );
        assert_eq!(
            answer.citations[2].coverage_role.as_deref(),
            Some(PACKET_MATERIAL_SCHEMA_ENTITY_ROLE)
        );
        assert_eq!(answer.citations[1].coverage_role, None);
        assert_eq!(answer.citations[3].coverage_role, None);
        let alias_names = answer
            .citations
            .iter()
            .map(|citation| citation.display_name.as_str())
            .collect::<Vec<_>>();
        assert!(alias_names.contains(&"CREATE TABLE Artist"));
        assert!(alias_names.contains(&"CREATE TABLE InvoiceLine"));
        assert!(!alias_names.contains(&"CREATE TABLE Customer"));
        assert_eq!(
            answer.citations.len(),
            4,
            "DDL spelling should rewrite the catalog citation instead of adding a duplicate alias"
        );
        assert_eq!(answer.citations[0].display_name, "CREATE TABLE Artist");
        assert_eq!(answer.citations[2].display_name, "CREATE TABLE InvoiceLine");
        assert_eq!(
            answer.citations[1].display_name, "CREATE TABLE Artist",
            "every matching catalog hit should use the DDL spelling, not only the preferred dialect"
        );
    }

    #[test]
    fn schema_entity_promotion_keeps_common_sql_dialect_scripts() {
        let question = "Explain schema relationships between artists, albums, and invoice lines across SQL scripts.";
        let mut answer = packet_answer_fixture(
            question,
            vec![
                test_packet_citation("public.Artist", "db/Chinook_Db2.sql", 0.95),
                test_packet_citation("public.Artist", "db/Chinook_Sqlite.sql", 0.4),
                test_packet_citation("seed", "db/Chinook_MySql.sql", 0.3),
                test_packet_citation("seed", "db/Chinook_PostgreSql.sql", 0.2),
            ],
        );
        for citation in &mut answer.citations {
            citation.evidence_producer = Some("structural_sql_collector".to_string());
            citation.origin = SearchHitOrigin::IndexedSymbol;
        }
        answer.citations[2].origin = SearchHitOrigin::TextMatch;
        answer.citations[3].origin = SearchHitOrigin::TextMatch;

        promote_retained_schema_entity_probes(question, &mut answer);

        let protected_paths = answer
            .citations
            .iter()
            .filter(|citation| {
                citation.coverage_role.as_deref() == Some(PACKET_MATERIAL_SCHEMA_ENTITY_ROLE)
            })
            .filter_map(|citation| citation.file_path.as_deref())
            .map(packet_display_path)
            .collect::<Vec<_>>();
        assert!(
            protected_paths.iter().any(|path| path.contains("Sqlite")),
            "sqlite dialect should be retained: {protected_paths:?}"
        );
        assert!(
            protected_paths.iter().any(|path| path.contains("MySql")),
            "mysql dialect should be retained: {protected_paths:?}"
        );
        assert!(
            protected_paths
                .iter()
                .any(|path| path.contains("PostgreSql")),
            "postgres dialect should be retained: {protected_paths:?}"
        );
        assert!(
            answer
                .citations
                .iter()
                .any(|citation| citation.display_name == "CREATE TABLE Artist"
                    && citation
                        .file_path
                        .as_deref()
                        .is_some_and(|path| packet_display_path(path).contains("Sqlite"))),
            "CREATE TABLE alias should follow the preferred dialect file: {:?}",
            answer.citations
        );
        let file_identity_names = answer
            .citations
            .iter()
            .filter(|citation| citation.kind == NodeKind::FILE)
            .map(|citation| citation.display_name.as_str())
            .collect::<Vec<_>>();
        assert!(
            file_identity_names.contains(&"db/Chinook_Sqlite.sql"),
            "retained dialect files should keep a repo-relative file identity: {file_identity_names:?}"
        );
        assert!(
            file_identity_names.contains(&"db/Chinook_MySql.sql"),
            "retained dialect files should keep a repo-relative file identity: {file_identity_names:?}"
        );
        assert!(
            file_identity_names.contains(&"db/Chinook_PostgreSql.sql"),
            "retained dialect files should keep a repo-relative file identity: {file_identity_names:?}"
        );
    }

    #[test]
    fn schema_entity_promotion_keeps_named_constraint_anchor_on_retained_dialect() {
        let question =
            "Explain schema relationships between publishers and titles across SQL scripts.";
        let mut answer = packet_answer_fixture(
            question,
            vec![
                test_packet_citation("public.Publisher", "schema/Catalog_Sqlite.sql", 0.4),
                test_packet_citation("IFK_TitlePublisherId", "schema/Catalog_MySql.sql", 0.35),
                test_packet_citation("public.title", "schema/Catalog_PostgreSql.sql", 0.2),
            ],
        );
        for citation in &mut answer.citations {
            citation.evidence_producer = Some("structural_sql_collector".to_string());
            citation.origin = SearchHitOrigin::IndexedSymbol;
            citation.kind = NodeKind::CLASS;
            citation.resolvable = true;
            citation.eligible_for_sufficiency = Some(true);
        }

        promote_retained_schema_entity_probes(question, &mut answer);
        rank_packet_evidence(question, &mut answer);

        assert!(
            answer.citations.iter().any(|citation| {
                citation.display_name == "IFK_TitlePublisherId"
                    && citation
                        .file_path
                        .as_deref()
                        .is_some_and(|path| packet_display_path(path).contains("MySql"))
            }),
            "named referential constraints on retained dialects should remain: {:?}",
            answer.citations
        );
        assert!(
            answer.citations.iter().any(|citation| {
                citation.display_name == "IFK_TitlePublisherId"
                    && citation.coverage_role.as_deref() == Some(PACKET_MATERIAL_SCHEMA_ENTITY_ROLE)
            }),
            "relationship constraints on retained dialects should keep a protected schema role: {:?}",
            answer.citations
        );
        assert!(
            answer.citations.iter().any(|citation| {
                citation.display_name.contains("CREATE TABLE")
                    && citation.file_path.as_deref().is_some_and(|path| {
                        let display = packet_display_path(path);
                        display.contains("Sqlite")
                            || display.contains("MySql")
                            || display.contains("PostgreSql")
                    })
            }),
            "retained dialects should keep a table-creation anchor: {:?}",
            answer.citations
        );
    }

    #[test]
    fn schema_file_identities_keep_relationship_constraints_on_the_same_dialect() {
        let question =
            "Explain schema relationships between publishers and titles across SQL scripts.";
        let mut answer = packet_answer_fixture(
            question,
            vec![
                test_packet_citation("public.Publisher", "schema/Catalog_Sqlite.sql", 0.4),
                test_packet_citation("public.Title", "schema/Catalog_Sqlite.sql", 0.38),
                test_packet_citation("IFK_TitlePublisherId", "schema/Catalog_Sqlite.sql", 0.2),
                test_packet_citation("public.Publisher", "schema/Catalog_MySql.sql", 0.3),
                test_packet_citation("public.Publisher", "schema/Catalog_PostgreSql.sql", 0.25),
                test_packet_citation("unrelated helper", "src/unrelated.rs", 0.9),
            ],
        );
        for citation in &mut answer.citations {
            if citation.display_name.starts_with("public.")
                || citation.display_name.starts_with("IFK_")
            {
                citation.evidence_producer = Some("structural_sql_collector".to_string());
                citation.origin = SearchHitOrigin::IndexedSymbol;
                citation.kind = NodeKind::CLASS;
                citation.resolvable = true;
                citation.eligible_for_sufficiency = Some(true);
            }
        }

        promote_retained_schema_entity_probes(question, &mut answer);
        let limits = PacketBudgetLimitsDto {
            max_anchors: 8,
            max_files: 8,
            max_snippets: 8,
            max_trail_edges: 8,
            max_output_bytes: 16 * 1024,
        };
        assert!(cap_citations(&mut answer, &limits));

        let names = answer
            .citations
            .iter()
            .map(|citation| citation.display_name.as_str())
            .collect::<Vec<_>>();
        assert!(
            names.contains(&"IFK_TitlePublisherId"),
            "dialect file identities must not evict relationship constraints: {names:?}"
        );
        assert!(
            names.contains(&"schema/Catalog_Sqlite.sql"),
            "retained dialect files should keep a repo-relative file identity: {names:?}"
        );
        assert!(
            names.iter().any(|name| name.contains("CREATE TABLE")),
            "retained dialects should keep a table-creation anchor: {names:?}"
        );
    }

    #[test]
    fn packet_ranking_drops_sql_schema_variant_copies_when_common_dialects_remain() {
        let question =
            "Explain schema relationships between publishers and titles across SQL scripts.";
        let mut answer = packet_answer_fixture(
            question,
            vec![
                test_packet_citation("CREATE TABLE Publisher", "schema/Catalog_Sqlite.sql", 0.4),
                test_packet_citation("FOREIGN KEY", "schema/Catalog_Sqlite.sql", 0.35),
                test_packet_citation("CREATE TABLE Publisher", "schema/Catalog_MySql.sql", 0.3),
                test_packet_citation("FOREIGN KEY", "schema/Catalog_PostgreSql.sql", 0.25),
                test_packet_citation(
                    "CREATE TABLE Publisher",
                    "schema/Catalog_SqliteAutoIncrementPKs.sql",
                    0.95,
                ),
                test_packet_citation(
                    "CREATE TABLE Publisher",
                    "schema/Catalog_PostgreSqlSerialPKs.sql",
                    0.9,
                ),
                test_packet_citation("CREATE TABLE Publisher", "schema/Catalog_Db2.sql", 0.85),
            ],
        );

        rank_packet_evidence(question, &mut answer);

        let paths = answer
            .citations
            .iter()
            .filter_map(|citation| citation.file_path.as_deref())
            .map(packet_display_path)
            .collect::<Vec<_>>();
        assert!(
            paths
                .iter()
                .any(|path| path.ends_with("schema/Catalog_Sqlite.sql")),
            "sqlite dialect should remain: {paths:?}"
        );
        assert!(
            paths
                .iter()
                .any(|path| path.ends_with("schema/Catalog_MySql.sql")),
            "mysql dialect should remain: {paths:?}"
        );
        assert!(
            paths
                .iter()
                .any(|path| path.ends_with("schema/Catalog_PostgreSql.sql")),
            "postgres dialect should remain: {paths:?}"
        );
        assert!(
            paths.iter().all(|path| {
                !path.to_ascii_lowercase().contains("autoincrement")
                    && !path.to_ascii_lowercase().contains("serialpks")
                    && !path.contains("Db2")
            }),
            "variant and extra-engine copies should drop: {paths:?}"
        );
        assert!(
            answer.citations.iter().any(|citation| {
                citation.display_name.contains("FOREIGN KEY")
                    && citation.file_path.as_deref().is_some_and(|path| {
                        let display = packet_display_path(path);
                        display.contains("Sqlite")
                            || display.contains("MySql")
                            || display.contains("PostgreSql")
                    })
            }),
            "FOREIGN KEY on a retained dialect should remain: {:?}",
            answer.citations
        );
        assert!(
            answer.citations.iter().any(|citation| {
                citation.display_name.contains("CREATE TABLE")
                    && citation.file_path.as_deref().is_some_and(|path| {
                        let display = packet_display_path(path);
                        display.contains("Sqlite")
                            || display.contains("MySql")
                            || display.contains("PostgreSql")
                    })
            }),
            "CREATE TABLE on a retained dialect should remain: {:?}",
            answer.citations
        );
    }

    #[test]
    fn cited_stylesheet_imports_promote_keyframe_and_variable_sheets() {
        let root = packet_temp_root("css-import-expansion");
        let _ = std::fs::remove_dir_all(&root);
        write_packet_fixture_file(
            &root,
            "styles/entry.css",
            "@import '_tokens.css';\n@import 'motion/spin.css';\n@import '../bundle.min.css';\n",
        );
        write_packet_fixture_file(
            &root,
            "styles/_tokens.css",
            ":root {\n  --motion-duration: 1s;\n}\n",
        );
        write_packet_fixture_file(
            &root,
            "styles/motion/spin.css",
            "@keyframes spin {\n  from { transform: rotate(0deg); }\n}\n.spin {\n  animation-name: spin;\n}\n",
        );
        write_packet_fixture_file(&root, "bundle.min.css", ".unused { color: red; }\n");

        let mut answer = packet_answer_fixture(
            "Explain how the stylesheet defines shared animation variables and base classes and connects named animation classes to keyframes.",
            vec![test_packet_citation("entry", "styles/entry.css", 0.4)],
        );
        answer.citations[0].file_path = Some(root.join("styles/entry.css").display().to_string());

        maybe_append_cited_stylesheet_import_citations(
            &root,
            "Explain how the stylesheet defines shared animation variables and base classes and connects named animation classes to keyframes.",
            &mut answer,
        );

        let displays = answer
            .citations
            .iter()
            .map(|citation| citation.display_name.as_str())
            .collect::<Vec<_>>();
        assert!(
            displays.iter().any(|name| name.contains("@keyframes spin")),
            "imported keyframe sheet should be cited: {displays:?}"
        );
        assert!(
            displays.contains(&".spin"),
            "imported animation class should be cited: {displays:?}"
        );
        assert!(
            displays.contains(&"--motion-duration"),
            "imported custom properties should be cited: {displays:?}"
        );
        assert!(
            displays
                .iter()
                .any(|name| packet_display_path(name).ends_with("styles/motion/spin.css")),
            "keyframe sheet path should be copyable from the citation list: {displays:?}"
        );
        assert!(
            !answer.citations.iter().any(|citation| citation
                .file_path
                .as_deref()
                .is_some_and(|path| packet_display_path(path).ends_with("bundle.min.css"))),
            "generated bundles should stay out of cited stylesheet imports: {:?}",
            answer.citations
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn cited_stylesheet_entry_sheet_promotes_import_barrel_beside_base_and_vars() {
        let root = packet_temp_root("css-entry-barrel");
        let _ = std::fs::remove_dir_all(&root);
        write_packet_fixture_file(
            &root,
            "styles/_tokens.css",
            ":root {\n  --motion-duration: 1s;\n}\n",
        );
        write_packet_fixture_file(
            &root,
            "styles/_base.css",
            ".base {\n  animation-duration: var(--motion-duration);\n}\n",
        );
        write_packet_fixture_file(
            &root,
            "styles/entry.css",
            "@import '_tokens.css';\n@import '_base.css';\n@import 'motion/spin.css';\n",
        );
        write_packet_fixture_file(
            &root,
            "styles/motion/spin.css",
            "@keyframes spin {\n  from { transform: rotate(0deg); }\n}\n.spin {\n  animation-name: spin;\n}\n",
        );
        write_packet_fixture_file(
            &root,
            "styles/motion/slide.css",
            "@keyframes slide {\n  from { transform: translateX(0); }\n}\n",
        );

        let question = "Explain how the stylesheet defines shared animation variables and base classes and connects named animation classes to keyframes.";
        let mut tokens = test_packet_citation("--motion-duration", "styles/_tokens.css", 0.9);
        tokens.file_path = Some(root.join("styles/_tokens.css").display().to_string());
        let mut base = test_packet_citation(".base", "styles/_base.css", 0.8);
        base.file_path = Some(root.join("styles/_base.css").display().to_string());
        let mut spin = test_packet_citation("@keyframes spin", "styles/motion/spin.css", 0.7);
        spin.file_path = Some(root.join("styles/motion/spin.css").display().to_string());
        let mut answer = packet_answer_fixture(question, vec![tokens, base, spin]);

        maybe_append_cited_stylesheet_import_citations(&root, question, &mut answer);
        rank_packet_evidence(question, &mut answer);

        assert!(
            answer.citations.iter().any(|citation| {
                citation.kind == NodeKind::FILE
                    && citation
                        .display_name
                        .replace('\\', "/")
                        .ends_with("styles/entry.css")
            }),
            "import barrel beside cited base/vars should be copyable: {:?}",
            answer
                .citations
                .iter()
                .map(|citation| {
                    (
                        &citation.display_name,
                        citation.kind,
                        citation.file_path.as_deref(),
                    )
                })
                .collect::<Vec<_>>()
        );
        assert!(
            answer.citations.iter().all(|citation| {
                !citation.file_path.as_deref().is_some_and(|path| {
                    packet_display_path(path).ends_with("styles/motion/slide.css")
                })
            }),
            "unimported motion sheets must stay out of the entry promotion: {:?}",
            answer.citations
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn cited_formatting_files_promote_argument_store_and_failure_types() {
        let root = packet_temp_root("cited-formatting-types");
        let _ = std::fs::remove_dir_all(&root);
        write_packet_fixture_file(
            &root,
            "include/tool/base.hpp",
            r#"
            template <typename Context, int NUM_ARGS>
            struct runtime_format_arg_store {
              void push_back();
            };
            "#,
        );
        write_packet_fixture_file(
            &root,
            "include/tool/args.hpp",
            r#"
            FMT_EXPORT template <typename Context>
            class dynamic_format_argument_store {
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
        write_packet_fixture_file(
            &root,
            "include/tool/uncited.hpp",
            r#"
            struct leftover_format_arg_store {};
            "#,
        );

        let prompt = "Explain how a formatting runtime turns arguments into type-erased format argument stores and reports formatting failure types.";
        let mut answer = packet_answer_fixture(
            prompt,
            vec![
                test_packet_citation("format_to", "include/tool/base.hpp", 0.7),
                test_packet_citation("named_arg", "include/tool/args.hpp", 0.6),
                test_packet_citation("report_error", "include/tool/errors.hpp", 0.5),
            ],
        );
        answer.citations[0].file_path =
            Some(root.join("include/tool/base.hpp").display().to_string());
        answer.citations[1].file_path =
            Some(root.join("include/tool/args.hpp").display().to_string());
        answer.citations[2].file_path =
            Some(root.join("include/tool/errors.hpp").display().to_string());

        maybe_append_cited_formatting_type_citations(&root, prompt, &mut answer);

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
                "expected cited formatting type {expected}; got {displays:?}"
            );
        }
        assert!(
            !displays.contains(&"leftover_format_arg_store"),
            "uncited files must not be scanned: {displays:?}"
        );

        let mut other = packet_answer_fixture(
            "Explain how a session sends HTTP requests.",
            vec![test_packet_citation(
                "format_to",
                "include/tool/base.hpp",
                0.7,
            )],
        );
        other.citations[0].file_path =
            Some(root.join("include/tool/base.hpp").display().to_string());
        maybe_append_cited_formatting_type_citations(
            &root,
            "Explain how a session sends HTTP requests.",
            &mut other,
        );
        assert!(
            other
                .citations
                .iter()
                .all(|citation| citation.display_name != "runtime_format_arg_store"),
            "non-formatting questions should not lift formatting types: {:?}",
            other.citations
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn cited_mapper_files_promote_mapper_interfaces() {
        let root = packet_temp_root("cited-mapper-interfaces");
        let _ = std::fs::remove_dir_all(&root);
        write_packet_fixture_file(
            &root,
            "src/ObjectMapping/RuntimeMapper.cs",
            r#"
            namespace ObjectMapping;
            public interface IRuntimeMapperBase
            {
              TDestination Map<TSource, TDestination>(TSource source);
            }
            public interface IRuntimeMapper : IRuntimeMapperBase
            {
              IConfigurationProvider ConfigurationProvider { get; }
            }
            public sealed class RuntimeMapper : IRuntimeMapper
            {
              public TDestination Map<TSource, TDestination>(TSource source) => default!;
            }
            "#,
        );
        write_packet_fixture_file(
            &root,
            "src/ObjectMapping/UncitedMapper.cs",
            "public interface ILeftoverMapper {}",
        );

        let prompt = "Explain how mapper configuration and runtime mapper APIs cooperate to map source objects to destination objects.";
        let mut answer = packet_answer_fixture(
            prompt,
            vec![test_packet_citation(
                "RuntimeMapper.Map",
                "src/ObjectMapping/RuntimeMapper.cs",
                0.7,
            )],
        );
        answer.citations[0].file_path = Some(
            root.join("src/ObjectMapping/RuntimeMapper.cs")
                .display()
                .to_string(),
        );
        maybe_append_cited_mapper_interface_citations(&root, prompt, &mut answer);
        let displays = answer
            .citations
            .iter()
            .map(|citation| citation.display_name.as_str())
            .collect::<Vec<_>>();
        assert!(
            displays.contains(&"IRuntimeMapperBase") && displays.contains(&"IRuntimeMapper"),
            "cited mapper file should promote mapper interfaces: {displays:?}"
        );
        assert!(
            !displays.contains(&"ILeftoverMapper"),
            "uncited mapper files must not be scanned: {displays:?}"
        );

        let mut graphed = packet_answer_fixture(prompt, Vec::new());
        let mut mapper_node = typed_graph_node("runtime-mapper", NodeKind::CLASS);
        mapper_node.file_path = Some(
            root.join("src/ObjectMapping/RuntimeMapper.cs")
                .display()
                .to_string(),
        );
        graphed.graphs = vec![uml_artifact(
            "mapper-flow",
            "runtime-mapper",
            vec![mapper_node],
            Vec::new(),
        )];
        maybe_append_cited_mapper_interface_citations(&root, prompt, &mut graphed);
        let graphed_displays = graphed
            .citations
            .iter()
            .map(|citation| citation.display_name.as_str())
            .collect::<Vec<_>>();
        assert!(
            graphed_displays.contains(&"IRuntimeMapperBase")
                && graphed_displays.contains(&"IRuntimeMapper"),
            "retained graph source should promote mapper interfaces: {graphed_displays:?}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn cited_client_files_follow_response_and_request_imports() {
        let root = packet_temp_root("cited-client-imports");
        let _ = std::fs::remove_dir_all(&root);
        write_packet_fixture_file(
            &root,
            "src/http/client.dart",
            r#"
            import 'base_request.dart';
            import 'response.dart';
            import 'ignore_me.dart';
            abstract interface class Client {
              Future<Response> get(Uri url);
            }
            "#,
        );
        write_packet_fixture_file(
            &root,
            "src/http/response.dart",
            "class Response {\n  final int statusCode;\n}\n",
        );
        write_packet_fixture_file(
            &root,
            "src/http/base_request.dart",
            "abstract class BaseRequest {\n  void finalize();\n}\n",
        );
        write_packet_fixture_file(&root, "src/http/ignore_me.dart", "class IgnoreMe {}\n");

        let prompt = "Explain how an HTTP client exposes helpers, BaseClient convenience methods, and IOClient send behavior.";
        let mut answer = packet_answer_fixture(
            prompt,
            vec![test_packet_citation(
                "Client.get",
                "src/http/client.dart",
                0.7,
            )],
        );
        answer.citations[0].file_path =
            Some(root.join("src/http/client.dart").display().to_string());
        maybe_append_cited_client_relative_imports(&root, prompt, &mut answer);
        let displays = answer
            .citations
            .iter()
            .map(|citation| citation.display_name.as_str())
            .collect::<Vec<_>>();
        assert!(
            displays.contains(&"Response") && displays.contains(&"BaseRequest"),
            "cited client file should follow response/request imports: {displays:?}"
        );
        assert!(
            displays
                .iter()
                .any(|name| packet_display_path(name).ends_with("src/http/response.dart"))
                && displays
                    .iter()
                    .any(|name| packet_display_path(name).ends_with("src/http/base_request.dart")),
            "imported client/request/response paths should be copyable: {displays:?}"
        );
        assert!(
            !displays.contains(&"IgnoreMe"),
            "unrelated dart imports should stay out: {displays:?}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn cited_client_barrels_prefer_exact_client_request_response_imports() {
        let root = packet_temp_root("cited-client-barrel-imports");
        let _ = std::fs::remove_dir_all(&root);
        write_packet_fixture_file(
            &root,
            "src/http/http.dart",
            r#"
            import 'client.dart';
            import 'request.dart';
            import 'response.dart';
            import 'streamed_request.dart';
            import 'streamed_response.dart';
            import 'multipart_request.dart';
            Future<Response> get(Uri url) => Client().get(url);
            "#,
        );
        write_packet_fixture_file(
            &root,
            "src/http/client.dart",
            "abstract interface class Client {\n  Future<Response> get(Uri url);\n}\n",
        );
        write_packet_fixture_file(&root, "src/http/request.dart", "class Request {}\n");
        write_packet_fixture_file(&root, "src/http/response.dart", "class Response {}\n");
        write_packet_fixture_file(
            &root,
            "src/http/streamed_request.dart",
            "class StreamedRequest {}\n",
        );
        write_packet_fixture_file(
            &root,
            "src/http/streamed_response.dart",
            "class StreamedResponse {}\n",
        );
        write_packet_fixture_file(
            &root,
            "src/http/multipart_request.dart",
            "class MultipartRequest {}\n",
        );
        write_packet_fixture_file(
            &root,
            "src/http/io_client.dart",
            "class IOClient {\n  Future<StreamedResponse> send(BaseRequest request) => throw '';\n}\n",
        );

        let prompt = "Explain how an HTTP client exposes helpers, BaseClient convenience methods, BaseRequest finalization, and IOClient send behavior.";
        let mut answer = packet_answer_fixture(
            prompt,
            vec![
                test_packet_citation("get", "src/http/http.dart", 0.9),
                test_packet_citation("IOClient.send", "src/http/io_client.dart", 0.8),
            ],
        );
        answer.citations[0].file_path = Some(root.join("src/http/http.dart").display().to_string());
        answer.citations[1].file_path =
            Some(root.join("src/http/io_client.dart").display().to_string());
        maybe_append_cited_client_relative_imports(&root, prompt, &mut answer);
        let displays = answer
            .citations
            .iter()
            .map(|citation| citation.display_name.as_str())
            .collect::<Vec<_>>();
        assert!(
            displays.contains(&"Client")
                && displays.contains(&"Request")
                && displays.contains(&"Response"),
            "exact client/request/response imports should outrank qualified siblings: {displays:?}"
        );
        assert!(
            displays
                .iter()
                .any(|name| packet_display_path(name).ends_with("src/http/client.dart")),
            "client source path should be copyable: {displays:?}"
        );
        let _ = std::fs::remove_dir_all(&root);
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
    fn compact_packet_retrieval_keeps_anchor_prompt() {
        let plan = build_packet_plan(
            "Explain how VS Code workbench startup reaches ExtensionService, ExtensionHostManager, AbstractExtHostExtensionService, and ExtHostCommands.executeCommand.",
            Some(PacketTaskClassDto::ArchitectureExplanation),
            PacketBudgetModeDto::Compact,
        );

        let prompt =
            packet_retrieval_prompt("Explain startup.", &plan, PacketBudgetModeDto::Compact);
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
    fn packet_plan_preserves_case_distinct_dot_qualified_exact_symbols() {
        let plan = build_packet_plan(
            "Find Foo.run and foo.run.",
            Some(PacketTaskClassDto::SymbolOwnership),
            PacketBudgetModeDto::Standard,
        );
        let queries = plan
            .queries
            .iter()
            .map(|query| query.query.as_str())
            .collect::<Vec<_>>();

        assert!(queries.contains(&"Foo.run"), "{queries:?}");
        assert!(queries.contains(&"foo.run"), "{queries:?}");
    }

    #[test]
    fn packet_plan_preserves_ruby_suffixes_and_cpp_destructor_identities() {
        let plan = build_packet_plan(
            "Trace Workflow::ready?, Workflow::save!, and Widget::~Widget.",
            Some(PacketTaskClassDto::SymbolOwnership),
            PacketBudgetModeDto::Standard,
        );
        let exact_queries = plan
            .queries
            .iter()
            .filter(|query| packet_plan_query_is_exact_symbol_identity(query))
            .map(|query| query.query.as_str())
            .collect::<Vec<_>>();
        let material_probe_queries = plan
            .obligations
            .query_obligations
            .iter()
            .filter(|obligation| obligation.material)
            .map(|obligation| obligation.query.as_str())
            .collect::<Vec<_>>();
        let material_binding_terms = plan
            .obligations
            .claim_obligations
            .iter()
            .filter(|obligation| obligation.material)
            .flat_map(|obligation| obligation.binding_terms.iter().map(String::as_str))
            .collect::<Vec<_>>();

        for expected in ["Workflow::ready?", "Workflow::save!", "Widget::~Widget"] {
            assert!(exact_queries.contains(&expected), "{exact_queries:?}");
            assert!(
                material_probe_queries.contains(&expected),
                "{material_probe_queries:?}"
            );
            assert!(
                material_binding_terms.contains(&expected),
                "{material_binding_terms:?}"
            );
        }
        for collapsed in ["ready", "save", "Widget::Widget"] {
            assert!(!exact_queries.contains(&collapsed), "{exact_queries:?}");
            assert!(
                !material_probe_queries.contains(&collapsed),
                "{material_probe_queries:?}"
            );
            assert!(
                !material_binding_terms.contains(&collapsed),
                "{material_binding_terms:?}"
            );
        }
    }

    #[test]
    fn packet_plan_does_not_turn_bang_comparison_into_ruby_identity() {
        for question in [
            "Compare foo_bar!=expected_value.",
            "Compare foo_bar!==expected_value.",
        ] {
            let plan = build_packet_plan(
                question,
                Some(PacketTaskClassDto::SymbolOwnership),
                PacketBudgetModeDto::Standard,
            );
            let exact_queries = plan
                .queries
                .iter()
                .filter(|query| packet_plan_query_is_exact_symbol_identity(query))
                .map(|query| query.query.as_str())
                .collect::<Vec<_>>();
            let material_probe_queries = plan
                .obligations
                .query_obligations
                .iter()
                .filter(|obligation| obligation.material)
                .map(|obligation| obligation.query.as_str())
                .collect::<Vec<_>>();
            let material_binding_terms = plan
                .obligations
                .claim_obligations
                .iter()
                .filter(|obligation| obligation.material)
                .flat_map(|obligation| obligation.binding_terms.iter().map(String::as_str))
                .collect::<Vec<_>>();

            for operands in [
                exact_queries.as_slice(),
                material_probe_queries.as_slice(),
                material_binding_terms.as_slice(),
            ] {
                assert!(operands.contains(&"foo_bar"), "{question}: {operands:?}");
                assert!(
                    operands.contains(&"expected_value"),
                    "{question}: {operands:?}"
                );
                assert!(
                    !operands.contains(&"foo_bar!"),
                    "{question} fabricated a Ruby bang identity: {operands:?}"
                );
            }
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
    fn route_tracing_packet_plan_uses_neutral_carrier_queries_without_eval_probes() {
        let question = "Trace how a server application registers middleware and routes, handles an incoming request through a router, and sends the response.";
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

        for expected in ["application use", "application handle", "response send"] {
            assert!(
                queries.contains(&expected),
                "production route tracing should carry neutral owner/action query {expected}: {queries:?}"
            );
        }
        for evaluation_only in ["createApplication", "app.use", "res.send"] {
            assert!(
                !queries.contains(&evaluation_only),
                "production route tracing must not emit evaluation-shaped query {evaluation_only}: {queries:?}"
            );
        }
    }

    #[test]
    fn packet_evidence_sections_publish_the_claim_telemetry_computed_with_the_claims() {
        let question = "Explain the packet evidence roles.";
        let citations = vec![
            test_packet_citation("CliCommand", "crates/tool-cli/src/main.rs", 0.8),
            test_packet_citation("RuntimeCoordinator", "crates/core/src/runtime.rs", 0.8),
            test_packet_citation("WorkspacePlan", "crates/core/src/workspace/plan.rs", 0.8),
            test_packet_citation("GraphIndexer", "crates/indexer/src/lib.rs", 0.8),
            test_packet_citation("ProjectionStore", "crates/store/src/projection.rs", 0.8),
        ];
        let limits = packet_budget_limits(PacketBudgetModeDto::Compact);
        let answer = packet_answer_fixture(question, citations);

        let (expected_claims, expected_telemetry) = packet_supported_claims_with_telemetry(&answer);
        assert!(
            !expected_claims.is_empty(),
            "fixture citations must produce claims, or a telemetry-free mutant is invisible"
        );
        let expected = expected_telemetry.to_dto(packet_claim_profile_registry_summary());
        let counted_claims: u32 = expected
            .claim_sources
            .iter()
            .map(|entry| entry.claims)
            .sum();
        assert!(
            counted_claims > 0,
            "the expected claim-source breakdown must be non-zero, or it cannot be told \
             apart from the defaulted telemetry of a telemetry-free claims pair"
        );

        let obligations = build_packet_obligation_plan(
            question,
            PacketTaskClassDto::ArchitectureExplanation,
            &[],
        );
        for (label, obligations) in [
            ("without obligations", None),
            ("with obligations", Some(&obligations)),
        ] {
            let mut published_answer = answer.clone();
            append_packet_evidence_sections(
                &mut published_answer,
                PacketTaskClassDto::ArchitectureExplanation,
                &limits,
                obligations,
            );
            let published = published_answer
                .retrieval_trace
                .packet_claim_profile_telemetry
                .expect("packet evidence sections must publish typed claim telemetry");
            assert_eq!(
                published, expected,
                "the {label} branch must publish the telemetry computed with the claims; \
                 a call site passing a telemetry-free claims pair (packet_supported_claims \
                 plus a defaulted telemetry) ships this all-zero breakdown instead (#1865 M1)"
            );
        }
    }

    #[test]
    fn packet_claim_profile_telemetry_stays_out_of_the_evidence_annotation_channel() {
        // Regression: the fire-rate counters were appended to `retrieval_trace.annotations`,
        // which is the packet's evidence-gap channel. The CLI classifier of the day
        // substring-matched this vocabulary, so the always-on telemetry lines
        // ("profiles_skipped_invalid=0", "skipped=0") were classified as gaps on every packet
        // and downgraded agent confidence from high/ready to medium/review universally.
        // Telemetry must therefore travel on its own typed field.
        const CLI_GAP_MARKERS: [&str; 10] = [
            "fallback",
            "gap",
            "low confidence",
            "missing",
            "no relevant",
            "skipped",
            "truncated",
            "uncertain",
            "unavailable",
            "weak",
        ];

        let limits = packet_budget_limits(PacketBudgetModeDto::Compact);
        let temp = tempfile::tempdir().expect("temp dir");
        let script_path = temp.path().join("install-runtime.sh");
        let source = "install_runtime() {\n  SOURCE_STR='[ -s \"$TOOL_DIR/runtime.sh\" ] && source \"$TOOL_DIR/runtime.sh\"'\n  download_file \"$RUNTIME_SOURCE\" \"$TOOL_DIR/runtime.sh\"\n}\n";
        std::fs::write(&script_path, source).expect("write shell fixture");
        let script_display = script_path.to_string_lossy().to_string();
        let prompt = "Trace how an install script bootstraps the shell function and dispatches install, download, and use commands.";
        let mut answer = packet_answer_fixture(
            prompt,
            vec![test_packet_citation(
                "install_runtime",
                &script_display,
                0.9,
            )],
        );
        let annotations_before = answer.retrieval_trace.annotations.clone();

        append_packet_evidence_sections(
            &mut answer,
            PacketTaskClassDto::ArchitectureExplanation,
            &limits,
            None,
        );

        assert_eq!(
            answer.retrieval_trace.annotations, annotations_before,
            "assembling packet evidence must not add telemetry to the evidence annotation channel"
        );
        // EV-6b retired prose classification, so wording alone no longer decides anything. This
        // loop is retained wording hygiene; per-producer kinds are pinned where the producers
        // actually run (see `packet_generic_source_shape_citations_are_observations_not_gaps`).
        for annotation in &answer.retrieval_trace.annotations {
            let lowered = annotation.text.to_ascii_lowercase();
            for marker in CLI_GAP_MARKERS {
                assert!(
                    !lowered.contains(marker),
                    "packet annotation `{}` reads as evidence gap `{marker}`",
                    annotation.text
                );
            }
        }
        assert!(
            answer
                .retrieval_trace
                .packet_claim_profile_telemetry
                .is_some(),
            "fire rates must still be observable, on the typed trace field"
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
            !lower.contains("ties ") && !lower.contains("adjacent ownership"),
            "production must not emit navigation boilerplate as a claim: {text}"
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
            "The packet carries independent indexing-entrypoint and runtime-orchestration source anchors.",
            "The packet carries independent runtime-orchestration and workspace-planning source anchors.",
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
        let extraction = claims
            .iter()
            .find(|claim| {
                claim.claim
                    == "Symbol extraction evidence builds graph nodes, edges, occurrences, and related source data."
            })
            .expect("symbol extraction claim");
        assert_eq!(
            extraction
                .citations
                .iter()
                .map(|citation| citation.display_name.as_str())
                .collect::<Vec<_>>(),
            ["index_file"],
            "indexing work-queue evidence cannot stand in for symbol-extraction/storage evidence"
        );
    }

    #[test]
    fn production_packet_claims_do_not_synthesize_local_real_template_claims() {
        let answer = AgentAnswerDto {
            source_coverage: Vec::new(),
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
                source_freshness_telemetry: None,
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
            source_coverage: Vec::new(),
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
                source_freshness_telemetry: None,
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
            source_coverage: Vec::new(),
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
                source_freshness_telemetry: None,
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
        assert!(
            !text.contains("ties ") && !text.contains("`getPayloadClient` in `src/lib/payload.ts`"),
            "generic source-evidence claims must be omitted rather than restated: {text}"
        );
    }

    #[test]
    fn packet_ranking_demotes_test_paths_without_fixture_specific_boosts() {
        let question = "Trace route dispatch through a handler.";
        let mut answer = AgentAnswerDto {
            source_coverage: Vec::new(),
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
                source_freshness_telemetry: None,
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
            source_coverage: Vec::new(),
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
                source_freshness_telemetry: None,
                steps: Vec::new(),
                packet_sidecar_diagnostics: Vec::new(),
                retrieval_shadow: None,
            },
        };

        rank_packet_evidence(question, &mut answer);

        assert_eq!(answer.citations.len(), 1);
        assert_eq!(
            answer.citations[0].display_name,
            "IndexService::run_indexing_blocking"
        );
    }

    #[test]
    fn packet_ranking_demotes_non_primary_roles_for_production_prompts() {
        let question = "Trace production route dispatch through the handler.";
        let mut answer = AgentAnswerDto {
            source_coverage: Vec::new(),
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
                source_freshness_telemetry: None,
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
            source_coverage: Vec::new(),
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
                source_freshness_telemetry: None,
                steps: Vec::new(),
                packet_sidecar_diagnostics: Vec::new(),
                retrieval_shadow: None,
            },
        };

        rank_packet_evidence(question, &mut answer);
        assert_eq!(answer.citations[0].display_name, "DocsRouteHandler");
    }

    #[test]
    fn packet_ranking_drops_wide_char_when_a_primary_format_header_remains() {
        let question = "Explain how formatting arguments reach vformat and format_to output paths.";
        let mut answer = packet_answer_fixture(
            question,
            vec![
                test_packet_citation("vformat_to", "include/tool/xchar.h", 0.9),
                test_packet_citation("vformat", "include/tool/format.h", 0.5),
                test_packet_citation("format_to", "include/tool/base.h", 0.4),
            ],
        );

        rank_packet_evidence(question, &mut answer);

        let paths = answer
            .citations
            .iter()
            .filter_map(|citation| citation.file_path.as_deref())
            .collect::<Vec<_>>();
        assert!(
            paths.contains(&"include/tool/format.h"),
            "primary format header should remain cited: {paths:?}"
        );
        assert!(
            paths.iter().all(|path| *path != "include/tool/xchar.h"),
            "unrequested wide-char sibling should drop: {paths:?}"
        );
    }

    #[test]
    fn flask_shaped_request_path_survives_production_packet_ranking() {
        let question = "Trace request handling through the application.";
        let mut answer = packet_answer_fixture(
            question,
            vec![
                test_packet_citation("FlaskApp.handle_request", "src/flask/app.py", 0.8),
                test_packet_citation("NativeApp.handle_request", "src/native/app.rs", 0.8),
            ],
        );

        rank_packet_evidence(question, &mut answer);

        assert_eq!(
            answer.citations[0].file_path.as_deref(),
            Some("src/flask/app.py")
        );
    }

    #[test]
    fn mixed_language_formatting_suppression_requires_a_replacement() {
        let question = "Explain how formatting arguments reach output.";
        let mut without_replacement = packet_answer_fixture(
            question,
            vec![test_packet_citation(
                "format_arguments",
                "src/formatting.py",
                0.8,
            )],
        );

        rank_packet_evidence(question, &mut without_replacement);

        assert_eq!(without_replacement.citations.len(), 1);
        assert_eq!(
            without_replacement.citations[0].file_path.as_deref(),
            Some("src/formatting.py")
        );

        let mut with_replacement = packet_answer_fixture(
            question,
            vec![
                test_packet_citation("format_arguments", "src/formatting.py", 0.8),
                test_packet_citation("format_arguments", "src/formatting.rs", 0.8),
            ],
        );

        rank_packet_evidence(question, &mut with_replacement);

        assert_eq!(
            with_replacement
                .citations
                .iter()
                .filter_map(|citation| citation.file_path.as_deref())
                .collect::<Vec<_>>(),
            vec!["src/formatting.rs"]
        );
    }

    #[test]
    fn packet_follow_up_commands_single_quote_shell_sensitive_questions() {
        let question = "Inspect $env:SECRET and $(Get-ChildItem) and 'literal'";
        let quoted = quote_packet_command_value(question);

        // Doubling the apostrophe is the PowerShell convention; `sh` joins the
        // adjacent quoted runs and deletes the character outright.
        assert_eq!(
            quoted,
            r#"'Inspect $env:SECRET and $(Get-ChildItem) and '\''literal'\'''"#
        );
        let argv = next_deeper_packet_argv(
            packet_fixture_project_root(),
            question,
            PacketBudgetModeDto::Tiny,
        )
        .expect("tiny packet should have deeper argv");
        assert_eq!(
            argv,
            vec![
                "codestory-cli".to_string(),
                "packet".to_string(),
                "--project".to_string(),
                "C:/workspace/project root".to_string(),
                "--question".to_string(),
                question.to_string(),
                "--budget".to_string(),
                "compact".to_string(),
            ],
            "the follow-up must be typed argv, with the question carried verbatim"
        );

        let command = next_deeper_packet_command(
            packet_fixture_project_root(),
            question,
            PacketBudgetModeDto::Tiny,
        )
        .expect("tiny packet should have deeper command");
        assert!(
            command.contains(r#"--question 'Inspect $env:SECRET and $(Get-ChildItem)"#),
            "packet command should single-quote shell-sensitive question text: {command}"
        );
        assert!(
            command.contains(r#"and '\''literal'\'''"#),
            "packet command must escape apostrophes so a POSIX shell preserves them: {command}"
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
    fn drill_continuation_keeps_public_packet_limits_and_bounds_retrieval_depth() {
        let ordinary = packet_budget_limits_for_request(PacketBudgetModeDto::Standard, false);
        let drill = packet_budget_limits_for_request(PacketBudgetModeDto::Standard, true);

        let expected = packet_budget_limits(PacketBudgetModeDto::Standard);
        assert_eq!(ordinary.max_anchors, expected.max_anchors);
        assert_eq!(ordinary.max_files, expected.max_files);
        assert_eq!(ordinary.max_snippets, expected.max_snippets);
        assert_eq!(ordinary.max_trail_edges, expected.max_trail_edges);
        assert_eq!(ordinary.max_output_bytes, expected.max_output_bytes);
        assert_eq!(drill.max_anchors, expected.max_anchors);
        assert_eq!(drill.max_files, expected.max_files);
        assert_eq!(drill.max_snippets, expected.max_snippets);
        assert_eq!(drill.max_trail_edges, expected.max_trail_edges);
        assert_eq!(drill.max_output_bytes, expected.max_output_bytes);

        let profile = packet_retrieval_profile(
            Some(PacketTaskClassDto::ArchitectureExplanation),
            PacketBudgetModeDto::Deep,
            &drill,
            true,
        );
        let AgentRetrievalProfileSelectionDto::Custom { config } = profile else {
            panic!("a drill continuation must use a depth-bounded custom profile");
        };
        assert_eq!(config.depth, PACKET_DRILL_MAX_DEPTH);
        assert_eq!(
            config.max_nodes,
            PACKET_DRILL_MAX_HITS * PACKET_DRILL_MAX_DEPTH
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
        answer
            .retrieval_trace
            .annotations
            .push(RetrievalAnnotationDto::observation(
                "large trace annotation should stay only on the canonical answer trace".repeat(8),
            ));
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
    fn markdown_budget_skips_tiny_diagram_intro_and_truncates_restating_sections_first() {
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

        let original_retrieval_markdown = match &answer.sections[2].blocks[0] {
            AgentResponseBlockDto::Markdown { markdown } => markdown.clone(),
            AgentResponseBlockDto::Mermaid { .. } => String::new(),
        };
        let original_diagram_intro = match &answer.sections[3].blocks[0] {
            AgentResponseBlockDto::Markdown { markdown } => markdown.clone(),
            AgentResponseBlockDto::Mermaid { .. } => String::new(),
        };
        let original_bytes = serde_json::to_vec(&answer).unwrap().len();
        let truncated = truncate_answer_markdown_to_byte_cap(&mut answer, original_bytes - 6_000);

        assert!(truncated);
        // The ledger and the claims list re-render `answer.citations` and the covered claims,
        // which survive truncation as structured fields; the retrieval evidence exists only
        // as this markdown. So the restating sections pay for the budget, not the evidence.
        for section in &answer.sections[0..2] {
            let AgentResponseBlockDto::Markdown { markdown } = &section.blocks[0] else {
                panic!("restating section should remain markdown");
            };
            assert!(
                markdown.contains(PACKET_MARKDOWN_TRUNCATION_SUFFIX.trim()),
                "restating section `{}` should absorb truncation first",
                section.id
            );
        }
        let AgentResponseBlockDto::Markdown {
            markdown: retrieval_markdown,
        } = &answer.sections[2].blocks[0]
        else {
            panic!("retrieval evidence should remain markdown");
        };
        assert_eq!(
            retrieval_markdown, &original_retrieval_markdown,
            "retrieval evidence is carried nowhere else and should survive intact"
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
                probe_resolutions: Vec::new(),
                obligations: Default::default(),
                trace: Vec::new(),
            },
            answer,
            budget,
            support: Vec::new(),
            disposition: PacketDispositionDto::supported(),
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
                .any(|annotation| annotation.text.starts_with("packet_step_trace ")),
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
                .disposition
                .omission_receipts
                .iter()
                .any(|gap| gap.contains("packet_payload") || gap.contains("output_bytes")),
            "budget remeasurement must not leave stale payload omissions on disposition: {:?}",
            packet.disposition
        );
        assert_eq!(
            packet.disposition.kind,
            codestory_contracts::api::PacketDispositionKindDto::Supported,
            "budget must not reclassify disposition"
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
            &[],
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
            annotation
                .text
                .starts_with("packet_required_file_scoped_source_citations ")
                && annotation.text.contains("appended=8")
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
            annotation
                .text
                .starts_with("packet_generic_source_shape_citations appended=")
        }));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn generic_source_shape_citation_counters_are_observations_not_evidence_gaps() {
        // EV-6b (#1746). This counter always renders `skipped_existing=`, and the retired CLI
        // classifier substring-matched "skipped", so every packet that appended generic
        // source-shape citations reported a phantom evidence gap and fell from high/ready to
        // medium/review. The counter is telemetry about the run; its kind must say so.
        let root = packet_temp_root("generic-source-shape-annotation-kind");
        let _ = std::fs::remove_dir_all(&root);
        write_packet_fixture_file(
            &root,
            "src/core/application.js",
            r#"
            service.init = function init() {
              this.router = new Router();
            };
            service.handle = function handle(req, res, callback) {
              this.router.handle(req, res, callback);
            };
            "#,
        );

        let prompt = "Trace how a server application registers middleware/routes and handles a request through router and response helpers.";
        let mut answer = packet_answer_fixture(prompt, Vec::new());
        maybe_append_generic_source_shape_citations(&root, prompt, &mut answer);

        let annotation = answer
            .retrieval_trace
            .annotations
            .iter()
            .find(|annotation| {
                annotation
                    .text
                    .starts_with("packet_generic_source_shape_citations appended=")
            })
            .cloned()
            .expect("generic source shape scan must record its counters");

        let _ = std::fs::remove_dir_all(&root);

        assert!(
            annotation.text.contains("skipped_existing="),
            "counter must still carry the word the old classifier tripped on: {}",
            annotation.text
        );
        assert_eq!(
            annotation.kind,
            RetrievalAnnotationKindDto::Observation,
            "generic source-shape counters are telemetry, not an evidence gap: {}",
            annotation.text
        );
    }

    #[test]
    fn fail_closed_retrieval_records_its_annotation_as_an_evidence_gap() {
        // EV-6c (#1775). The complement of the observation test above, driven through the real
        // `execute_retrieval` rather than a hand-built annotation. Retrieval refusing to serve
        // is the strongest possible evidence gap: the packet has no retrieval evidence at all.
        // If this producer were reclassified as an observation, `agent_gap_notes` would drop it
        // and a packet built on a refused retrieval would still report clean confidence.
        let controller = AppController::new();
        let req = AgentAskRequest {
            prompt: "Trace how the router dispatches a request".to_string(),
            retrieval_profile: AgentRetrievalProfileSelectionDto::Auto,
            focus_node_id: None,
            max_results: Some(5),
            response_mode: AgentResponseModeDto::default(),
            latency_budget_ms: Some(120_000),
            include_evidence: false,
            hybrid_weights: None,
        };
        let resolved_profile = resolve_profile(&req.prompt, &req.retrieval_profile);
        let mut trace = TraceRecorder::new(Some(120_000));

        let outcome = execute_retrieval(
            &controller,
            &req,
            &req.prompt,
            Instant::now(),
            &resolved_profile,
            &mut trace,
        );
        assert!(
            outcome.is_err(),
            "a controller with no open project must fail closed rather than serve"
        );

        let published = trace.finish(
            "ev6c-fail-closed".to_string(),
            AgentRetrievalPresetDto::Architecture,
            AgentRetrievalPolicyModeDto::CompletenessFirst,
        );
        let fail_closed = published
            .annotations
            .iter()
            .find(|annotation| {
                annotation
                    .text
                    .starts_with("retrieval_primary unavailable=true")
            })
            .unwrap_or_else(|| {
                panic!(
                    "fail-closed retrieval must annotate the trace: {:?}",
                    published.annotations
                )
            });
        assert_eq!(
            fail_closed.kind,
            RetrievalAnnotationKindDto::Gap,
            "the fail-closed retrieval annotation must be an evidence gap: {}",
            fail_closed.text
        );
        assert!(
            fail_closed.is_gap(),
            "the DTO predicate consumers read must agree: {}",
            fail_closed.text
        );
    }

    #[test]
    fn explicit_file_scoped_probe_citation_cannot_promote_sufficiency() {
        let root = packet_temp_root("explicit-source-probe-sufficiency");
        let _ = std::fs::remove_dir_all(&root);
        write_packet_fixture_file(
            &root,
            "src/session.rs",
            "impl Session {\n    fn request(&self) {}\n}\n",
        );

        let mut answer = packet_answer_fixture("fixture packet", Vec::new());
        let probes = ["src/session.rs Session::request".to_string()];
        maybe_append_required_file_scoped_source_citations(
            &root,
            "fixture packet",
            PacketTaskClassDto::RouteTracing,
            &probes,
            &probes,
            &mut answer,
        );

        let explicit = answer
            .citations
            .iter()
            .find(|citation| citation.coverage_role.as_deref() == Some("explicit source probe"))
            .expect("explicit source probe citation");
        assert_eq!(explicit.eligible_for_sufficiency, Some(false));

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
                "Example/BodyRequest.swift",
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
            &[],
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
            annotation
                .text
                .starts_with("packet_required_file_scoped_source_citations ")
                && annotation.text.contains("appended=1")
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
            &[],
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
            annotation
                .text
                .starts_with("packet_required_file_scoped_source_citations ")
                && annotation.text.contains("appended=2")
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
            &[],
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
            target: None,
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
            source_excerpt: None,
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
                evidence_tier: None,
                evidence_producer: None,
                resolution_status: None,
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
            target: None,
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
            source_excerpt: None,
            verification_targets: Vec::new(),
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
                    target: None,
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
                    source_excerpt: None,
                    verification_targets: Vec::new(),
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
                    target: None,
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
                    source_excerpt: None,
                    verification_targets: Vec::new(),
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

    // -----------------------------------------------------------------------
    // Stage 4: R3 partial-atom protection, selection, extras builder, R4
    // -----------------------------------------------------------------------

    const LOG_HANDLER_QUESTION: &str = "Trace how the logger creates a log record and dispatches it to each handler for processing.";

    fn log_handler_requirements() -> Vec<FlowRequirement> {
        crate::agent::packet_flow_requirements::packet_flow_requirements_for_terms(
            &packet_probe_terms(LOG_HANDLER_QUESTION),
            PacketTaskClassDto::ArchitectureExplanation,
        )
    }

    fn typed_graph_edge(
        id: &str,
        source: &str,
        target: &str,
        kind: EdgeKind,
        certainty: Option<&str>,
        callsite_identity: Option<&str>,
    ) -> codestory_contracts::api::GraphEdgeDto {
        codestory_contracts::api::GraphEdgeDto {
            id: EdgeId(id.to_string()),
            source: NodeId(source.to_string()),
            target: NodeId(target.to_string()),
            kind,
            confidence: None,
            certainty: certainty.map(str::to_string),
            callsite_identity: callsite_identity.map(str::to_string),
            candidate_targets: Vec::new(),
        }
    }

    fn typed_graph_node(id: &str, kind: codestory_contracts::api::NodeKind) -> GraphNodeDto {
        GraphNodeDto {
            id: NodeId(id.to_string()),
            label: id.to_string(),
            kind,
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

    fn uml_artifact(
        id: &str,
        center: &str,
        nodes: Vec<GraphNodeDto>,
        edges: Vec<codestory_contracts::api::GraphEdgeDto>,
    ) -> GraphArtifactDto {
        GraphArtifactDto::Uml {
            id: id.to_string(),
            title: id.to_string(),
            graph: GraphResponse {
                center_id: NodeId(center.to_string()),
                nodes,
                edges,
                truncated: false,
                omitted_edge_count: 0,
                canonical_layout: None,
            },
        }
    }

    /// Review-005 finding 10: a proof whose facts are all TypedRelation with
    /// non-citation endpoints has no citation carriers — protection must key
    /// on the edges and their endpoint node ids. Negative first: an edge
    /// without its receiver-owner marker matches no atom and protects
    /// nothing.
    #[test]
    fn partial_atom_protection_reaches_typed_relation_edge_endpoints() {
        let requirements = log_handler_requirements();
        let formulas = packet_flow_proof_formulas(&requirements);
        assert!(
            !formulas.is_empty(),
            "the log-handler question must carry the M formula"
        );
        let m_identity = "app/log.php:10:5:handle|syntax:php-call|receiver-owner:handler|receiver-binding:loop-element@8-14";
        let answer_with = |identity: &str| {
            let mut answer = packet_answer_fixture(LOG_HANDLER_QUESTION, Vec::new());
            answer.graphs = vec![uml_artifact(
                "log-flow",
                "owner-1",
                vec![
                    typed_graph_node("owner-1", codestory_contracts::api::NodeKind::METHOD),
                    typed_graph_node("handler-1", codestory_contracts::api::NodeKind::METHOD),
                ],
                vec![typed_graph_edge(
                    "dispatch-edge",
                    "owner-1",
                    "handler-1",
                    EdgeKind::CALL,
                    Some("certain"),
                    Some(identity),
                )],
            )];
            answer
        };

        let unmarked = answer_with("app/log.php:10:5:handle|syntax:php-call");
        let protection = packet_partial_atom_protection_with_planned(
            &formulas,
            &unmarked,
            &PacketProofEvidenceExtras::default(),
            &[],
            &[],
        );
        assert!(
            protection.carrier_node_ids.is_empty() && protection.edge_ids.is_empty(),
            "an edge failing the atom patterns must protect nothing: {protection:?}"
        );

        let marked = answer_with(m_identity);
        let protection = packet_partial_atom_protection_with_planned(
            &formulas,
            &marked,
            &PacketProofEvidenceExtras::default(),
            &[],
            &[],
        );
        assert_eq!(
            protection.carrier_node_ids,
            vec![NodeId("owner-1".into()), NodeId("handler-1".into())],
            "both TypedRelation endpoints are protected carriers (finding 10)"
        );
        assert_eq!(protection.edge_ids, vec![EdgeId("dispatch-edge".into())]);
        let owner_cover = protection
            .carrier_atom_cover
            .iter()
            .find(|(node_id, _)| node_id.0 == "owner-1")
            .map(|(_, atoms)| atoms.clone())
            .expect("owner atom cover");
        assert!(
            owner_cover.contains(&ProofAtomId::M2) && owner_cover.contains(&ProofAtomId::M3),
            "the M2/M3 dispatch edge covers both handler_processing atoms: {owner_cover:?}"
        );
    }

    /// The gate-critical C shape end to end at the R3 boundary: the full
    /// css_animation_structure group (C2+C3+C4, including C3's absence over
    /// the depth-2 covering scan and its MEMBER witness) proves under
    /// provisional anchors, and the protection output covers the NARROWED
    /// ledger scan set — including recorded coverage edges that are NOT
    /// discharged facts — so the caps cannot void C3's coverage (F3
    /// finding 3).
    #[test]
    fn css_structure_partial_proof_protects_narrowed_scan_coverage_sets() {
        use crate::agent::packet_candidate::{PacketCandidateTrailScan, PacketGraphDirection};

        let css_requirements =
            crate::agent::packet_flow_requirements::packet_flow_requirements_for_terms(
                &packet_probe_terms(
                    "Trace how the css animation keyframes and custom property variables are declared and used by the base selectors in the imported stylesheets.",
                ),
                PacketTaskClassDto::ArchitectureExplanation,
            );
        let formulas = packet_flow_proof_formulas(&css_requirements);
        assert!(
            !formulas.is_empty(),
            "css question must carry the C formula"
        );

        let mut answer = packet_answer_fixture("css", Vec::new());
        answer.graphs = vec![uml_artifact(
            "packet-atom-hydration-base",
            "base",
            vec![
                typed_graph_node("entry", codestory_contracts::api::NodeKind::FILE),
                typed_graph_node("vars", codestory_contracts::api::NodeKind::FILE),
                typed_graph_node("base", codestory_contracts::api::NodeKind::FILE),
                typed_graph_node("anim", codestory_contracts::api::NodeKind::FILE),
                typed_graph_node("var-node", codestory_contracts::api::NodeKind::VARIABLE),
                typed_graph_node("sb", codestory_contracts::api::NodeKind::CONSTANT),
                typed_graph_node("sb2", codestory_contracts::api::NodeKind::CONSTANT),
                typed_graph_node("sa", codestory_contracts::api::NodeKind::CONSTANT),
                typed_graph_node("kf", codestory_contracts::api::NodeKind::FUNCTION),
            ],
            vec![
                typed_graph_edge("e1", "entry", "vars", EdgeKind::IMPORT, None, None),
                typed_graph_edge("e2", "vars", "var-node", EdgeKind::MEMBER, None, None),
                typed_graph_edge("e3", "entry", "base", EdgeKind::IMPORT, None, None),
                typed_graph_edge("e4", "base", "sb", EdgeKind::MEMBER, None, None),
                typed_graph_edge("e5", "sb", "var-node", EdgeKind::USAGE, None, None),
                typed_graph_edge("e6", "entry", "anim", EdgeKind::IMPORT, None, None),
                typed_graph_edge("e7", "anim", "kf", EdgeKind::MEMBER, None, None),
                typed_graph_edge("e8", "anim", "sa", EdgeKind::MEMBER, None, None),
                typed_graph_edge("e9", "sa", "kf", EdgeKind::USAGE, None, None),
                typed_graph_edge("e10", "base", "sb2", EdgeKind::MEMBER, None, None),
                typed_graph_edge("e11", "sb2", "var-node", EdgeKind::USAGE, None, None),
            ],
        )];

        // The depth-2 covering scan over the base stylesheet, with the
        // MEMBER witness coverage attached (what the extras builder would
        // produce from the post-pass ledger).
        let base_scan_coverage = TrailCoverage::Scanned {
            root: NodeId("base".into()),
            traversal_kinds: vec![EdgeKind::MEMBER, EdgeKind::USAGE, EdgeKind::IMPORT],
            direction: ProofTrailDirection::Outgoing,
            depth: 2,
            truncated: false,
        };
        let coverage_extras = PacketProofEvidenceExtras {
            trail_scans: vec![base_scan_coverage.clone()],
            edge_coverage: [(EdgeId("e4".into()), base_scan_coverage.clone())]
                .into_iter()
                .collect(),
            anchored_receipts: Vec::new(),
        };
        let provisional = |atom: ProofAtomId, node: &str| PlannedAtomAnchor {
            atom,
            owner: NodeId(node.to_string()),
            symbol: NodeId(node.to_string()),
            line: 3,
            receipt: VerifiedSourceAspectReceipt {
                kind: SourceAspectKind::VerifiedCarrierRange,
                owner: NodeId(node.to_string()),
                symbol_id: Some(NodeId(node.to_string())),
                start_line: Some(3),
                end_line: Some(3),
                atom_anchor: Some(atom),
            },
        };
        let planned = vec![
            provisional(ProofAtomId::C2, "var-node"),
            provisional(ProofAtomId::C4, "kf"),
        ];
        // The post-pass ledger's narrowed set for the base scan: the USAGE
        // absence subjects plus the MEMBER witnesses — including edges that
        // are NOT discharged proof facts (e10, e11).
        let ledger: Vec<(String, Vec<PacketCandidateTrailScan>)> = vec![(
            "packet-atom-hydration-base".to_string(),
            vec![PacketCandidateTrailScan {
                root: "base".into(),
                direction: PacketGraphDirection::Outgoing,
                depth: 2,
                edge_kinds: vec![EdgeKind::MEMBER, EdgeKind::USAGE, EdgeKind::IMPORT],
                truncated: false,
                coverage_edge_ids: vec![
                    EdgeId("e5".into()),
                    EdgeId("e11".into()),
                    EdgeId("e4".into()),
                    EdgeId("e10".into()),
                ],
            }],
        )];

        let protection = packet_partial_atom_protection_with_planned(
            &formulas,
            &answer,
            &coverage_extras,
            &planned,
            &ledger,
        );

        // The structure group proved: every bound carrier is protected.
        for carrier in [
            "entry", "vars", "var-node", "base", "sb", "anim", "kf", "sa",
        ] {
            assert!(
                protection
                    .carrier_node_ids
                    .iter()
                    .any(|node_id| node_id.0 == carrier),
                "carrier {carrier} must be protected: {:?}",
                protection.carrier_node_ids
            );
        }
        let sb_cover = protection
            .carrier_atom_cover
            .iter()
            .find(|(node_id, _)| node_id.0 == "sb")
            .map(|(_, atoms)| atoms.clone())
            .expect("sb atom cover");
        assert!(sb_cover.contains(&ProofAtomId::C3));
        // Discharged typed facts are protected...
        for edge in ["e1", "e2", "e3", "e4", "e5", "e6", "e7", "e8", "e9"] {
            assert!(
                protection.edge_ids.iter().any(|edge_id| edge_id.0 == edge),
                "proof edge {edge} must be protected: {:?}",
                protection.edge_ids
            );
        }
        // ...AND the covering scan's narrowed recorded set, including edges
        // that are not proof facts — losing either to a cap would void C3's
        // coverage at finalize.
        for edge in ["e10", "e11"] {
            assert!(
                protection.edge_ids.iter().any(|edge_id| edge_id.0 == edge),
                "narrowed scan-coverage edge {edge} must be protected: {:?}",
                protection.edge_ids
            );
        }
    }

    /// The selection step is a deterministic weighted set cover: snapshot
    /// carriers keep their priority and order, the best-covering partial
    /// carrier comes next, and the leftovers fill by existing citation rank.
    #[test]
    fn protected_carrier_selection_orders_by_atom_cover_then_rank() {
        let mut answer = packet_answer_fixture(LOG_HANDLER_QUESTION, Vec::new());
        answer.citations = vec![
            test_packet_citation("rank-zero", "src/a.php", 0.9),
            test_packet_citation("wide-cover", "src/b.php", 0.8),
            test_packet_citation("narrow-cover", "src/c.php", 0.7),
            test_packet_citation("tail-carrier", "src/d.php", 0.6),
        ];
        let partial = PacketPartialAtomProtection {
            carrier_node_ids: vec![
                NodeId("narrow-cover".into()),
                NodeId("wide-cover".into()),
                NodeId("tail-carrier".into()),
            ],
            edge_ids: vec![EdgeId("edge-b".into())],
            carrier_atom_cover: vec![
                (
                    NodeId("narrow-cover".into()),
                    BTreeSet::from([ProofAtomId::M3]),
                ),
                (
                    NodeId("wide-cover".into()),
                    BTreeSet::from([ProofAtomId::M2, ProofAtomId::M3]),
                ),
                (NodeId("tail-carrier".into()), BTreeSet::new()),
            ],
        };
        let snapshot_carriers = [NodeId("rank-zero".into())];
        let snapshot_edges = [EdgeId("edge-a".into())];
        let (ordered, edges) = select_protected_obligation_carriers(
            &answer,
            &[],
            &snapshot_carriers,
            &snapshot_edges,
            &partial,
        );
        assert_eq!(
            ordered
                .iter()
                .map(|node_id| node_id.0.as_str())
                .collect::<Vec<_>>(),
            ["rank-zero", "wide-cover", "narrow-cover", "tail-carrier"],
            "snapshot first, then greedy cover, then rank-ordered tail"
        );
        assert_eq!(
            edges,
            vec![EdgeId("edge-a".into()), EdgeId("edge-b".into())],
            "snapshot edges keep priority over partial-atom edges"
        );
        // Determinism.
        let (again, _) = select_protected_obligation_carriers(
            &answer,
            &[],
            &snapshot_carriers,
            &snapshot_edges,
            &partial,
        );
        assert_eq!(ordered, again);
    }

    /// The extras builder enforces the evidence-completeness obligation over
    /// the NARROWED coverage sets (F3 finding 3): a scan loses its coverage
    /// only when a RECORDED edge — an absence subject or a depth-2 MEMBER
    /// witness — left the live graphs (negative first); an INCIDENTAL
    /// enumerated edge capped out of the graphs (the IMPORT edge here, absent
    /// from the live artifact entirely) does not void it. Per-edge coverage
    /// comes from untruncated scans only, and dropping the whole artifact
    /// drops its scans.
    #[test]
    fn extras_builder_refuses_scans_whose_enumeration_lost_edges() {
        use crate::agent::packet_candidate::{
            PacketAtomHydrationSpec, PacketCandidateTrailScan, PacketGraphDirection,
            PacketProofSession,
        };

        let session = PacketProofSession::new(PacketAtomHydrationSpec::default());
        session.record_artifact_scans(
            "artifact-live",
            &[
                PacketCandidateTrailScan {
                    root: "1".into(),
                    direction: PacketGraphDirection::Outgoing,
                    depth: 2,
                    edge_kinds: vec![EdgeKind::MEMBER, EdgeKind::USAGE, EdgeKind::IMPORT],
                    truncated: false,
                    coverage_edge_ids: vec![EdgeId("101".into()), EdgeId("103".into())],
                },
                PacketCandidateTrailScan {
                    root: "1".into(),
                    direction: PacketGraphDirection::Incoming,
                    depth: 2,
                    edge_kinds: vec![EdgeKind::MEMBER, EdgeKind::USAGE, EdgeKind::IMPORT],
                    truncated: false,
                    // 999 was enumerated but never merged / later capped out.
                    coverage_edge_ids: vec![EdgeId("102".into()), EdgeId("999".into())],
                },
                PacketCandidateTrailScan {
                    root: "1".into(),
                    direction: PacketGraphDirection::Incoming,
                    depth: 1,
                    edge_kinds: vec![EdgeKind::MEMBER],
                    truncated: true,
                    coverage_edge_ids: vec![EdgeId("101".into())],
                },
            ],
        );
        session.record_artifact_scans(
            "artifact-dropped",
            &[PacketCandidateTrailScan {
                root: "7".into(),
                direction: PacketGraphDirection::Outgoing,
                depth: 1,
                edge_kinds: vec![EdgeKind::USAGE],
                truncated: false,
                coverage_edge_ids: vec![EdgeId("101".into())],
            }],
        );

        let mut answer = packet_answer_fixture(LOG_HANDLER_QUESTION, Vec::new());
        // The incidental IMPORT edge 102 the trails also enumerated has been
        // capped out of the live graphs — it is NOT in any recorded coverage
        // set, so it must not void the outgoing scan (F3 finding 3).
        answer.graphs = vec![uml_artifact(
            "artifact-live",
            "1",
            vec![
                typed_graph_node("1", codestory_contracts::api::NodeKind::FILE),
                typed_graph_node("3", codestory_contracts::api::NodeKind::CONSTANT),
                typed_graph_node("6", codestory_contracts::api::NodeKind::VARIABLE),
            ],
            vec![
                typed_graph_edge("101", "1", "3", EdgeKind::MEMBER, None, None),
                typed_graph_edge("103", "3", "6", EdgeKind::USAGE, None, None),
            ],
        )];

        let extras = build_packet_proof_evidence_extras(&answer, &session, Vec::new());
        assert_eq!(
            extras.trail_scans.len(),
            2,
            "the coverage-incomplete scan and the dropped artifact's scan must be refused, \
             while the incidental-edge drop keeps the outgoing scan attached: {extras:?}"
        );
        assert!(extras.trail_scans.iter().all(|scan| matches!(
            scan,
            TrailCoverage::Scanned { root, .. } if root.0 == "1"
        )));
        assert!(
            extras.trail_scans.iter().any(|scan| matches!(
                scan,
                TrailCoverage::Scanned {
                    depth: 2,
                    truncated: false,
                    ..
                }
            )),
            "the narrowed outgoing scan survives the incidental IMPORT drop: {extras:?}"
        );
        // Per-edge coverage only from the untruncated complete scan.
        assert!(extras.edge_coverage.contains_key(&EdgeId("101".into())));
        assert!(extras.edge_coverage.contains_key(&EdgeId("103".into())));
        assert!(
            !extras.edge_coverage.contains_key(&EdgeId("102".into())),
            "the incomplete incoming scan must not attach coverage"
        );
        let Some(TrailCoverage::Scanned { truncated, .. }) =
            extras.edge_coverage.get(&EdgeId("101".into()))
        else {
            panic!("expected scanned coverage");
        };
        assert!(
            !truncated,
            "edge coverage must come from the untruncated scan, not the truncated one"
        );

        // The anchored receipts ride through unchanged.
        let anchored = vec![VerifiedSourceAspectReceipt {
            kind: SourceAspectKind::VerifiedCarrierRange,
            owner: NodeId("1".into()),
            symbol_id: Some(NodeId("3".into())),
            start_line: Some(5),
            end_line: Some(9),
            atom_anchor: Some(ProofAtomId::C2),
        }];
        let extras = build_packet_proof_evidence_extras(&answer, &session, anchored.clone());
        assert_eq!(extras.anchored_receipts, anchored);
    }

    /// R4 planning: provisional anchors are derived only for the carriers the
    /// formula atoms name — the C2 variable, the C4 keyframe, and the C1
    /// import-statement window — never for unrelated nodes, and nodes
    /// without a declaration line are skipped (fail closed).
    #[test]
    fn planned_anchor_candidates_cover_only_atom_named_carriers() {
        let css_requirements =
            crate::agent::packet_flow_requirements::packet_flow_requirements_for_terms(
                &packet_probe_terms(
                    "Trace how the css animation keyframes and custom property variables are declared and used by the base selectors in the imported stylesheets.",
                ),
                PacketTaskClassDto::ArchitectureExplanation,
            );
        let formulas = packet_flow_proof_formulas(&css_requirements);
        assert!(
            !formulas.is_empty(),
            "css question must carry the C formula"
        );

        let mut answer = packet_answer_fixture("css", Vec::new());
        answer.graphs = vec![uml_artifact(
            "css-artifact",
            "entry",
            vec![
                typed_graph_node("entry", codestory_contracts::api::NodeKind::FILE),
                typed_graph_node("vars", codestory_contracts::api::NodeKind::FILE),
                typed_graph_node("var-node", codestory_contracts::api::NodeKind::VARIABLE),
                typed_graph_node("keyframe", codestory_contracts::api::NodeKind::FUNCTION),
                typed_graph_node("stmt", codestory_contracts::api::NodeKind::MODULE),
                typed_graph_node("unrelated", codestory_contracts::api::NodeKind::CLASS),
            ],
            vec![
                typed_graph_edge("m-var", "vars", "var-node", EdgeKind::MEMBER, None, None),
                typed_graph_edge("m-key", "anim", "keyframe", EdgeKind::MEMBER, None, None),
                typed_graph_edge("m-stmt", "entry", "stmt", EdgeKind::MEMBER, None, None),
                typed_graph_edge(
                    "m-unrelated",
                    "entry",
                    "unrelated",
                    EdgeKind::MEMBER,
                    None,
                    None,
                ),
                typed_graph_edge(
                    "m-lineless",
                    "vars",
                    "lineless",
                    EdgeKind::MEMBER,
                    None,
                    None,
                ),
            ],
        )];
        // `lineless` is VARIABLE-kind but has no declaration line.
        if let GraphArtifactDto::Uml { graph, .. } = &mut answer.graphs[0] {
            graph.nodes.push(typed_graph_node(
                "lineless",
                codestory_contracts::api::NodeKind::VARIABLE,
            ));
        }
        let lines: HashMap<&str, u32> = [
            ("var-node", 3),
            ("keyframe", 12),
            ("stmt", 2),
            ("unrelated", 40),
        ]
        .into_iter()
        .collect();
        let planned = planned_atom_anchor_candidates_with_lines(&answer, &formulas, |node_id| {
            lines.get(node_id.0.as_str()).copied()
        });

        let planned_keys = planned
            .iter()
            .map(|anchor| {
                (
                    anchor.atom,
                    anchor.owner.0.as_str(),
                    anchor.symbol.0.as_str(),
                )
            })
            .collect::<Vec<_>>();
        assert!(planned_keys.contains(&(ProofAtomId::C2, "var-node", "var-node")));
        assert!(planned_keys.contains(&(ProofAtomId::C4, "keyframe", "keyframe")));
        assert!(planned_keys.contains(&(ProofAtomId::C1, "entry", "stmt")));
        assert!(
            !planned_keys
                .iter()
                .any(|(_, owner, symbol)| *owner == "unrelated" || *symbol == "unrelated"),
            "a CLASS member is not an atom-named carrier: {planned_keys:?}"
        );
        assert!(
            !planned_keys
                .iter()
                .any(|(_, _, symbol)| *symbol == "lineless"),
            "a node without a declaration line cannot be anchored: {planned_keys:?}"
        );
        for anchor in &planned {
            assert_eq!(anchor.receipt.atom_anchor, Some(anchor.atom));
            assert_eq!(anchor.receipt.start_line, Some(anchor.line));
        }
    }

    /// R4 verification shares the carrier-source budgets and fails closed:
    /// negative first — a window not starting at the anchored declaration
    /// line, a failed read, and an over-budget window all yield NO receipt
    /// (with budget drops counted for the step-trace annotation); a lawful
    /// window yields one anchored receipt, one rendered entry, and one
    /// SourceRead step.
    #[test]
    fn anchor_verification_shares_budgets_and_fails_closed_on_dishonest_windows() {
        let planned = |atom: ProofAtomId, node: &str, line: u32| PlannedAtomAnchor {
            atom,
            owner: NodeId(node.to_string()),
            symbol: NodeId(node.to_string()),
            line,
            receipt: VerifiedSourceAspectReceipt {
                kind: SourceAspectKind::VerifiedCarrierRange,
                owner: NodeId(node.to_string()),
                symbol_id: Some(NodeId(node.to_string())),
                start_line: Some(line),
                end_line: Some(line),
                atom_anchor: Some(atom),
            },
        };
        let lawful = planned(ProofAtomId::C2, "var-node", 3);
        let misaligned = planned(ProofAtomId::C4, "keyframe", 12);
        let unreadable = planned(ProofAtomId::C1, "stmt", 2);
        let selected = vec![&lawful, &misaligned, &unreadable];
        let limits = packet_budget_limits(PacketBudgetModeDto::Compact);
        let mut rendered = String::new();
        let mut steps = Vec::new();
        let mut outcome = PacketAtomAnchorOutcome {
            receipts: Vec::new(),
            planned: 0,
            budget_dropped: 0,
        };
        let mut source_support = Vec::new();
        verify_planned_atom_anchors(
            &selected,
            None,
            &HashMap::new(),
            &limits,
            &mut rendered,
            &mut steps,
            &mut source_support,
            &mut outcome,
            |anchor| match anchor.symbol.0.as_str() {
                // The lawful window starts exactly at the declaration line.
                "var-node" => Some((
                    "styles/_vars.css".to_string(),
                    "  3 | --hero-color: #fff;\n  4 | more".to_string(),
                )),
                // A window whose first numbered line is NOT the anchor line.
                "keyframe" => Some((
                    "styles/animate.css".to_string(),
                    " 14 | from {}\n 15 | to {}".to_string(),
                )),
                // The read fails.
                _ => None,
            },
        );
        assert_eq!(outcome.planned, 3);
        assert_eq!(outcome.receipts.len(), 1, "{outcome:?}");
        let receipt = &outcome.receipts[0];
        assert_eq!(receipt.atom_anchor, Some(ProofAtomId::C2));
        assert_eq!(
            receipt.symbol_id.as_ref().map(|id| id.0.as_str()),
            Some("var-node")
        );
        assert_eq!((receipt.start_line, receipt.end_line), (Some(3), Some(4)));
        assert_eq!(steps.len(), 1, "one SourceRead step per verified anchor");
        assert!(rendered.contains("(atom-anchored)"));
        assert_eq!(
            outcome.budget_dropped, 0,
            "failed reads are fail-closed, not budget drops"
        );
        // F3 finding 9: the verified window rides packet.support as a
        // structured SourceRange unit — one per verified anchor, none for
        // fail-closed ones.
        assert_eq!(source_support.len(), 1, "{source_support:?}");
        let unit = &source_support[0];
        assert_eq!(unit.id, "atom-anchor:C2:var-node:3");
        assert_eq!(unit.kind, SupportUnitKindDto::SourceRange);
        assert_eq!(unit.symbol_id.as_deref(), Some("var-node"));
        assert_eq!((unit.start_line, unit.end_line), (Some(3), Some(4)));
        assert!(
            unit.snippet
                .as_deref()
                .is_some_and(|snippet| snippet.contains("--hero-color"))
        );

        // Snippet-count budget: with the step budget exhausted every anchor
        // is dropped and counted, and nothing is read at all.
        let exhausted = PacketBudgetLimitsDto {
            max_snippets: 0,
            ..limits.clone()
        };
        let mut rendered = String::new();
        let mut steps = Vec::new();
        let mut source_support = Vec::new();
        let mut outcome = PacketAtomAnchorOutcome {
            receipts: Vec::new(),
            planned: 0,
            budget_dropped: 0,
        };
        verify_planned_atom_anchors(
            &selected,
            None,
            &HashMap::new(),
            &exhausted,
            &mut rendered,
            &mut steps,
            &mut source_support,
            &mut outcome,
            |_| panic!("an exhausted snippet budget must not read source"),
        );
        assert_eq!(outcome.budget_dropped, 3);
        assert!(outcome.receipts.is_empty() && rendered.is_empty() && steps.is_empty());
        assert!(source_support.is_empty());
    }
}
