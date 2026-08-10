use crate::agent::packet_capping::cap_packet_citations_with_obligation_carriers;
use crate::agent::packet_claims::{
    packet_flow_claims_markdown, packet_supported_claims_with_telemetry,
};
use crate::agent::packet_obligations::{
    bind_claims_to_packet_obligations, finalize_packet_obligation_plan,
    packet_claims_with_obligation_receipts,
};
use crate::agent::packet_plan::{packet_explicit_request_probe_queries, push_unique_term};
use crate::agent::packet_probe::exact_packet_probe_paths;
use crate::agent::packet_required_probes::packet_sufficiency_required_probe_queries_with_extra;
use crate::agent::packet_sufficiency::{
    PACKET_MARKDOWN_TRUNCATION_SUFFIX, build_packet_sufficiency_with_obligation_context,
};
use crate::agent::path_identity::RuntimeWorkspacePathIdentity;
use crate::agent::trace_export::{
    PACKET_STEP_TRACE_ANNOTATION_PREFIX, compact_retained_packet_step_trace_for_budget,
    packet_retrieval_trace_summary, retain_packet_step_trace_for_export,
};
use codestory_contracts::api::{
    AgentAnswerDto, AgentPacketDto, AgentResponseBlockDto, AgentRetrievalStepKindDto,
    AgentRetrievalStepStatusDto, EdgeId, GraphArtifactDto, GraphResponse, PacketBudgetDto,
    PacketBudgetLimitsDto, PacketBudgetModeDto, PacketBudgetUsageDto, PacketTaskClassDto,
    RetrievalShadowDto, RetrievalStageTimingDto,
};
use std::collections::{HashMap, HashSet};
use std::path::Path;

#[allow(unused_imports)]
pub(crate) use codestory_agent::packet_command::next_deeper_packet_argv;
pub(crate) use codestory_agent::packet_command::next_deeper_packet_command;

const MARKDOWN_TRUNCATION_FLOOR_BYTES: usize = 256;
const ANSWER_RETRIEVAL_DIAGNOSTICS_OMISSION: &str = "answer.retrieval_trace.diagnostics";
const AVOID_OPENING_OMISSION: &str = "avoid_opening";
const COVERAGE_REPORT_INELIGIBLE_OMISSION: &str = "coverage_report.ineligible";
const RETRIEVAL_TRACE_SUMMARY_OMISSION: &str = "retrieval_trace_summary";
const RETAINED_STEP_TRACE_DETAIL_OMISSION: &str = "answer.retrieval_trace.step_details";
const PACKET_PROOF_RESERVE_PERCENT: usize = 70;
const PACKET_GRAPH_MAX_PERCENT: usize = 20;
const PACKET_DIAGNOSTICS_MAX_PERCENT: usize = 10;

pub(crate) fn packet_budget_limits(mode: PacketBudgetModeDto) -> PacketBudgetLimitsDto {
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
pub(crate) fn apply_packet_budget(
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

#[cfg(test)]
pub(crate) fn apply_packet_budget_with_extra(
    project_root: &Path,
    question: &str,
    task_class: PacketTaskClassDto,
    requested: PacketBudgetModeDto,
    limits: PacketBudgetLimitsDto,
    answer: &mut AgentAnswerDto,
    extra_probes: &[String],
) -> PacketBudgetDto {
    apply_packet_budget_with_extra_and_obligation_carriers(
        project_root,
        question,
        task_class,
        requested,
        limits,
        answer,
        extra_probes,
        &[],
        &[],
    )
}

pub(crate) fn apply_packet_budget_with_extra_and_obligation_carriers(
    project_root: &Path,
    question: &str,
    task_class: PacketTaskClassDto,
    requested: PacketBudgetModeDto,
    limits: PacketBudgetLimitsDto,
    answer: &mut AgentAnswerDto,
    extra_probes: &[String],
    obligation_carrier_node_ids: &[codestory_contracts::api::NodeId],
    obligation_edge_ids: &[EdgeId],
) -> PacketBudgetDto {
    let mut truncated = false;
    let mut omitted_sections = Vec::new();

    let mut protected_probe_queries = Vec::new();
    for probe in
        packet_sufficiency_required_probe_queries_with_extra(question, task_class, extra_probes)
    {
        push_unique_term(&mut protected_probe_queries, &probe);
    }
    if cap_packet_citations_with_obligation_carriers(
        answer,
        &limits,
        &protected_probe_queries,
        obligation_carrier_node_ids,
    ) {
        truncated = true;
        omitted_sections.push("citations".to_string());
    }
    if cap_graph_edges(answer, limits.max_trail_edges, obligation_edge_ids) {
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

pub(crate) fn enforce_packet_output_budget(project_root: &Path, packet: &mut AgentPacketDto) {
    enforce_packet_output_budget_for_representation(project_root, packet, serialized_packet_len);
}

/// Enforce the packet cap against one adapter's complete serialized representation.
///
/// The default runtime path measures compact packet JSON. Adapters whose public wire shape adds
/// formatting or an envelope can supply that exact measurement without duplicating packet
/// trimming or changing the representation reported by other adapters.
pub fn enforce_packet_output_budget_for_representation(
    project_root: &Path,
    packet: &mut AgentPacketDto,
    representation_len: impl Fn(&AgentPacketDto) -> usize,
) {
    let extra_probes = packet_explicit_request_probe_queries(&packet.plan);
    let section_budget_changed = enforce_packet_section_budgets(packet, &representation_len);
    let mut needs_dependent_rebuild = section_budget_changed
        || packet
            .budget
            .omitted_sections
            .iter()
            .any(|section| section == "output_bytes" || section == "packet_payload");
    let mut dependent_shape_rebuilt = false;
    loop {
        let output_bytes = if needs_dependent_rebuild {
            dependent_shape_rebuilt = true;
            refresh_packet_after_budget_mutation(
                project_root,
                packet,
                &extra_probes,
                &representation_len,
            )
        } else {
            refresh_packet_budget_usage_for_representation(packet, &representation_len)
        };
        if output_bytes <= packet.budget.limits.max_output_bytes as usize {
            let hard_omission_present = packet
                .budget
                .omitted_sections
                .iter()
                .any(|section| section == "output_bytes" || section == "packet_payload");
            if !hard_omission_present {
                if !dependent_shape_rebuilt {
                    needs_dependent_rebuild = true;
                    continue;
                }
                return;
            }

            // The omission receipt participates in sufficiency and obligation rendering. Probe
            // the fully rebuilt marker-free shape before committing its removal: it can be larger
            // than the marker-present shape. If that shape does not fit, the measured marker shape
            // is the truthful irreducible result.
            let marker_shape = packet.clone();
            remove_omitted_section(&mut packet.budget, "output_bytes");
            remove_omitted_section(&mut packet.budget, "packet_payload");
            let marker_free_output_bytes = refresh_packet_after_budget_mutation(
                project_root,
                packet,
                &extra_probes,
                &representation_len,
            );
            if marker_free_output_bytes <= packet.budget.limits.max_output_bytes as usize {
                return;
            }
            *packet = marker_shape;
            return;
        }

        let hard_budget_state_changed = !packet.budget.truncated
            || !packet
                .budget
                .omitted_sections
                .iter()
                .any(|section| section == "output_bytes")
            || !packet
                .budget
                .omitted_sections
                .iter()
                .any(|section| section == "packet_payload");
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

        let mut structurally_trimmed = false;
        let trimmed_verbose_sections = trim_packet_sufficiency_verbose_lists(packet);
        if !trimmed_verbose_sections.is_empty() {
            for section in trimmed_verbose_sections {
                push_omitted_section(&mut packet.budget, section);
            }
            structurally_trimmed = true;
        } else if trim_packet_retrieval_trace_summary(packet) {
            push_omitted_section(&mut packet.budget, RETRIEVAL_TRACE_SUMMARY_OMISSION);
            structurally_trimmed = true;
        } else if trim_packet_answer_retrieval_diagnostics(packet) {
            push_omitted_section(&mut packet.budget, ANSWER_RETRIEVAL_DIAGNOSTICS_OMISSION);
            structurally_trimmed = true;
        } else if trim_one_optional_graph_unit(packet) {
            push_omitted_section(&mut packet.budget, "trail_edges");
            structurally_trimmed = true;
        } else if truncate_answer_markdown_to_byte_cap(&mut packet.answer, next_answer_cap) {
            push_omitted_section(&mut packet.budget, "markdown_blocks");
            structurally_trimmed = true;
        } else if compact_retained_packet_step_trace_for_budget(
            &mut packet.answer.retrieval_trace,
            // The omission receipt and dependent rebuild also consume output bytes. The loop
            // remeasures the exact adapter shape, so this is only bounded headroom rather than an
            // estimate used as the final acceptance decision.
            over_by.saturating_add(256),
        ) {
            push_omitted_section(&mut packet.budget, RETAINED_STEP_TRACE_DETAIL_OMISSION);
            structurally_trimmed = true;
        }

        if structurally_trimmed {
            needs_dependent_rebuild = true;
            continue;
        }
        if hard_budget_state_changed {
            // No structural trim was available in the pre-marker dependent shape. Rebuild the
            // newly installed hard receipt before deciding whether that measured shape fits.
            needs_dependent_rebuild = true;
            continue;
        }
        return;
    }
}

/// Keep optional packet sections inside their shares of the exact adapter envelope before the
/// hard-cap fixpoint starts. Material carrier edges are proof, not optional graph detail, so they
/// are excluded from the graph share and survive every section-budget trim.
fn enforce_packet_section_budgets(
    packet: &mut AgentPacketDto,
    representation_len: &impl Fn(&AgentPacketDto) -> usize,
) -> bool {
    debug_assert_eq!(
        PACKET_PROOF_RESERVE_PERCENT + PACKET_GRAPH_MAX_PERCENT + PACKET_DIAGNOSTICS_MAX_PERCENT,
        100
    );
    let envelope_bytes = packet_fixed_envelope_bytes(packet, representation_len);
    let remaining_bytes =
        (packet.budget.limits.max_output_bytes as usize).saturating_sub(envelope_bytes);
    let graph_cap = remaining_bytes.saturating_mul(PACKET_GRAPH_MAX_PERCENT) / 100;
    let diagnostics_cap = remaining_bytes.saturating_mul(PACKET_DIAGNOSTICS_MAX_PERCENT) / 100;
    let mut changed = false;

    while packet_optional_diagnostics_bytes(packet, representation_len) > diagnostics_cap {
        let trimmed_sections = trim_packet_sufficiency_verbose_lists(packet);
        if !trimmed_sections.is_empty() {
            for section in trimmed_sections {
                push_omitted_section(&mut packet.budget, section);
            }
        } else if trim_packet_retrieval_trace_summary(packet) {
            push_omitted_section(&mut packet.budget, RETRIEVAL_TRACE_SUMMARY_OMISSION);
        } else if trim_packet_answer_retrieval_diagnostics(packet) {
            push_omitted_section(&mut packet.budget, ANSWER_RETRIEVAL_DIAGNOSTICS_OMISSION);
        } else {
            break;
        }
        packet.budget.truncated = true;
        changed = true;
    }

    while packet_optional_graph_bytes(packet, representation_len) > graph_cap {
        if !trim_one_optional_graph_unit(packet) {
            break;
        }
        packet.budget.truncated = true;
        push_omitted_section(&mut packet.budget, "trail_edges");
        changed = true;
    }

    changed
}

fn packet_fixed_envelope_bytes(
    packet: &AgentPacketDto,
    representation_len: &impl Fn(&AgentPacketDto) -> usize,
) -> usize {
    let mut envelope = packet.clone();
    envelope.answer.summary.clear();
    envelope.answer.source_coverage.clear();
    envelope.answer.sections.clear();
    envelope.answer.citations.clear();
    envelope.answer.subgraph_ids.clear();
    envelope.answer.graphs.clear();
    strip_optional_packet_diagnostics(&mut envelope);
    representation_len(&envelope)
}

fn packet_optional_diagnostics_bytes(
    packet: &AgentPacketDto,
    representation_len: &impl Fn(&AgentPacketDto) -> usize,
) -> usize {
    let full = representation_len(packet);
    let mut without_optional = packet.clone();
    strip_optional_packet_diagnostics(&mut without_optional);
    full.saturating_sub(representation_len(&without_optional))
}

fn strip_optional_packet_diagnostics(packet: &mut AgentPacketDto) {
    let _ = trim_packet_sufficiency_verbose_lists(packet);
    let _ = trim_packet_retrieval_trace_summary(packet);
    let _ = trim_packet_answer_retrieval_diagnostics(packet);
}

fn packet_obligation_edge_ids(packet: &AgentPacketDto) -> Vec<EdgeId> {
    let mut seen = HashSet::new();
    packet
        .plan
        .obligations
        .claim_obligations
        .iter()
        .flat_map(|obligation| obligation.carrier_edge_proofs.iter())
        .filter_map(|proof| {
            if seen.insert(proof.edge_id.clone()) {
                Some(proof.edge_id.clone())
            } else {
                None
            }
        })
        .collect()
}

fn packet_optional_graph_bytes(
    packet: &AgentPacketDto,
    representation_len: &impl Fn(&AgentPacketDto) -> usize,
) -> usize {
    let full = representation_len(packet);
    let protected = packet_obligation_edge_ids(packet);
    let protected = protected.into_iter().collect::<HashSet<_>>();
    let mut proof_only = packet.clone();
    retain_required_graph_proof_only(&mut proof_only.answer, &protected);
    full.saturating_sub(representation_len(&proof_only))
}

fn retain_required_graph_proof_only(answer: &mut AgentAnswerDto, protected: &HashSet<EdgeId>) {
    answer.graphs.retain_mut(|artifact| {
        let GraphArtifactDto::Uml { graph, .. } = artifact else {
            return false;
        };
        graph.edges.retain(|edge| protected.contains(&edge.id));
        if graph.edges.is_empty() {
            return false;
        }
        let _ = prune_graph_to_retained_edges(graph);
        true
    });
}

fn trim_one_optional_graph_unit(packet: &mut AgentPacketDto) -> bool {
    let protected = packet_obligation_edge_ids(packet);
    let protected_set = protected.iter().cloned().collect::<HashSet<_>>();

    if let Some(index) = packet
        .answer
        .graphs
        .iter()
        .rposition(|artifact| matches!(artifact, GraphArtifactDto::Mermaid { .. }))
    {
        packet.answer.graphs.remove(index);
        return true;
    }
    if let Some(index) = packet
        .answer
        .graphs
        .iter()
        .rposition(|artifact| match artifact {
            GraphArtifactDto::Uml { graph, .. } => {
                !graph
                    .edges
                    .iter()
                    .any(|edge| protected_set.contains(&edge.id))
                    && graph.edges.is_empty()
            }
            GraphArtifactDto::Mermaid { .. } => false,
        })
    {
        packet.answer.graphs.remove(index);
        return true;
    }

    let total_edges = packet
        .answer
        .graphs
        .iter()
        .filter_map(|artifact| match artifact {
            GraphArtifactDto::Uml { graph, .. } => Some(graph.edges.len()),
            GraphArtifactDto::Mermaid { .. } => None,
        })
        .sum::<usize>();
    let protected_present = packet
        .answer
        .graphs
        .iter()
        .filter_map(|artifact| match artifact {
            GraphArtifactDto::Uml { graph, .. } => Some(graph.edges.iter()),
            GraphArtifactDto::Mermaid { .. } => None,
        })
        .flatten()
        .filter(|edge| protected_set.contains(&edge.id))
        .count();
    if total_edges <= protected_present {
        return false;
    }
    cap_graph_edges(
        &mut packet.answer,
        total_edges.saturating_sub(1).try_into().unwrap_or(u32::MAX),
        &protected,
    )
}

fn refresh_packet_after_budget_mutation(
    project_root: &Path,
    packet: &mut AgentPacketDto,
    extra_probes: &[String],
    representation_len: &impl Fn(&AgentPacketDto) -> usize,
) -> usize {
    rebuild_packet_budget_dependents(project_root, packet, extra_probes);
    refresh_packet_budget_usage_for_representation(packet, representation_len)
}

fn refresh_packet_budget_usage_for_representation(
    packet: &mut AgentPacketDto,
    representation_len: &impl Fn(&AgentPacketDto) -> usize,
) -> usize {
    let adapter_output_bytes = packet.budget.used.output_bytes;
    packet.budget.used = packet_budget_usage(&packet.answer);
    packet.budget.used.output_bytes = adapter_output_bytes;
    refresh_packet_output_bytes(packet, representation_len)
}

fn trim_packet_sufficiency_verbose_lists(packet: &mut AgentPacketDto) -> Vec<&'static str> {
    let mut trimmed_sections = Vec::new();

    if !packet.sufficiency.avoid_opening.is_empty()
        || !packet.sufficiency.avoid_opening_paths.is_empty()
    {
        packet.sufficiency.avoid_opening.clear();
        packet.sufficiency.avoid_opening_paths.clear();
        trimmed_sections.push(AVOID_OPENING_OMISSION);
    }

    if let Some(report) = packet.sufficiency.coverage_report.as_mut()
        && !report.ineligible.is_empty()
    {
        report.ineligible.clear();
        trimmed_sections.push(COVERAGE_REPORT_INELIGIBLE_OMISSION);
    }

    trimmed_sections
}

fn trim_packet_retrieval_trace_summary(packet: &mut AgentPacketDto) -> bool {
    let trace = &mut packet.retrieval_trace_summary.retrieval_trace;
    let shadow_trimmed = trace
        .retrieval_shadow
        .as_mut()
        .is_some_and(trim_retrieval_shadow_verbose_diagnostics);
    let trimmed = !trace.request_id.is_empty()
        || trace.semantic_fallback_count != 0
        || !trace.semantic_fallbacks.is_empty()
        || !trace.annotations.is_empty()
        || !trace.steps.is_empty()
        || !trace.packet_sidecar_diagnostics.is_empty()
        || shadow_trimmed;

    if trimmed {
        trace.request_id.clear();
        trace.semantic_fallback_count = 0;
        trace.semantic_fallbacks.clear();
        trace.annotations.clear();
        trace.steps.clear();
        trace.packet_sidecar_diagnostics.clear();
    }

    trimmed
}

fn trim_packet_answer_retrieval_diagnostics(packet: &mut AgentPacketDto) -> bool {
    let trace = &mut packet.answer.retrieval_trace;
    let original_annotation_count = trace.annotations.len();
    // Gaps affect sufficiency, and the scalar packet-step record is the retained provenance for
    // the full trace before its verbose steps are removed. Other observations are duplicate
    // diagnostics and can be discarded under the public payload cap.
    trace.annotations.retain(|annotation| {
        annotation.is_gap()
            || annotation
                .text
                .starts_with(PACKET_STEP_TRACE_ANNOTATION_PREFIX)
    });

    let shadow_trimmed = trace
        .retrieval_shadow
        .as_mut()
        .is_some_and(trim_retrieval_shadow_verbose_diagnostics);
    let steps_trimmed = retain_packet_step_trace_for_export(trace);
    let trimmed =
        original_annotation_count != trace.annotations.len() || steps_trimmed || shadow_trimmed;
    trace.steps.clear();
    trimmed
}

fn trim_retrieval_shadow_verbose_diagnostics(shadow: &mut RetrievalShadowDto) -> bool {
    let mut stage_details_trimmed = false;
    for stage in &mut shadow.stage_timings {
        stage_details_trimmed |= trim_retrieval_stage_verbose_diagnostics(stage);
    }
    let trimmed = stage_details_trimmed
        || !shadow.candidates.is_empty()
        || !shadow.would_rank.is_empty()
        || !shadow.candidate_resolution_counts.is_empty();
    shadow.candidates.clear();
    shadow.would_rank.clear();
    shadow.candidate_resolution_counts.clear();
    trimmed
}

fn trim_retrieval_stage_verbose_diagnostics(stage: &mut RetrievalStageTimingDto) -> bool {
    let trimmed = stage.deadline_ms.is_some()
        || stage.admission_wait_ms.is_some()
        || stage.queue_wait_ms.is_some()
        || stage.execution_ms.is_some()
        || stage.candidates_added != 0
        || stage.marginal_gain != 0.0
        || stage.sidecar_latency_ms.is_some();
    stage.deadline_ms = None;
    stage.admission_wait_ms = None;
    stage.queue_wait_ms = None;
    stage.execution_ms = None;
    stage.candidates_added = 0;
    stage.marginal_gain = 0.0;
    stage.sidecar_latency_ms = None;
    trimmed
}

fn rebuild_packet_budget_dependents(
    project_root: &Path,
    packet: &mut AgentPacketDto,
    extra_probes: &[String],
) {
    packet.retrieval_trace_summary = packet_retrieval_trace_summary(&packet.answer);
    let exact_probe_paths = exact_packet_probe_paths(&packet.plan.probe_resolutions);
    let task_class = packet
        .task_class
        .unwrap_or(PacketTaskClassDto::ArchitectureExplanation);
    finalize_packet_obligation_plan(
        &packet.question,
        task_class,
        &mut packet.plan.obligations,
        &packet.answer,
        &packet.budget,
    );
    refresh_packet_claim_markdown(packet);
    packet.sufficiency = build_packet_sufficiency_with_obligation_context(
        &RuntimeWorkspacePathIdentity,
        project_root,
        &packet.question,
        task_class,
        &packet.answer,
        &packet.budget,
        extra_probes,
        &exact_probe_paths,
        &packet.plan.obligations,
    );
    let trim_avoid_opening = packet
        .budget
        .omitted_sections
        .iter()
        .any(|section| section == AVOID_OPENING_OMISSION);
    let trim_ineligible = packet
        .budget
        .omitted_sections
        .iter()
        .any(|section| section == COVERAGE_REPORT_INELIGIBLE_OMISSION);
    let trim_trace_summary = packet
        .budget
        .omitted_sections
        .iter()
        .any(|section| section == RETRIEVAL_TRACE_SUMMARY_OMISSION);

    if trim_avoid_opening {
        packet.sufficiency.avoid_opening.clear();
        packet.sufficiency.avoid_opening_paths.clear();
    }
    if trim_ineligible && let Some(report) = packet.sufficiency.coverage_report.as_mut() {
        report.ineligible.clear();
    }
    if trim_trace_summary {
        let _ = trim_packet_retrieval_trace_summary(packet);
    }
}

fn refresh_packet_claim_markdown(packet: &mut AgentPacketDto) {
    let supported_claims_with_telemetry = packet_supported_claims_with_telemetry(&packet.answer);
    let mut claims = packet_claims_with_obligation_receipts(
        &packet.answer,
        &packet.plan.obligations,
        supported_claims_with_telemetry,
    );
    bind_claims_to_packet_obligations(&packet.plan.obligations, &mut claims);
    let Some(markdown) = packet
        .answer
        .sections
        .iter_mut()
        .find(|section| section.id == "packet-flow-claims")
        .and_then(|section| {
            section.blocks.iter_mut().find_map(|block| match block {
                AgentResponseBlockDto::Markdown { markdown } => Some(markdown),
                AgentResponseBlockDto::Mermaid { .. } => None,
            })
        })
    else {
        return;
    };

    let retained_prefix_bytes = markdown
        .strip_suffix(PACKET_MARKDOWN_TRUNCATION_SUFFIX)
        .map(str::len);
    let mut refreshed = packet_flow_claims_markdown(&claims);
    if let Some(retained_prefix_bytes) = retained_prefix_bytes
        && retained_prefix_bytes < refreshed.len()
    {
        let mut boundary = retained_prefix_bytes;
        while boundary > 0 && !refreshed.is_char_boundary(boundary) {
            boundary -= 1;
        }
        refreshed.truncate(boundary);
        refreshed.push_str(PACKET_MARKDOWN_TRUNCATION_SUFFIX);
    }
    *markdown = refreshed;
}

fn refresh_packet_output_bytes(
    packet: &mut AgentPacketDto,
    representation_len: &impl Fn(&AgentPacketDto) -> usize,
) -> usize {
    for _ in 0..4 {
        let output_bytes = representation_len(packet);
        let output_bytes_u32 = output_bytes.try_into().unwrap_or(u32::MAX);
        if packet.budget.used.output_bytes == output_bytes_u32 {
            return output_bytes;
        }
        packet.budget.used.output_bytes = output_bytes_u32;
    }
    representation_len(packet)
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

fn remove_omitted_section(budget: &mut PacketBudgetDto, section: &str) -> bool {
    let original_len = budget.omitted_sections.len();
    budget
        .omitted_sections
        .retain(|existing| existing != section);
    budget.omitted_sections.len() != original_len
}

fn cap_graph_edges(
    answer: &mut AgentAnswerDto,
    max_edges: u32,
    protected_edge_ids: &[EdgeId],
) -> bool {
    let protected_order = protected_edge_ids
        .iter()
        .enumerate()
        .map(|(index, edge_id)| (edge_id.clone(), index))
        .collect::<HashMap<_, _>>();
    let present_edge_ids = answer
        .graphs
        .iter()
        .filter_map(|artifact| match artifact {
            GraphArtifactDto::Uml { graph, .. } => Some(graph.edges.iter()),
            GraphArtifactDto::Mermaid { .. } => None,
        })
        .flatten()
        .map(|edge| edge.id.clone())
        .collect::<HashSet<_>>();
    let selected_protected = protected_edge_ids
        .iter()
        .filter(|edge_id| present_edge_ids.contains(*edge_id))
        .take(max_edges as usize)
        .cloned()
        .collect::<HashSet<_>>();
    let mut remaining = (max_edges as usize).saturating_sub(selected_protected.len());
    let mut truncated = false;
    for artifact in &mut answer.graphs {
        let GraphArtifactDto::Uml { graph, .. } = artifact else {
            continue;
        };
        let original_len = graph.edges.len();
        graph.edges.sort_by(|left, right| {
            let left_order = protected_order.get(&left.id).copied();
            let right_order = protected_order.get(&right.id).copied();
            match (left_order, right_order) {
                (Some(left), Some(right)) => left.cmp(&right),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            }
        });
        let mut retained = Vec::with_capacity(graph.edges.len().min(max_edges as usize));
        for edge in graph.edges.drain(..) {
            if selected_protected.contains(&edge.id) {
                retained.push(edge);
            } else if remaining > 0 {
                retained.push(edge);
                remaining -= 1;
            }
        }
        graph.edges = retained;
        if graph.edges.len() < original_len {
            let omitted = original_len - graph.edges.len();
            graph.truncated = true;
            graph.omitted_edge_count = graph
                .omitted_edge_count
                .saturating_add(omitted.try_into().unwrap_or(u32::MAX));
            truncated = true;
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

pub(crate) fn truncate_answer_markdown_to_byte_cap(
    answer: &mut AgentAnswerDto,
    byte_cap: usize,
) -> bool {
    let mut truncated = false;
    for _ in 0..8 {
        let Ok(bytes) = serde_json::to_vec(answer) else {
            return truncated;
        };
        if bytes.len() <= byte_cap {
            return truncated;
        }
        let Some((section_index, block_index, len)) = next_markdown_truncation_candidate(answer)
        else {
            return truncated;
        };
        if len <= MARKDOWN_TRUNCATION_FLOOR_BYTES {
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

fn next_markdown_truncation_candidate(answer: &AgentAnswerDto) -> Option<(usize, usize, usize)> {
    let mut candidate = None;
    for (section_index, section) in answer.sections.iter().enumerate() {
        for (block_index, block) in section.blocks.iter().enumerate() {
            if let AgentResponseBlockDto::Markdown { markdown } = block {
                let len = markdown.len();
                if len <= MARKDOWN_TRUNCATION_FLOOR_BYTES {
                    continue;
                }
                let priority = packet_markdown_truncation_priority(section.id.as_str());
                if candidate.is_none_or(|(_, _, existing_priority, existing_len)| {
                    priority < existing_priority
                        || (priority == existing_priority && len > existing_len)
                }) {
                    candidate = Some((section_index, block_index, priority, len));
                }
            }
        }
    }
    candidate.map(|(section_index, block_index, _, len)| (section_index, block_index, len))
}

fn packet_markdown_truncation_priority(section_id: &str) -> u8 {
    if section_id == "diagrams" {
        return 0;
    }
    if section_id == "retrieval-evidence" || section_id.starts_with("packet-subquery-") {
        return 1;
    }
    if section_id == "packet-evidence-ledger" || section_id == "packet-flow-claims" {
        return 10;
    }
    5
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

pub(crate) fn packet_budget_usage(answer: &AgentAnswerDto) -> PacketBudgetUsageDto {
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

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::agent::packet_obligations::build_packet_obligation_plan;
    use crate::agent::trace_export::{packet_step_trace_json, write_packet_step_trace_from_env};
    use codestory_contracts::api::{
        AgentCitationDto, AgentResponseSectionDto, AgentRetrievalPolicyModeDto,
        AgentRetrievalPresetDto, AgentRetrievalStepDto, AgentRetrievalTraceDto, EdgeId, EdgeKind,
        GraphEdgeDto, GraphNodeDto, NodeId, NodeKind, PacketClaimDto, PacketClaimObligationDto,
        PacketClaimObligationKindDto, PacketCoverageReportDto, PacketEvidenceResolutionDto,
        PacketEvidenceTierDto, PacketObligationCarrierEdgeProofDto, PacketObligationProofStatusDto,
        PacketPlanDto, PacketPlanQueryDto, PacketProbeDto, PacketProbeRejectionCodeDto,
        PacketProbeRejectionDto, PacketProbeResolutionDto, PacketProbeResolutionStatusDto,
        PacketQueryCompletionDto, PacketRetrievalTraceSummaryDto, PacketSidecarQueryDiagnosticDto,
        PacketSufficiencyDto, PacketSufficiencyStatusDto, SearchHitOrigin,
    };

    fn budget_graph_node(id: &str) -> GraphNodeDto {
        GraphNodeDto {
            id: NodeId(id.to_string()),
            label: id.to_string(),
            kind: NodeKind::FUNCTION,
            depth: 1,
            label_policy: None,
            badge_visible_members: None,
            badge_total_members: None,
            merged_symbol_examples: Vec::new(),
            file_path: Some(format!("src/{id}.rs")),
            qualified_name: Some(id.to_string()),
            member_access: None,
        }
    }

    fn budget_graph_artifact(id: &str, edge_ids: &[&str]) -> GraphArtifactDto {
        let center = format!("{id}-center");
        let mut nodes = vec![budget_graph_node(&center)];
        let edges = edge_ids
            .iter()
            .map(|edge_id| {
                let target = format!("{id}-{edge_id}-target");
                nodes.push(budget_graph_node(&target));
                GraphEdgeDto {
                    id: EdgeId((*edge_id).to_string()),
                    source: NodeId(center.clone()),
                    target: NodeId(target),
                    kind: EdgeKind::CALL,
                    confidence: Some(1.0),
                    certainty: Some("certain".to_string()),
                    callsite_identity: None,
                    candidate_targets: Vec::new(),
                }
            })
            .collect();
        GraphArtifactDto::Uml {
            id: id.to_string(),
            title: id.to_string(),
            graph: GraphResponse {
                center_id: NodeId(center),
                nodes,
                edges,
                truncated: false,
                omitted_edge_count: 0,
                canonical_layout: None,
            },
        }
    }

    #[test]
    fn graph_cap_reserves_material_proof_edges_before_unrelated_artifact_order() {
        let mut packet = test_packet("Trace material proof edges.", 96 * 1024);
        packet.answer.graphs = vec![
            budget_graph_artifact("first", &["ordinary-a", "ordinary-b"]),
            budget_graph_artifact("second", &["material-proof"]),
        ];
        let protected = EdgeId("material-proof".to_string());

        assert!(cap_graph_edges(
            &mut packet.answer,
            2,
            std::slice::from_ref(&protected),
        ));
        let retained = packet
            .answer
            .graphs
            .iter()
            .filter_map(|artifact| match artifact {
                GraphArtifactDto::Uml { graph, .. } => Some(graph.edges.iter()),
                GraphArtifactDto::Mermaid { .. } => None,
            })
            .flatten()
            .map(|edge| edge.id.clone())
            .collect::<Vec<_>>();
        assert_eq!(retained.len(), 2);
        assert!(retained.contains(&protected));
    }

    #[test]
    fn section_budgets_reserve_proof_before_optional_graph_and_diagnostics() {
        let mut packet = test_packet("Trace a material dispatch proof.", 96 * 1024);
        let required_edge = EdgeId("required-call".to_string());
        packet.answer.graphs = vec![budget_graph_artifact(
            "dispatch",
            &[
                "ordinary-00",
                "ordinary-01",
                "ordinary-02",
                "ordinary-03",
                "ordinary-04",
                "ordinary-05",
                "ordinary-06",
                "ordinary-07",
                "ordinary-08",
                "ordinary-09",
                "ordinary-10",
                "ordinary-11",
                "ordinary-12",
                "ordinary-13",
                "ordinary-14",
                "ordinary-15",
                "ordinary-16",
                "ordinary-17",
                "ordinary-18",
                "required-call",
            ],
        )];
        packet.plan.obligations.claim_obligations = vec![PacketClaimObligationDto {
            id: "material-dispatch".to_string(),
            kind: PacketClaimObligationKindDto::Dispatch,
            binding_terms: vec!["dispatch".to_string()],
            probe_binding: None,
            material: true,
            allowed_node_kinds: vec![NodeKind::FUNCTION],
            required_edge_kind: Some(EdgeKind::CALL),
            requires_complete_discovery: false,
            proof_status: PacketObligationProofStatusDto::Proven,
            reason: None,
            carrier_node_ids: vec![NodeId("dispatch-center".to_string())],
            carrier_paths: vec!["src/dispatch-center.rs".to_string()],
            carrier_edge_proofs: vec![PacketObligationCarrierEdgeProofDto {
                carrier_node_id: NodeId("dispatch-center".to_string()),
                edge_id: required_edge.clone(),
                edge_kind: EdgeKind::CALL,
            }],
            open_next_candidates: Vec::new(),
        }];
        for index in 0..24 {
            packet.answer.retrieval_trace.annotations.push(
                codestory_contracts::api::RetrievalAnnotationDto::observation(format!(
                    "optional diagnostic {index}: {}",
                    "detail ".repeat(80)
                )),
            );
        }
        packet.retrieval_trace_summary = packet_retrieval_trace_summary(&packet.answer);

        let envelope_bytes = packet_fixed_envelope_bytes(&packet, &serialized_packet_len);
        let remaining_bytes = 16 * 1024;
        packet.budget.limits.max_output_bytes = (envelope_bytes + remaining_bytes)
            .try_into()
            .expect("test cap");
        let graph_cap = remaining_bytes * PACKET_GRAPH_MAX_PERCENT / 100;
        let diagnostics_cap = remaining_bytes * PACKET_DIAGNOSTICS_MAX_PERCENT / 100;
        assert!(packet_optional_graph_bytes(&packet, &serialized_packet_len) > graph_cap);
        assert!(
            packet_optional_diagnostics_bytes(&packet, &serialized_packet_len) > diagnostics_cap
        );

        assert!(enforce_packet_section_budgets(
            &mut packet,
            &serialized_packet_len,
        ));

        assert!(packet_optional_graph_bytes(&packet, &serialized_packet_len) <= graph_cap);
        assert!(
            packet_optional_diagnostics_bytes(&packet, &serialized_packet_len) <= diagnostics_cap
        );
        assert_eq!(
            packet.answer.citations.len(),
            2,
            "proof citations must survive"
        );
        assert!(packet.answer.graphs.iter().any(|artifact| match artifact {
            GraphArtifactDto::Uml { graph, .. } => {
                graph.edges.iter().any(|edge| edge.id == required_edge)
            }
            GraphArtifactDto::Mermaid { .. } => false,
        }));
        assert!(
            packet
                .budget
                .omitted_sections
                .iter()
                .any(|section| section == "trail_edges")
        );
        assert!(
            packet
                .budget
                .omitted_sections
                .iter()
                .any(|section| section == ANSWER_RETRIEVAL_DIAGNOSTICS_OMISSION)
        );
    }

    #[test]
    fn post_budget_rebuild_demotes_retained_false_safe_architecture_packet() {
        let question = "Explain the ownership boundary from the packaged CodeStory plugin request through stdio transport, runtime grounding orchestration, retrieval, and evidence publication. Identify uncertainty or gaps.";
        let project_root = Path::new("/workspace/CodeStory");
        let launcher_path = "plugins/codestory/scripts/launcher.mjs";
        let stdio_path = "crates/codestory-cli/src/stdio_transport.rs";
        let runtime_path = "crates/codestory-runtime/src/agent/orchestrator.rs";
        let mut packet = test_packet(question, 96 * 1024);
        packet.packet_id = "ask-1784577982067658000".to_string();
        packet.answer.answer_id = packet.packet_id.clone();
        packet.task_class = Some(PacketTaskClassDto::ArchitectureExplanation);
        packet.plan.task_class = PacketTaskClassDto::ArchitectureExplanation;
        packet.plan.probe_resolutions = vec![
            PacketProbeResolutionDto {
                input_index: 0,
                probe: PacketProbeDto::ExactPath {
                    path: launcher_path.to_string(),
                },
                status: PacketProbeResolutionStatusDto::Rejected,
                normalized_query: None,
                path: Some(launcher_path.to_string()),
                symbol_id: None,
                candidates: Vec::new(),
                rejection: Some(PacketProbeRejectionDto {
                    code: PacketProbeRejectionCodeDto::MissingTarget,
                    message: "exact-path target does not exist".to_string(),
                }),
            },
            retained_exact_path_resolution(1, stdio_path),
            retained_exact_path_resolution(2, runtime_path),
        ];
        packet.answer.citations = vec![
            retained_graph_citation(
                "stdio_response_retrieval_publication",
                project_root.join(stdio_path).to_string_lossy().as_ref(),
            ),
            retained_graph_citation(
                "PacketEvidenceRole::TransportAdapter",
                project_root
                    .join("crates/codestory-runtime/src/agent/packet_evidence_roles.rs")
                    .to_string_lossy()
                    .as_ref(),
            ),
            retained_graph_citation(
                "transport_adapter_claim",
                project_root
                    .join("crates/codestory-runtime/src/agent/packet_claim_profiles.rs")
                    .to_string_lossy()
                    .as_ref(),
            ),
            retained_graph_citation(
                "ResolutionPhaseTelemetry::record_semantic_request_stats",
                project_root
                    .join("crates/codestory-indexer/src/resolution/mod.rs")
                    .to_string_lossy()
                    .as_ref(),
            ),
        ];
        packet.sufficiency.status = PacketSufficiencyStatusDto::Sufficient;
        packet.sufficiency.gaps.clear();
        packet.sufficiency.follow_up_commands.clear();

        enforce_packet_output_budget(project_root, &mut packet);

        assert_eq!(
            packet.sufficiency.status,
            PacketSufficiencyStatusDto::Partial,
            "the final post-budget packet must fail closed when the retained answer has no proof-bearing claim from the requested runtime path"
        );
        assert!(
            packet
                .sufficiency
                .gaps
                .iter()
                .any(|gap| gap.contains(runtime_path)),
            "the final packet should identify the uncovered requested path: {:?}",
            packet.sufficiency
        );
        assert!(
            packet
                .sufficiency
                .follow_up_commands
                .iter()
                .any(|command| command.contains(runtime_path)),
            "the final packet should provide a targeted follow-up for the uncovered path: {:?}",
            packet.sufficiency
        );
    }

    #[test]
    fn post_budget_claim_markdown_tracks_the_final_obligation_status_without_growing() {
        let question = "Explain RuntimeCoordinator::run.";
        let mut packet = test_packet(question, 96 * 1024);
        packet.task_class = Some(PacketTaskClassDto::ArchitectureExplanation);
        packet.plan.task_class = PacketTaskClassDto::ArchitectureExplanation;
        packet.answer.citations = vec![retained_graph_citation(
            "RuntimeCoordinator::run",
            "crates/core/src/runtime.rs",
        )];
        let edge_id = EdgeId("runtime-coordinator-call".to_string());
        packet.answer.citations[0].evidence_edge_ids = vec![edge_id.clone()];
        packet.answer.graphs = vec![GraphArtifactDto::Uml {
            id: "runtime-call".to_string(),
            title: "Runtime call".to_string(),
            graph: GraphResponse {
                center_id: NodeId("RuntimeCoordinator::run".to_string()),
                nodes: vec![
                    GraphNodeDto {
                        id: NodeId("RuntimeCoordinator::run".to_string()),
                        label: "RuntimeCoordinator::run".to_string(),
                        kind: NodeKind::FUNCTION,
                        depth: 1,
                        label_policy: None,
                        badge_visible_members: None,
                        badge_total_members: None,
                        merged_symbol_examples: Vec::new(),
                        file_path: Some("crates/core/src/runtime.rs".to_string()),
                        qualified_name: Some("RuntimeCoordinator::run".to_string()),
                        member_access: None,
                    },
                    GraphNodeDto {
                        id: NodeId("RuntimeService::finish".to_string()),
                        label: "RuntimeService::finish".to_string(),
                        kind: NodeKind::FUNCTION,
                        depth: 1,
                        label_policy: None,
                        badge_visible_members: None,
                        badge_total_members: None,
                        merged_symbol_examples: Vec::new(),
                        file_path: Some("crates/core/src/runtime_service.rs".to_string()),
                        qualified_name: Some("RuntimeService::finish".to_string()),
                        member_access: None,
                    },
                ],
                edges: vec![GraphEdgeDto {
                    id: edge_id,
                    source: NodeId("RuntimeCoordinator::run".to_string()),
                    target: NodeId("RuntimeService::finish".to_string()),
                    kind: EdgeKind::CALL,
                    confidence: Some(1.0),
                    certainty: Some("certain".to_string()),
                    callsite_identity: None,
                    candidate_targets: Vec::new(),
                }],
                truncated: false,
                omitted_edge_count: 0,
                canonical_layout: None,
            },
        }];
        packet.plan.obligations = build_packet_obligation_plan(
            question,
            PacketTaskClassDto::ArchitectureExplanation,
            &[],
        );
        packet
            .answer
            .retrieval_trace
            .packet_sidecar_diagnostics
            .extend(
                packet
                    .plan
                    .obligations
                    .query_obligations
                    .iter()
                    .filter(|query| query.material)
                    .map(|query| PacketSidecarQueryDiagnosticDto {
                        query: query.query.clone(),
                        completion: PacketQueryCompletionDto::Completed,
                        retrieval_mode: "full".to_string(),
                        sidecar_query_ms: Some(1),
                        candidate_resolution_ms: Some(0),
                        total_elapsed_ms: Some(1),
                        sidecar_stage_count: 1,
                        sidecar_stage_total_ms: Some(1),
                        batch_query_wall_ms: Some(1),
                        candidate_count: 1,
                        resolved_hit_count: 1,
                        unresolved_candidate_count: 0,
                        blocking_unresolved_candidate_count: 0,
                        semantic_stage_timeout_zero_hits: false,
                        semantic_abstained: false,
                        diagnostic: None,
                    }),
            );
        finalize_packet_obligation_plan(
            question,
            PacketTaskClassDto::ArchitectureExplanation,
            &mut packet.plan.obligations,
            &packet.answer,
            &packet.budget,
        );
        let supported_claims_with_telemetry =
            packet_supported_claims_with_telemetry(&packet.answer);
        let mut initial_claims = packet_claims_with_obligation_receipts(
            &packet.answer,
            &packet.plan.obligations,
            supported_claims_with_telemetry,
        );
        bind_claims_to_packet_obligations(&packet.plan.obligations, &mut initial_claims);
        let full_initial_markdown = packet_flow_claims_markdown(&initial_claims);
        let proven_marker_end = full_initial_markdown
            .find("[`P`]")
            .map(|offset| offset + "[`P`]".len())
            .expect("finalized CALL receipt should render as proven");
        let initial_markdown = format!(
            "{}{}",
            &full_initial_markdown[..proven_marker_end],
            PACKET_MARKDOWN_TRUNCATION_SUFFIX
        );
        assert!(initial_markdown.contains("[`P`]"), "{initial_markdown}");
        packet.answer.sections.push(AgentResponseSectionDto {
            id: "packet-flow-claims".to_string(),
            title: "Packet Claims".to_string(),
            blocks: vec![AgentResponseBlockDto::Markdown {
                markdown: initial_markdown.clone(),
            }],
        });
        for obligation in &mut packet.plan.obligations.claim_obligations {
            obligation.carrier_edge_proofs.clear();
        }
        packet.answer.graphs.clear();
        packet.budget.truncated = true;
        packet.budget.omitted_sections = vec!["trail_edges".to_string()];
        finalize_packet_obligation_plan(
            question,
            PacketTaskClassDto::ArchitectureExplanation,
            &mut packet.plan.obligations,
            &packet.answer,
            &packet.budget,
        );

        refresh_packet_claim_markdown(&mut packet);

        let refreshed = packet
            .answer
            .sections
            .iter()
            .find(|section| section.id == "packet-flow-claims")
            .and_then(|section| section.blocks.first())
            .and_then(|block| match block {
                AgentResponseBlockDto::Markdown { markdown } => Some(markdown),
                AgentResponseBlockDto::Mermaid { .. } => None,
            })
            .expect("packet claim markdown");
        assert!(refreshed.contains("[`R`]"), "{refreshed}");
        assert!(!refreshed.contains("[`P`]"), "{refreshed}");
        assert_eq!(refreshed.len(), initial_markdown.len());
    }

    #[test]
    fn compact_budget_trims_summary_trace_before_hard_payload_omission() {
        let question = "Explain duplicated packet trace diagnostics.";
        let mut packet = test_packet(question, 1);
        install_duplicate_summary_trace_payload(&mut packet, 180);

        let mut trimmed_probe = packet.clone();
        assert!(trim_packet_retrieval_trace_summary(&mut trimmed_probe));
        push_omitted_section(&mut trimmed_probe.budget, RETRIEVAL_TRACE_SUMMARY_OMISSION);
        let trimmed_len = serialized_packet_len(&trimmed_probe);
        let max_output_bytes = u32::try_from(trimmed_len + 4096).expect("test cap fits u32");
        packet.budget.limits.max_output_bytes = max_output_bytes;
        assert!(
            serialized_packet_len(&packet) > max_output_bytes as usize,
            "fixture must start over the packet output cap"
        );

        enforce_packet_output_budget(test_project_root(), &mut packet);

        let serialized_len = serialized_packet_len(&packet);
        assert!(
            serialized_len <= max_output_bytes as usize,
            "trimming summary trace should bring the packet under cap: {serialized_len} > {max_output_bytes}"
        );
        assert_eq!(packet.budget.used.output_bytes as usize, serialized_len);
        assert!(
            packet
                .budget
                .omitted_sections
                .contains(&RETRIEVAL_TRACE_SUMMARY_OMISSION.to_string())
        );
        assert!(
            !packet
                .budget
                .omitted_sections
                .contains(&"output_bytes".to_string())
        );
        assert!(
            !packet
                .budget
                .omitted_sections
                .contains(&"packet_payload".to_string())
        );
        assert_eq!(packet.retrieval_trace_summary.search_steps, 1);
        assert_eq!(packet.retrieval_trace_summary.trail_steps, 1);
        assert_eq!(packet.retrieval_trace_summary.source_read_steps, 1);
        assert!(
            packet
                .retrieval_trace_summary
                .retrieval_trace
                .request_id
                .is_empty()
        );
        assert!(
            packet
                .retrieval_trace_summary
                .retrieval_trace
                .steps
                .is_empty()
        );
        assert_eq!(packet.answer.retrieval_trace.steps.len(), 3);
        assert!(
            packet
                .answer
                .retrieval_trace
                .annotations
                .iter()
                .any(|annotation| annotation.text.contains("canonical trace annotation"))
        );
    }

    #[test]
    fn compact_budget_trims_answer_trace_diagnostics_before_hard_payload_omission() {
        let question = "Explain packet retrieval diagnostics under the wire cap.";
        let mut packet = test_packet(question, 1);
        install_duplicate_summary_trace_payload(&mut packet, 180);
        packet.answer.retrieval_trace.annotations.push(
            codestory_contracts::api::RetrievalAnnotationDto::gap(
                "material retrieval gap must survive payload trimming",
            ),
        );
        packet.answer.retrieval_trace.annotations.push(
            codestory_contracts::api::RetrievalAnnotationDto::observation(
                "packet_step_trace search_total_ms=10 step_count=3",
            ),
        );
        packet.retrieval_trace_summary = packet_retrieval_trace_summary(&packet.answer);

        let mut trimmed_probe = packet.clone();
        assert!(trim_packet_retrieval_trace_summary(&mut trimmed_probe));
        assert!(trim_packet_answer_retrieval_diagnostics(&mut trimmed_probe));
        let trimmed_len = serialized_packet_len(&trimmed_probe);
        let max_output_bytes = u32::try_from(trimmed_len + 4096).expect("test cap fits u32");
        packet.budget.limits.max_output_bytes = max_output_bytes;
        assert!(
            serialized_packet_len(&packet) > max_output_bytes as usize,
            "fixture must start over the packet output cap"
        );

        enforce_packet_output_budget(test_project_root(), &mut packet);

        let serialized_len = serialized_packet_len(&packet);
        assert!(
            serialized_len <= max_output_bytes as usize,
            "trimming answer trace diagnostics should bring the packet under cap: {serialized_len} > {max_output_bytes}"
        );
        assert_eq!(packet.budget.used.output_bytes as usize, serialized_len);
        assert!(
            packet
                .budget
                .omitted_sections
                .contains(&ANSWER_RETRIEVAL_DIAGNOSTICS_OMISSION.to_string())
        );
        assert!(packet.answer.retrieval_trace.steps.is_empty());
        assert_eq!(packet.answer.retrieval_trace.annotations.len(), 2);
        assert!(
            packet
                .answer
                .retrieval_trace
                .annotations
                .iter()
                .any(|annotation| annotation.is_gap())
        );
        assert!(
            packet
                .answer
                .retrieval_trace
                .annotations
                .iter()
                .any(|annotation| annotation
                    .text
                    .starts_with(PACKET_STEP_TRACE_ANNOTATION_PREFIX))
        );
        assert!(
            !packet
                .answer
                .retrieval_trace
                .annotations
                .iter()
                .any(|annotation| annotation.text.contains("canonical trace annotation"))
        );
        assert_eq!(
            packet
                .retrieval_trace_summary
                .retrieval_trace
                .total_latency_ms,
            123
        );
        assert_eq!(
            packet.retrieval_trace_summary.retrieval_trace.sla_target_ms,
            Some(1_000)
        );
        assert!(packet.retrieval_trace_summary.retrieval_trace.sla_missed);
    }

    #[test]
    fn compact_budget_retains_step_rows_for_json_and_file_export() {
        let question = "Explain packet step trace export under the wire cap.";
        let mut packet = test_packet(question, 1);
        install_duplicate_summary_trace_payload(&mut packet, 180);
        packet.answer.retrieval_trace.annotations.push(
            codestory_contracts::api::RetrievalAnnotationDto::observation(
                "packet_step_trace search_total_ms=10 step_count=3",
            ),
        );
        packet.answer.retrieval_trace.annotations.push(
            codestory_contracts::api::RetrievalAnnotationDto::gap(
                "packet_step_trace typed gap must survive compaction",
            ),
        );
        packet.retrieval_trace_summary = packet_retrieval_trace_summary(&packet.answer);

        let mut trimmed_probe = packet.clone();
        assert!(trim_packet_retrieval_trace_summary(&mut trimmed_probe));
        assert!(trim_packet_answer_retrieval_diagnostics(&mut trimmed_probe));
        let max_output_bytes = u32::try_from(serialized_packet_len(&trimmed_probe) + 4_096)
            .expect("test cap fits u32");
        packet.budget.limits.max_output_bytes = max_output_bytes;
        assert!(
            serialized_packet_len(&packet) > max_output_bytes as usize,
            "fixture must start over the packet output cap"
        );

        enforce_packet_output_budget(test_project_root(), &mut packet);

        assert!(packet.answer.retrieval_trace.steps.is_empty());
        assert!(serialized_packet_len(&packet) <= max_output_bytes as usize);
        let json = packet_step_trace_json(&packet.answer);
        assert_eq!(json["step_count"], 3);
        assert_eq!(json["retained_step_trace"]["source_step_count"], 3);
        assert_eq!(json["retained_step_trace"]["rows_truncated"], false);
        assert_eq!(json["steps"][0]["kind"], "Search");
        assert_eq!(json["steps"][0]["duration_ms"], 10);
        assert_eq!(json["steps"][1]["kind"], "Trail");
        assert_eq!(json["steps"][2]["kind"], "SourceRead");
        assert!(
            packet
                .answer
                .retrieval_trace
                .annotations
                .iter()
                .any(|annotation| annotation.is_gap()
                    && annotation.text.starts_with("packet_step_trace typed gap"))
        );

        packet
            .answer
            .retrieval_trace
            .steps
            .push(AgentRetrievalStepDto {
                kind: AgentRetrievalStepKindDto::AnswerSynthesis,
                status: AgentRetrievalStepStatusDto::Ok,
                duration_ms: 7,
                input: Vec::new(),
                output: Vec::new(),
                message: Some("post-budget phase ".repeat(1_000)),
            });
        packet.retrieval_trace_summary = packet_retrieval_trace_summary(&packet.answer);
        assert!(serialized_packet_len(&packet) > max_output_bytes as usize);

        enforce_packet_output_budget(test_project_root(), &mut packet);

        assert!(packet.answer.retrieval_trace.steps.is_empty());
        assert!(serialized_packet_len(&packet) <= max_output_bytes as usize);
        let json = packet_step_trace_json(&packet.answer);
        assert_eq!(json["step_count"], 4);
        assert_eq!(json["retained_step_trace"]["source_step_count"], 4);
        assert_eq!(json["steps"][3]["step_index"], 3);
        assert_eq!(json["steps"][3]["kind"], "AnswerSynthesis");
        assert_eq!(json["steps"][3]["duration_ms"], 7);

        let _lock = crate::process_env_test_lock();
        let trace_path = std::env::temp_dir().join(format!(
            "codestory-over-cap-packet-step-trace-{}.json",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&trace_path);
        // SAFETY: this test holds the process env lock and restores the variable below.
        unsafe {
            std::env::set_var("CODESTORY_PACKET_STEP_TRACE_OUT", &trace_path);
        }
        let diagnostic = write_packet_step_trace_from_env(&packet.answer);
        // SAFETY: this test holds the process env lock.
        unsafe {
            std::env::remove_var("CODESTORY_PACKET_STEP_TRACE_OUT");
        }
        assert_eq!(diagnostic, None);
        let exported: serde_json::Value = serde_json::from_slice(
            &std::fs::read(&trace_path).expect("read exported packet step trace"),
        )
        .expect("parse exported packet step trace");
        let _ = std::fs::remove_file(&trace_path);
        assert_eq!(exported["step_count"], 4);
        assert_eq!(exported["steps"][0]["kind"], "Search");
        assert_eq!(exported["steps"][2]["duration_ms"], 30);
        assert_eq!(exported["steps"][3]["kind"], "AnswerSynthesis");
    }

    #[test]
    fn compact_step_trace_proof_reports_bounded_row_loss() {
        let mut packet = test_packet("Explain bounded packet step trace retention.", u32::MAX);
        packet.answer.retrieval_trace.steps = (0..70)
            .map(|_| AgentRetrievalStepDto {
                kind: AgentRetrievalStepKindDto::Search,
                status: AgentRetrievalStepStatusDto::Ok,
                duration_ms: 1,
                input: Vec::new(),
                output: Vec::new(),
                message: None,
            })
            .collect();
        packet.answer.retrieval_trace.annotations.push(
            codestory_contracts::api::RetrievalAnnotationDto::observation(
                "packet_step_trace search_total_ms=70 step_count=70",
            ),
        );

        assert!(trim_packet_answer_retrieval_diagnostics(&mut packet));

        let json = packet_step_trace_json(&packet.answer);
        assert_eq!(json["step_count"], 64);
        assert_eq!(json["retained_step_trace"]["source_step_count"], 70);
        assert_eq!(json["retained_step_trace"]["retained_step_count"], 64);
        assert_eq!(json["retained_step_trace"]["rows_truncated"], true);
    }

    #[test]
    fn compact_budget_refreshes_usage_after_answer_trace_trimming() {
        let question = "Explain packet usage accounting after diagnostic trimming.";
        let mut packet = test_packet(question, 1);
        packet.answer.retrieval_trace.steps = vec![AgentRetrievalStepDto {
            kind: AgentRetrievalStepKindDto::SourceRead,
            status: AgentRetrievalStepStatusDto::Ok,
            duration_ms: 30,
            input: Vec::new(),
            output: Vec::new(),
            message: Some("duplicated source-read diagnostic ".repeat(700)),
        }];
        install_verbose_semantic_stage_shadow(&mut packet);
        packet.retrieval_trace_summary = packet_retrieval_trace_summary(&packet.answer);
        packet.budget.used = packet_budget_usage(&packet.answer);
        assert_eq!(packet.budget.used.snippets, 1);
        assert_eq!(packet.retrieval_trace_summary.source_read_steps, 1);

        let represented_len = |packet: &AgentPacketDto| serialized_packet_len(packet) + 2_048;
        let mut trimmed_probe = packet.clone();
        assert!(trim_packet_retrieval_trace_summary(&mut trimmed_probe));
        assert!(trim_packet_answer_retrieval_diagnostics(&mut trimmed_probe));
        let max_output_bytes =
            u32::try_from(represented_len(&trimmed_probe) + 4_096).expect("test cap fits u32");
        packet.budget.limits.max_output_bytes = max_output_bytes;
        assert!(
            represented_len(&packet) > max_output_bytes as usize,
            "fixture must start over the represented output cap"
        );

        enforce_packet_output_budget_for_representation(
            test_project_root(),
            &mut packet,
            represented_len,
        );

        let retained_source_reads = packet
            .answer
            .retrieval_trace
            .steps
            .iter()
            .filter(|step| {
                step.kind == AgentRetrievalStepKindDto::SourceRead
                    && step.status == AgentRetrievalStepStatusDto::Ok
            })
            .count() as u32;
        assert_eq!(retained_source_reads, 0);
        assert_eq!(
            packet.retrieval_trace_summary.source_read_steps,
            retained_source_reads
        );
        assert_eq!(packet.budget.used.snippets, retained_source_reads);

        let answer_usage = packet_budget_usage(&packet.answer);
        assert_eq!(packet.budget.used.anchors, answer_usage.anchors);
        assert_eq!(packet.budget.used.files, answer_usage.files);
        assert_eq!(packet.budget.used.snippets, answer_usage.snippets);
        assert_eq!(packet.budget.used.trail_edges, answer_usage.trail_edges);
        assert_eq!(
            packet.budget.used.output_bytes as usize,
            represented_len(&packet)
        );
        assert!(
            packet
                .budget
                .omitted_sections
                .contains(&ANSWER_RETRIEVAL_DIAGNOSTICS_OMISSION.to_string())
        );

        for trace in [
            &packet.answer.retrieval_trace,
            &packet.retrieval_trace_summary.retrieval_trace,
        ] {
            let semantic_stages = &trace
                .retrieval_shadow
                .as_ref()
                .expect("retrieval shadow proof")
                .stage_timings;
            assert_eq!(semantic_stages.len(), 2);
            assert_eq!(semantic_stages[0].completion_status, "completed");
            assert_eq!(
                semantic_stages[1].cancel_reason.as_deref(),
                Some("stage_deadline")
            );
            assert!(semantic_stages[1].degraded);
        }
    }

    #[test]
    fn compact_budget_preserves_semantic_execution_proof_under_adapter_cap() {
        let question = "Explain semantic retrieval proof under the public adapter cap.";
        let mut packet = test_packet(question, 1);
        install_duplicate_summary_trace_payload(&mut packet, 180);
        install_verbose_semantic_stage_shadow(&mut packet);

        let represented_len = |packet: &AgentPacketDto| serialized_packet_len(packet) + 2_048;
        let mut trimmed_probe = packet.clone();
        assert!(trim_packet_retrieval_trace_summary(&mut trimmed_probe));
        assert!(trim_packet_answer_retrieval_diagnostics(&mut trimmed_probe));
        let max_output_bytes =
            u32::try_from(represented_len(&trimmed_probe) + 4_096).expect("test cap fits u32");
        packet.budget.limits.max_output_bytes = max_output_bytes;
        assert!(
            represented_len(&packet) > max_output_bytes as usize,
            "fixture must start over the represented output cap"
        );

        enforce_packet_output_budget_for_representation(
            test_project_root(),
            &mut packet,
            represented_len,
        );

        assert!(represented_len(&packet) <= max_output_bytes as usize);
        assert_eq!(
            packet.budget.used.output_bytes as usize,
            represented_len(&packet)
        );
        assert!(
            packet
                .budget
                .omitted_sections
                .contains(&RETRIEVAL_TRACE_SUMMARY_OMISSION.to_string())
        );
        assert!(
            packet
                .budget
                .omitted_sections
                .contains(&ANSWER_RETRIEVAL_DIAGNOSTICS_OMISSION.to_string())
        );

        for trace in [
            &packet.answer.retrieval_trace,
            &packet.retrieval_trace_summary.retrieval_trace,
        ] {
            assert_eq!(trace.total_latency_ms, 123);
            assert_eq!(trace.sla_target_ms, Some(1_000));
            assert!(trace.sla_missed);
            let semantic_stages = trace
                .retrieval_shadow
                .as_ref()
                .expect("retrieval shadow proof")
                .stage_timings
                .iter()
                .filter(|stage| stage.stage.contains("semantic"))
                .collect::<Vec<_>>();
            assert_eq!(semantic_stages.len(), 2);
            assert!(
                semantic_stages
                    .iter()
                    .any(|stage| stage.completion_status == "completed")
            );
            assert!(
                semantic_stages
                    .iter()
                    .any(|stage| stage.cancel_reason.as_deref() == Some("stage_deadline"))
            );
            assert!(semantic_stages.iter().any(|stage| stage.degraded));
            assert!(
                semantic_stages
                    .iter()
                    .any(|stage| stage.stub_reason.as_deref() == Some("semantic_runtime_degraded"))
            );
        }
    }

    #[test]
    fn compact_budget_keeps_hard_payload_omission_when_diagnostic_trimming_is_not_enough() {
        let question = "Explain still oversized packet diagnostics.";
        let mut packet = test_packet(question, 512);
        install_duplicate_summary_trace_payload(&mut packet, 24);

        enforce_packet_output_budget(test_project_root(), &mut packet);

        let serialized_len = serialized_packet_len(&packet);
        assert!(
            serialized_len > packet.budget.limits.max_output_bytes as usize,
            "fixture should remain over an impossible cap after diagnostic trimming"
        );
        assert_eq!(packet.budget.used.output_bytes as usize, serialized_len);
        assert!(
            packet
                .budget
                .omitted_sections
                .contains(&RETRIEVAL_TRACE_SUMMARY_OMISSION.to_string())
        );
        assert!(
            packet
                .budget
                .omitted_sections
                .contains(&"output_bytes".to_string())
        );
        assert!(
            packet
                .budget
                .omitted_sections
                .contains(&"packet_payload".to_string())
        );
        assert!(packet.budget.truncated);
        assert_eq!(packet.retrieval_trace_summary.search_steps, 0);
        assert_eq!(packet.retrieval_trace_summary.trail_steps, 0);
        assert_eq!(packet.retrieval_trace_summary.source_read_steps, 0);
        assert!(
            packet
                .retrieval_trace_summary
                .retrieval_trace
                .steps
                .is_empty()
        );
        assert!(packet.answer.retrieval_trace.steps.is_empty());
        assert!(
            packet
                .budget
                .omitted_sections
                .contains(&ANSWER_RETRIEVAL_DIAGNOSTICS_OMISSION.to_string())
        );
    }

    #[test]
    fn compact_budget_trims_ineligible_coverage_report_before_payload_omission() {
        let question = "Explain symbol ownership for PacketBudget.";
        let mut packet = test_packet(question, 1);
        packet
            .sufficiency
            .coverage_report
            .as_mut()
            .expect("coverage report")
            .ineligible = (0..48)
            .map(|index| {
                format!(
                    "claim=\"diagnostic claim {index} {}\" role=\"source evidence\" tier=\"diagnostic\" reason=\"claim marked diagnostic\"",
                    "padding ".repeat(80)
                )
            })
            .collect();

        let mut trimmed_probe = packet.clone();
        let trimmed_sections = trim_packet_sufficiency_verbose_lists(&mut trimmed_probe);
        assert_eq!(trimmed_sections, vec![COVERAGE_REPORT_INELIGIBLE_OMISSION]);
        let trimmed_len = serialized_packet_len(&trimmed_probe);
        let max_output_bytes = u32::try_from(trimmed_len + 4096).expect("test cap fits u32");
        packet.budget.limits.max_output_bytes = max_output_bytes;
        assert!(
            serialized_packet_len(&packet) > max_output_bytes as usize,
            "fixture must start over the packet output cap"
        );

        enforce_packet_output_budget(test_project_root(), &mut packet);

        let serialized_len = serialized_packet_len(&packet);
        assert!(
            serialized_len <= max_output_bytes as usize,
            "trimming verbose ineligible diagnostics should bring the packet under cap: {serialized_len} > {max_output_bytes}"
        );
        assert_eq!(packet.budget.used.output_bytes as usize, serialized_len);
        assert!(
            !packet
                .budget
                .omitted_sections
                .contains(&"output_bytes".to_string())
        );
        assert!(
            !packet
                .budget
                .omitted_sections
                .contains(&"packet_payload".to_string())
        );
        assert!(
            packet
                .budget
                .omitted_sections
                .contains(&COVERAGE_REPORT_INELIGIBLE_OMISSION.to_string())
        );
        assert!(
            packet
                .sufficiency
                .coverage_report
                .as_ref()
                .expect("coverage report")
                .ineligible
                .is_empty()
        );
    }

    #[test]
    fn output_budget_converges_across_all_available_structural_trims() {
        let max_output_bytes = 64 * 1024;
        let mut packet = test_packet(
            "Explain packet output convergence after many independent markdown blocks.",
            max_output_bytes,
        );
        packet.answer.sections.push(AgentResponseSectionDto {
            id: "many-diagnostics".to_string(),
            title: "Many diagnostics".to_string(),
            blocks: (0..96)
                .map(|index| AgentResponseBlockDto::Markdown {
                    markdown: format!("diagnostic block {index} {}", "padding ".repeat(256)),
                })
                .collect(),
        });
        assert!(
            serialized_packet_len(&packet) > max_output_bytes as usize,
            "fixture must start over the output cap"
        );

        enforce_packet_output_budget(test_project_root(), &mut packet);

        let serialized_len = serialized_packet_len(&packet);
        assert!(
            serialized_len <= max_output_bytes as usize,
            "every available structural trim must participate in convergence: {serialized_len} > {max_output_bytes}"
        );
        assert_eq!(packet.budget.used.output_bytes as usize, serialized_len);
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
                .contains(&"output_bytes".to_string())
        );
        assert!(
            !packet
                .budget
                .omitted_sections
                .contains(&"packet_payload".to_string())
        );
    }

    #[test]
    fn adapter_budget_compacts_retained_step_proof_after_other_trims_are_exhausted() {
        const PUBLIC_CAP: usize = 98_304;
        const RETAINED_F22_SHAPE: usize = 98_467;
        let mut packet = test_packet(
            "Explain the retained packet trace after all ordinary compact trims are exhausted.",
            PUBLIC_CAP as u32,
        );
        packet.answer.retrieval_trace.steps = (0..26)
            .map(|index| AgentRetrievalStepDto {
                kind: if index % 3 == 0 {
                    AgentRetrievalStepKindDto::Search
                } else {
                    AgentRetrievalStepKindDto::SourceRead
                },
                status: AgentRetrievalStepStatusDto::Ok,
                duration_ms: 5 + index,
                input: vec![codestory_contracts::api::AgentRetrievalSummaryFieldDto {
                    key: "query".to_string(),
                    value: format!("packet-query-{index}-{}", "q".repeat(128)),
                }],
                output: vec![
                    codestory_contracts::api::AgentRetrievalSummaryFieldDto {
                        key: "hits".to_string(),
                        value: "8".to_string(),
                    },
                    codestory_contracts::api::AgentRetrievalSummaryFieldDto {
                        key: "mode".to_string(),
                        value: "packet_fused_batch".to_string(),
                    },
                    codestory_contracts::api::AgentRetrievalSummaryFieldDto {
                        key: "sidecar_query_ms".to_string(),
                        value: "7".to_string(),
                    },
                    codestory_contracts::api::AgentRetrievalSummaryFieldDto {
                        key: "candidate_resolution_ms".to_string(),
                        value: "11".to_string(),
                    },
                ],
                message: Some(format!("packet-step-{index}-{}", "diagnostic".repeat(14))),
            })
            .collect();
        packet.answer.retrieval_trace.annotations.push(
            codestory_contracts::api::RetrievalAnnotationDto::gap(
                "typed retrieval gap must survive retained proof compaction",
            ),
        );
        install_verbose_semantic_stage_shadow(&mut packet);
        packet.retrieval_trace_summary = packet_retrieval_trace_summary(&packet.answer);

        let mut exhausted = packet.clone();
        push_omitted_section(&mut exhausted.budget, "output_bytes");
        push_omitted_section(&mut exhausted.budget, "packet_payload");
        let extra_probes = packet_explicit_request_probe_queries(&exhausted.plan);
        loop {
            let trimmed_verbose_sections = trim_packet_sufficiency_verbose_lists(&mut exhausted);
            let structurally_trimmed = if !trimmed_verbose_sections.is_empty() {
                for section in trimmed_verbose_sections {
                    push_omitted_section(&mut exhausted.budget, section);
                }
                true
            } else if trim_packet_retrieval_trace_summary(&mut exhausted) {
                push_omitted_section(&mut exhausted.budget, RETRIEVAL_TRACE_SUMMARY_OMISSION);
                true
            } else if trim_packet_answer_retrieval_diagnostics(&mut exhausted) {
                push_omitted_section(&mut exhausted.budget, ANSWER_RETRIEVAL_DIAGNOSTICS_OMISSION);
                true
            } else {
                false
            };
            if !structurally_trimmed {
                break;
            }
            refresh_packet_after_budget_mutation(
                test_project_root(),
                &mut exhausted,
                &extra_probes,
                &serialized_packet_len,
            );
        }
        assert!(
            exhausted
                .answer
                .sections
                .iter()
                .all(|section| section.blocks.iter().all(|block| match block {
                    AgentResponseBlockDto::Markdown { markdown } =>
                        markdown.len() < MARKDOWN_TRUNCATION_FLOOR_BYTES,
                    AgentResponseBlockDto::Mermaid { .. } => true,
                }))
        );
        assert!(!truncate_answer_markdown_to_byte_cap(
            &mut exhausted.answer,
            1
        ));
        let exhausted_len = serialized_packet_len(&exhausted);
        assert!(
            exhausted_len < RETAINED_F22_SHAPE,
            "adapter envelope must be positive"
        );
        let adapter_envelope = RETAINED_F22_SHAPE - exhausted_len;
        let represented_len = |packet: &AgentPacketDto| {
            serialized_packet_len(packet).saturating_add(adapter_envelope)
        };
        assert_eq!(represented_len(&exhausted), RETAINED_F22_SHAPE);

        enforce_packet_output_budget_for_representation(
            test_project_root(),
            &mut packet,
            represented_len,
        );

        let final_len = represented_len(&packet);
        assert!(
            final_len <= PUBLIC_CAP,
            "fully rebuilt adapter packet must satisfy its cap: {final_len} > {PUBLIC_CAP}"
        );
        assert_eq!(packet.budget.used.output_bytes as usize, final_len);
        assert!(
            packet
                .budget
                .omitted_sections
                .contains(&RETAINED_STEP_TRACE_DETAIL_OMISSION.to_string())
        );
        assert!(
            !packet
                .budget
                .omitted_sections
                .iter()
                .any(|section| section == "output_bytes" || section == "packet_payload")
        );
        assert!(
            packet
                .answer
                .retrieval_trace
                .annotations
                .iter()
                .any(|annotation| annotation.is_gap()
                    && annotation.text.contains("typed retrieval gap"))
        );

        let exported = packet_step_trace_json(&packet.answer);
        let retained_count = exported["retained_step_trace"]["retained_step_count"]
            .as_u64()
            .expect("retained count") as usize;
        let source_count = exported["retained_step_trace"]["source_step_count"]
            .as_u64()
            .expect("source count") as usize;
        assert_eq!(source_count, 26);
        assert!(retained_count > 0 && retained_count <= source_count);
        assert_eq!(exported["steps"][0]["step_index"], 0);
        assert!(exported["steps"][0]["kind"].is_string());
        assert_eq!(exported["retained_step_trace"]["fields_truncated"], true);
        assert_eq!(
            exported["retained_step_trace"]["rows_truncated"],
            retained_count < source_count
        );

        for trace in [
            &packet.answer.retrieval_trace,
            &packet.retrieval_trace_summary.retrieval_trace,
        ] {
            let stages = &trace
                .retrieval_shadow
                .as_ref()
                .expect("semantic stage proof")
                .stage_timings;
            assert_eq!(stages.len(), 2);
            assert_eq!(stages[0].completion_status, "completed");
            assert_eq!(stages[1].completion_status, "cancelled_before_start");
            assert_eq!(stages[1].cancel_reason.as_deref(), Some("stage_deadline"));
            assert!(stages[1].degraded);
        }
    }

    #[test]
    fn adapter_budget_keeps_hard_receipt_when_marker_free_shape_exceeds_cap() {
        let max_output_bytes = 32 * 1024;
        let mut packet = test_packet(
            "Explain a representation whose marker-free dependent shape is larger.",
            max_output_bytes,
        );
        let original_sections =
            serde_json::to_value(&packet.answer.sections).expect("serialize original sections");
        let original_citations =
            serde_json::to_value(&packet.answer.citations).expect("serialize original citations");
        let represented_len = |packet: &AgentPacketDto| {
            let marker_free_penalty = if packet
                .budget
                .omitted_sections
                .iter()
                .any(|section| section == "packet_payload")
            {
                0
            } else {
                64 * 1024
            };
            serialized_packet_len(packet).saturating_add(marker_free_penalty)
        };
        assert!(represented_len(&packet) > max_output_bytes as usize);

        enforce_packet_output_budget_for_representation(
            test_project_root(),
            &mut packet,
            represented_len,
        );

        let marker_shape_len = represented_len(&packet);
        assert!(marker_shape_len <= max_output_bytes as usize);
        assert_eq!(packet.budget.used.output_bytes as usize, marker_shape_len);
        assert!(packet.budget.truncated);
        assert!(
            packet
                .budget
                .omitted_sections
                .contains(&"output_bytes".to_string())
        );
        assert!(
            packet
                .budget
                .omitted_sections
                .contains(&"packet_payload".to_string())
        );
        assert_eq!(
            serde_json::to_value(&packet.answer.sections).expect("serialize retained sections"),
            original_sections
        );
        assert_eq!(
            serde_json::to_value(&packet.answer.citations).expect("serialize retained citations"),
            original_citations
        );

        let converged = serde_json::to_vec(&packet).expect("serialize converged marker shape");
        enforce_packet_output_budget_for_representation(
            test_project_root(),
            &mut packet,
            represented_len,
        );
        assert_eq!(
            serde_json::to_vec(&packet).expect("serialize repeated marker shape"),
            converged,
            "a second enforcement must retain the same measured marker-present fixpoint"
        );
    }

    #[test]
    fn representation_budget_does_not_change_default_compact_accounting() {
        let mut packet = test_packet("Explain packet output accounting.", u32::MAX);
        packet.answer.sections.push(AgentResponseSectionDto {
            id: "representation-padding".to_string(),
            title: "Representation padding".to_string(),
            blocks: (0..8)
                .map(|index| AgentResponseBlockDto::Markdown {
                    markdown: format!("diagnostic {index} {}", "padding ".repeat(128)),
                })
                .collect(),
        });

        enforce_packet_output_budget(test_project_root(), &mut packet);

        let compact_len = serde_json::to_vec(&packet)
            .expect("serialize compact packet")
            .len();
        let pretty_len = serde_json::to_vec_pretty(&packet)
            .expect("serialize pretty packet")
            .len()
            + 1;
        let public_cap = compact_len + ((pretty_len - compact_len) / 2);
        packet.budget.limits.max_output_bytes =
            u32::try_from(public_cap).expect("fixture cap fits u32");

        enforce_packet_output_budget(test_project_root(), &mut packet);

        let compact_len = serde_json::to_vec(&packet)
            .expect("serialize compact packet at bounded cap")
            .len();
        let pretty_len = serde_json::to_vec_pretty(&packet)
            .expect("serialize pretty packet at bounded cap")
            .len()
            + 1;
        assert!(
            compact_len <= public_cap,
            "default compact encoding should fit: {compact_len} > {public_cap}"
        );
        assert!(
            pretty_len > public_cap,
            "pretty encoding plus its newline must still exceed the cap"
        );
        assert_eq!(packet.budget.used.output_bytes as usize, compact_len);

        enforce_packet_output_budget_for_representation(
            test_project_root(),
            &mut packet,
            |packet| {
                serde_json::to_vec_pretty(packet)
                    .expect("serialize represented packet")
                    .len()
                    + 1
            },
        );

        let rendered_len = serde_json::to_vec_pretty(&packet)
            .expect("serialize budgeted represented packet")
            .len()
            + 1;
        assert!(rendered_len <= public_cap, "{rendered_len} > {public_cap}");
        assert_eq!(packet.budget.used.output_bytes as usize, rendered_len);
    }

    #[test]
    fn sufficiency_verbose_trimming_preserves_missing_and_blocking_report_entries() {
        let mut packet = test_packet("Explain route dispatch gaps.", 4096);
        packet.sufficiency.coverage_report = Some(PacketCoverageReportDto {
            covered: vec!["request dispatch".to_string()],
            provenance_labels: vec!["graph_neighbor".to_string()],
            provenance_counts: std::collections::BTreeMap::from([(
                "graph_neighbor".to_string(),
                1,
            )]),
            missing: vec!["route handling".to_string()],
            ineligible: vec!["claim=\"diagnostic\" reason=\"claim marked diagnostic\"".to_string()],
            unresolved: vec!["RouteDispatcher".to_string()],
            budget_omitted: vec!["packet_payload".to_string(), "output_bytes".to_string()],
        });

        let trimmed_sections = trim_packet_sufficiency_verbose_lists(&mut packet);

        assert_eq!(trimmed_sections, vec![COVERAGE_REPORT_INELIGIBLE_OMISSION]);
        let report = packet
            .sufficiency
            .coverage_report
            .as_ref()
            .expect("coverage report");
        assert_eq!(report.covered, vec!["request dispatch".to_string()]);
        assert_eq!(report.provenance_labels, vec!["graph_neighbor".to_string()]);
        assert_eq!(report.provenance_counts.get("graph_neighbor"), Some(&1));
        assert_eq!(report.missing, vec!["route handling".to_string()]);
        assert!(report.ineligible.is_empty());
        assert_eq!(report.unresolved, vec!["RouteDispatcher".to_string()]);
        assert_eq!(
            report.budget_omitted,
            vec!["packet_payload".to_string(), "output_bytes".to_string()]
        );
    }

    pub(in crate::agent) fn test_packet(question: &str, max_output_bytes: u32) -> AgentPacketDto {
        let answer = AgentAnswerDto {
            source_coverage: Vec::new(),
            answer_id: "packet-budget-test".to_string(),
            prompt: question.to_string(),
            summary: "Packet budget test answer.".to_string(),
            freshness: Some(crate::agent::packet_freshness::fresh_index_observation()),
            sections: vec![AgentResponseSectionDto {
                id: "answer".to_string(),
                title: "Answer".to_string(),
                blocks: vec![AgentResponseBlockDto::Markdown {
                    markdown: "Short answer with cited ownership evidence.".to_string(),
                }],
            }],
            citations: vec![
                test_citation(
                    "PacketBudget",
                    "crates/codestory-runtime/src/agent/packet_budget.rs",
                ),
                test_citation(
                    "AgentPacketDto",
                    "crates/codestory-contracts/src/api/dto.rs",
                ),
            ],
            subgraph_ids: Vec::new(),
            retrieval_version: "test".to_string(),
            graphs: Vec::new(),
            retrieval_trace: AgentRetrievalTraceDto {
                request_id: "packet-budget-test".to_string(),
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
        let budget = PacketBudgetDto {
            requested: PacketBudgetModeDto::Compact,
            limits: PacketBudgetLimitsDto {
                max_anchors: 13,
                max_files: 13,
                max_snippets: 12,
                max_trail_edges: 20,
                max_output_bytes,
            },
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
        let sufficiency = PacketSufficiencyDto {
            status: PacketSufficiencyStatusDto::Sufficient,
            covered_claims: vec![PacketClaimDto {
                claim: "Packet budget ownership is covered by cited runtime and contract anchors."
                    .to_string(),
                required_obligation_ids: Vec::new(),
                required_obligation_kinds: Vec::new(),
                proof_status: None,
                required_evidence_role: None,
                citations: answer.citations.clone(),
                coverage_role: Some("source evidence".to_string()),
                eligible_for_sufficiency: Some(true),
            }],
            open_next: Vec::new(),
            avoid_opening: Vec::new(),
            avoid_opening_paths: Vec::new(),
            gaps: Vec::new(),
            follow_up_commands: Vec::new(),
            follow_up_invocations: Vec::new(),
            coverage_report: Some(PacketCoverageReportDto::default()),
        };
        let retrieval_trace_summary = PacketRetrievalTraceSummaryDto {
            retrieval_trace: answer.retrieval_trace.clone(),
            source_read_steps: 0,
            search_steps: 0,
            trail_steps: 0,
        };

        AgentPacketDto {
            packet_id: answer.answer_id.clone(),
            question: question.to_string(),
            task_class: Some(PacketTaskClassDto::SymbolOwnership),
            plan: PacketPlanDto {
                task_class: PacketTaskClassDto::SymbolOwnership,
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
            sufficiency,
            retrieval_trace_summary,
        }
    }

    fn test_citation(display_name: &str, file_path: &str) -> AgentCitationDto {
        AgentCitationDto {
            node_id: NodeId(display_name.to_string()),
            display_name: display_name.to_string(),
            kind: NodeKind::FUNCTION,
            file_path: Some(file_path.to_string()),
            line: Some(10),
            score: 0.9,
            origin: SearchHitOrigin::IndexedSymbol,
            target: None,
            resolvable: true,
            subgraph_id: None,
            evidence_edge_ids: Vec::new(),
            retrieval_score_breakdown: None,
            evidence_tier: None,
            evidence_producer: None,
            resolution_status: None,
            loss_reason: None,
            coverage_role: None,
            eligible_for_sufficiency: Some(true),
        }
    }

    fn retained_exact_path_resolution(input_index: u32, path: &str) -> PacketProbeResolutionDto {
        PacketProbeResolutionDto {
            input_index,
            probe: PacketProbeDto::ExactPath {
                path: path.to_string(),
            },
            status: PacketProbeResolutionStatusDto::ExactPath,
            normalized_query: Some(path.to_string()),
            path: Some(path.to_string()),
            symbol_id: None,
            candidates: Vec::new(),
            rejection: None,
        }
    }

    fn retained_graph_citation(display_name: &str, file_path: &str) -> AgentCitationDto {
        let mut citation = test_citation(display_name, file_path);
        citation.evidence_tier = Some(PacketEvidenceTierDto::ResolvedGraph);
        citation.evidence_producer = Some("route_endpoint".to_string());
        citation.resolution_status = Some(PacketEvidenceResolutionDto::Resolved);
        citation
    }

    fn install_duplicate_summary_trace_payload(packet: &mut AgentPacketDto, repeat: usize) {
        packet.answer.retrieval_trace.request_id = "canonical-answer-trace".to_string();
        packet.answer.retrieval_trace.total_latency_ms = 123;
        packet.answer.retrieval_trace.sla_target_ms = Some(1_000);
        packet.answer.retrieval_trace.sla_missed = true;
        packet.answer.retrieval_trace.annotations = vec![
            codestory_contracts::api::RetrievalAnnotationDto::observation(format!(
                "canonical trace annotation {}",
                "answer-retained ".repeat(repeat)
            )),
        ];
        packet.answer.retrieval_trace.steps = vec![
            AgentRetrievalStepDto {
                kind: AgentRetrievalStepKindDto::Search,
                status: AgentRetrievalStepStatusDto::Ok,
                duration_ms: 10,
                input: Vec::new(),
                output: Vec::new(),
                message: Some("search duplicate diagnostic ".repeat(repeat)),
            },
            AgentRetrievalStepDto {
                kind: AgentRetrievalStepKindDto::Trail,
                status: AgentRetrievalStepStatusDto::Ok,
                duration_ms: 20,
                input: Vec::new(),
                output: Vec::new(),
                message: Some("trail duplicate diagnostic ".repeat(repeat)),
            },
            AgentRetrievalStepDto {
                kind: AgentRetrievalStepKindDto::SourceRead,
                status: AgentRetrievalStepStatusDto::Ok,
                duration_ms: 30,
                input: Vec::new(),
                output: Vec::new(),
                message: Some("source duplicate diagnostic ".repeat(repeat)),
            },
        ];
        packet.retrieval_trace_summary = PacketRetrievalTraceSummaryDto {
            retrieval_trace: packet.answer.retrieval_trace.clone(),
            source_read_steps: 1,
            search_steps: 1,
            trail_steps: 1,
        };
    }

    fn install_verbose_semantic_stage_shadow(packet: &mut AgentPacketDto) {
        let shadow = serde_json::from_value::<RetrievalShadowDto>(serde_json::json!({
            "retrieval_mode": "full",
            "degraded_reason": "semantic_runtime_degraded",
            "retrieval_total_ms": 88,
            "total_budget_ms": 100,
            "cancel_reason": "stage_deadline",
            "cache_hit": false,
            "stage_timings": [
                {
                    "stage": "stage1b_semantic",
                    "deadline_ms": 50,
                    "elapsed_ms": 40,
                    "admission_wait_ms": 3,
                    "queue_wait_ms": 2,
                    "execution_ms": 35,
                    "candidates_added": 4,
                    "marginal_gain": 0.75,
                    "cache_hit": false,
                    "sidecar_latency_ms": 35,
                    "degraded": false,
                    "completion_status": "completed"
                },
                {
                    "stage": "stage2_semantic_vector",
                    "deadline_ms": 50,
                    "elapsed_ms": 48,
                    "admission_wait_ms": 4,
                    "queue_wait_ms": 3,
                    "execution_ms": 41,
                    "candidates_added": 0,
                    "marginal_gain": 0.0,
                    "cancel_reason": "stage_deadline",
                    "cache_hit": false,
                    "sidecar_latency_ms": 41,
                    "degraded": true,
                    "stub_reason": "semantic_runtime_degraded",
                    "completion_status": "cancelled_before_start"
                }
            ],
            "would_rank": ["verbose semantic candidate detail ".repeat(180)],
            "candidate_count": 4,
            "resolved_hit_count": 2,
            "unresolved_candidate_count": 2,
            "diagnostic_only": false,
            "candidate_resolution_counts": [
                { "resolution": "semantic candidate detail", "count": 4 }
            ]
        }))
        .expect("semantic retrieval shadow fixture");
        packet.answer.retrieval_trace.retrieval_shadow = Some(shadow);
        packet.retrieval_trace_summary = packet_retrieval_trace_summary(&packet.answer);
    }

    fn test_project_root() -> &'static Path {
        Path::new("C:/workspace/project root")
    }
}
