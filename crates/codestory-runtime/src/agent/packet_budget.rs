use crate::agent::packet_candidate::is_packet_candidate_selection_view_id;
use crate::agent::packet_capping::cap_packet_citations_in_repository_order;
use crate::agent::trace_export::{
    PACKET_STEP_TRACE_ANNOTATION_PREFIX, compact_retained_packet_step_trace_for_budget,
    packet_retrieval_trace_summary, retain_packet_step_trace_for_export,
};
use codestory_contracts::api::{
    AgentAnswerDto, AgentPacketDto, AgentResponseBlockDto, AgentRetrievalStepKindDto,
    AgentRetrievalStepStatusDto, ApiError, EdgeId, GraphArtifactDto, GraphResponse,
    PacketBudgetDto, PacketBudgetLimitsDto, PacketBudgetModeDto, PacketBudgetUsageDto,
    RetrievalShadowDto, RetrievalStageTimingDto,
};
use std::collections::{HashMap, HashSet};
use std::path::Path;

#[allow(unused_imports)]
pub(crate) use codestory_agent::packet_command::next_deeper_packet_argv;
pub(crate) use codestory_agent::packet_command::next_deeper_packet_command;

const MARKDOWN_TRUNCATION_FLOOR_BYTES: usize = 256;
pub(super) const PACKET_MARKDOWN_TRUNCATION_SUFFIX: &str =
    "\n\n... packet section truncated by budget ...\n";
const ANSWER_RETRIEVAL_DIAGNOSTICS_OMISSION: &str = "answer.retrieval_trace.diagnostics";
const MINIMAL_PARTIAL_OMISSION: &str = "minimal_partial";
const RETRIEVAL_TRACE_SUMMARY_OMISSION: &str = "retrieval_trace_summary";
const RETAINED_STEP_TRACE_DETAIL_OMISSION: &str = "answer.retrieval_trace.step_details";
const PACKET_PROOF_RESERVE_PERCENT: usize = 70;
const PACKET_GRAPH_MAX_PERCENT: usize = 20;
const PACKET_DIAGNOSTICS_MAX_PERCENT: usize = 10;

pub(crate) fn packet_budget_limits(mode: PacketBudgetModeDto) -> PacketBudgetLimitsDto {
    let mut limits = match mode {
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
            max_anchors: 16,
            max_files: 25,
            max_snippets: 80,
            max_trail_edges: 240,
            max_output_bytes: 512 * 1024,
        },
    };
    limits.max_output_bytes = limits
        .max_output_bytes
        .min(codestory_contracts::compilation::PUBLIC_PACKET_SERIALIZED_MAX_BYTES as u32);
    limits
}

pub(crate) fn apply_packet_budget(
    project_root: &Path,
    question: &str,
    requested: PacketBudgetModeDto,
    limits: PacketBudgetLimitsDto,
    answer: &mut AgentAnswerDto,
) -> PacketBudgetDto {
    let mut truncated = false;
    let mut omitted_sections = Vec::new();

    if cap_packet_citations_in_repository_order(answer, &limits) {
        truncated = true;
        omitted_sections.push("citations".to_string());
    }
    if cap_graph_edges(answer, limits.max_trail_edges, &[]) {
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
    if enforce_packet_output_budget_for_representation(project_root, packet, serialized_packet_len)
        .is_err()
    {
        packet.budget.truncated = true;
        push_omitted_section(&mut packet.budget, "serialized_public_budget");
    }
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
) -> Result<(), ApiError> {
    let graph_shape_changed = canonicalize_packet_graphs_and_references(&mut packet.answer);
    if graph_shape_changed {
        packet.budget.truncated = true;
        push_omitted_section(&mut packet.budget, "trail_edges");
    }
    if packet
        .budget
        .omitted_sections
        .iter()
        .any(|section| section == MINIMAL_PARTIAL_OMISSION)
    {
        let final_bytes =
            refresh_packet_budget_usage_for_representation(packet, &representation_len);
        if final_bytes <= packet.budget.limits.max_output_bytes as usize {
            return Ok(());
        }
        return Err(packet_output_budget_exceeded_error(packet, final_bytes));
    }
    let section_budget_changed = enforce_packet_section_budgets(packet, &representation_len);
    let mut needs_dependent_rebuild = graph_shape_changed
        || section_budget_changed
        || packet
            .budget
            .omitted_sections
            .iter()
            .any(|section| section == "output_bytes" || section == "packet_payload");
    let mut dependent_shape_rebuilt = false;
    loop {
        let output_bytes = if needs_dependent_rebuild {
            dependent_shape_rebuilt = true;
            refresh_packet_after_budget_mutation(project_root, packet, &representation_len)
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
                return Ok(());
            }

            // Probe the fully rebuilt marker-free shape before committing its removal: it can be larger
            // than the marker-present shape. If that shape does not fit, the measured marker shape
            // is the truthful irreducible result.
            let marker_shape = packet.clone();
            remove_omitted_section(&mut packet.budget, "output_bytes");
            remove_omitted_section(&mut packet.budget, "packet_payload");
            let marker_free_output_bytes =
                refresh_packet_after_budget_mutation(project_root, packet, &representation_len);
            if marker_free_output_bytes <= packet.budget.limits.max_output_bytes as usize {
                return Ok(());
            }
            *packet = marker_shape;
            return Ok(());
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
        push_omitted_section(&mut packet.budget, "serialized_public_budget");

        let over_by = output_bytes.saturating_sub(packet.budget.limits.max_output_bytes as usize);
        let current_answer_bytes = serde_json::to_vec(&packet.answer)
            .map(|bytes| bytes.len())
            .unwrap_or_default();
        let next_answer_cap = current_answer_bytes
            .saturating_sub(over_by.saturating_add(1024))
            .max(1024);

        let mut structurally_trimmed = false;
        let trimmed_verbose_sections = trim_packet_verbose_plan_lists(packet);
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
        if minimize_packet_for_hard_budget(packet) {
            let final_bytes =
                refresh_packet_budget_usage_for_representation(packet, &representation_len);
            if final_bytes <= packet.budget.limits.max_output_bytes as usize {
                return Ok(());
            }
        }
        let final_bytes =
            refresh_packet_budget_usage_for_representation(packet, &representation_len);
        return Err(packet_output_budget_exceeded_error(packet, final_bytes));
    }
}

fn packet_output_budget_exceeded_error(packet: &AgentPacketDto, final_bytes: usize) -> ApiError {
    ApiError::new(
        "packet_output_budget_exceeded",
        format!(
            "packet mandatory envelope is {final_bytes} bytes, exceeding the {}-byte output cap",
            packet.budget.limits.max_output_bytes
        ),
    )
}

/// Keep optional packet sections inside their shares of the exact adapter envelope before the
/// hard-cap fixpoint starts.
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
        let trimmed_sections = trim_packet_verbose_plan_lists(packet);
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
    let _ = trim_packet_verbose_plan_lists(packet);
    let _ = trim_packet_retrieval_trace_summary(packet);
    let _ = trim_packet_answer_retrieval_diagnostics(packet);
}

fn minimize_packet_for_hard_budget(packet: &mut AgentPacketDto) -> bool {
    if packet
        .budget
        .omitted_sections
        .iter()
        .any(|section| section == MINIMAL_PARTIAL_OMISSION)
    {
        return false;
    }

    packet.budget.truncated = true;
    for section in [
        MINIMAL_PARTIAL_OMISSION,
        "citations",
        "trail_edges",
        "markdown_blocks",
        RETRIEVAL_TRACE_SUMMARY_OMISSION,
        ANSWER_RETRIEVAL_DIAGNOSTICS_OMISSION,
    ] {
        push_omitted_section(&mut packet.budget, section);
    }

    packet.plan.queries.clear();
    packet.plan.trace.clear();
    packet.plan.probe_resolutions.clear();

    packet.answer.prompt.clear();
    packet.answer.summary.clear();
    packet.answer.source_coverage.clear();
    packet.answer.sections.clear();
    packet.answer.subgraph_ids.clear();
    packet.answer.retrieval_trace.request_id.clear();
    packet.answer.retrieval_trace.semantic_fallback_count = 0;
    packet.answer.retrieval_trace.semantic_fallbacks.clear();
    packet.answer.retrieval_trace.annotations.clear();
    packet.answer.retrieval_trace.steps.clear();
    packet
        .answer
        .retrieval_trace
        .packet_sidecar_diagnostics
        .clear();
    if let Some(shadow) = packet.answer.retrieval_trace.retrieval_shadow.as_mut() {
        let _ = trim_retrieval_shadow_verbose_diagnostics(shadow);
    }
    packet.retrieval_trace_summary = packet_retrieval_trace_summary(&packet.answer);
    packet
        .disposition
        .omission_receipts
        .push("serialized_public_budget".to_string());
    // Hard-budget minimizer may drop traces and duplicate ledgers. It must not
    // change the compiled disposition or drop the only retained support units.
    true
}

fn packet_optional_graph_bytes(
    packet: &AgentPacketDto,
    representation_len: &impl Fn(&AgentPacketDto) -> usize,
) -> usize {
    let full = representation_len(packet);
    let mut proof_only = packet.clone();
    retain_required_graph_proof_only(&mut proof_only.answer, &HashSet::new());
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
    if let Some(index) = packet
        .answer
        .graphs
        .iter()
        .rposition(|artifact| matches!(artifact, GraphArtifactDto::Mermaid { .. }))
    {
        packet.answer.graphs.remove(index);
        let _ = prune_packet_graph_references(&mut packet.answer);
        return true;
    }
    if let Some(index) = packet
        .answer
        .graphs
        .iter()
        .rposition(|artifact| match artifact {
            GraphArtifactDto::Uml { graph, .. } => graph.edges.is_empty(),
            GraphArtifactDto::Mermaid { .. } => false,
        })
    {
        packet.answer.graphs.remove(index);
        let _ = prune_packet_graph_references(&mut packet.answer);
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
    if total_edges == 0 {
        return false;
    }

    cap_graph_edges(
        &mut packet.answer,
        total_edges.saturating_sub(1).try_into().unwrap_or(u32::MAX),
        &[],
    )
}

fn refresh_packet_after_budget_mutation(
    project_root: &Path,
    packet: &mut AgentPacketDto,
    representation_len: &impl Fn(&AgentPacketDto) -> usize,
) -> usize {
    rebuild_packet_budget_dependents(project_root, packet);
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

fn trim_packet_verbose_plan_lists(packet: &mut AgentPacketDto) -> Vec<&'static str> {
    let mut trimmed_sections = Vec::new();
    if !packet.plan.trace.is_empty() {
        packet.plan.trace.clear();
        trimmed_sections.push("plan.trace");
    }
    if !packet.plan.queries.is_empty() {
        packet.plan.queries.clear();
        trimmed_sections.push("plan.queries");
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
    // Gaps affect evidence availability, and the scalar packet-step record is the retained provenance for
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

fn rebuild_packet_budget_dependents(_project_root: &Path, packet: &mut AgentPacketDto) {
    packet.retrieval_trace_summary = packet_retrieval_trace_summary(&packet.answer);
    let trim_trace_summary = packet
        .budget
        .omitted_sections
        .iter()
        .any(|section| section == RETRIEVAL_TRACE_SUMMARY_OMISSION);
    if trim_trace_summary {
        let _ = trim_packet_retrieval_trace_summary(packet);
    }
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
    let max_edges = max_edges as usize;
    // Ordinary packet graphs keep one canonical owner per edge ID. A fingerprinted packet
    // candidate graph is instead one bounded source view: its opaque omission count is meaningful
    // only beside that view's own retained edge occurrences. Preserve those occurrences across
    // views, even when IDs overlap; they consume the same unchanged global edge budget.
    let mut canonical_owners = HashMap::<EdgeId, (String, usize, usize)>::new();
    let mut selectable_occurrences = Vec::<(EdgeId, String, usize, usize)>::new();
    for (artifact_index, artifact) in answer.graphs.iter().enumerate() {
        let GraphArtifactDto::Uml { id, graph, .. } = artifact else {
            continue;
        };
        for (edge_index, edge) in graph.edges.iter().enumerate() {
            let candidate = (id.clone(), artifact_index, edge_index);
            if is_packet_candidate_selection_view_id(id) {
                selectable_occurrences.push((
                    edge.id.clone(),
                    id.clone(),
                    artifact_index,
                    edge_index,
                ));
            } else {
                canonical_owners
                    .entry(edge.id.clone())
                    .and_modify(|current| {
                        if candidate < current.clone() {
                            *current = candidate.clone();
                        }
                    })
                    .or_insert(candidate);
            }
        }
    }
    selectable_occurrences.extend(canonical_owners.iter().map(
        |(edge_id, (artifact_id, artifact_index, edge_index))| {
            (
                edge_id.clone(),
                artifact_id.clone(),
                *artifact_index,
                *edge_index,
            )
        },
    ));
    selectable_occurrences.sort_by(
        |(left_edge, left_artifact, left_artifact_index, left_edge_index),
         (right_edge, right_artifact, right_artifact_index, right_edge_index)| {
            left_edge
                .0
                .cmp(&right_edge.0)
                .then(left_artifact.cmp(right_artifact))
                .then(left_artifact_index.cmp(right_artifact_index))
                .then(left_edge_index.cmp(right_edge_index))
        },
    );

    let mut selected_occurrences = Vec::<(usize, usize)>::new();
    let mut selected_set = HashSet::<(usize, usize)>::new();
    if max_edges > 0 {
        let mut protected_ids_selected = HashSet::new();
        for edge_id in protected_edge_ids {
            if !protected_ids_selected.insert(edge_id.clone()) {
                continue;
            }
            if let Some((_, _, artifact_index, edge_index)) = selectable_occurrences.iter().find(
                |(candidate_id, _, artifact_index, edge_index)| {
                    candidate_id == edge_id
                        && !selected_set.contains(&(*artifact_index, *edge_index))
                },
            ) && selected_set.insert((*artifact_index, *edge_index))
            {
                selected_occurrences.push((*artifact_index, *edge_index));
            }
            if selected_occurrences.len() >= max_edges {
                break;
            }
        }
    }
    for (_, _, artifact_index, edge_index) in &selectable_occurrences {
        if selected_occurrences.len() >= max_edges {
            break;
        }
        if selected_set.insert((*artifact_index, *edge_index)) {
            selected_occurrences.push((*artifact_index, *edge_index));
        }
    }
    let selected_order = selected_occurrences
        .iter()
        .enumerate()
        .map(|(index, occurrence)| (*occurrence, index))
        .collect::<HashMap<_, _>>();

    let mut truncated = false;
    for (artifact_index, artifact) in answer.graphs.iter_mut().enumerate() {
        let GraphArtifactDto::Uml { graph, .. } = artifact else {
            continue;
        };
        let original_len = graph.edges.len();
        let mut retained = Vec::with_capacity(original_len.min(max_edges));
        for (edge_index, edge) in graph.edges.drain(..).enumerate() {
            let occurrence = (artifact_index, edge_index);
            if let Some(order) = selected_order.get(&occurrence) {
                retained.push((*order, edge));
            }
        }
        retained.sort_by_key(|(order, _)| *order);
        graph.edges = retained.into_iter().map(|(_, edge)| edge).collect();
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
    let original_graph_count = answer.graphs.len();
    answer.graphs.retain(|artifact| match artifact {
        GraphArtifactDto::Uml { graph, .. } => !graph.edges.is_empty(),
        GraphArtifactDto::Mermaid { .. } => true,
    });
    truncated |= answer.graphs.len() != original_graph_count;
    truncated |= prune_packet_graph_references(answer);
    truncated
}

fn canonicalize_packet_graphs_and_references(answer: &mut AgentAnswerDto) -> bool {
    cap_graph_edges(answer, u32::MAX, &[])
}

fn prune_packet_graph_references(answer: &mut AgentAnswerDto) -> bool {
    let retained_graph_ids = answer
        .graphs
        .iter()
        .map(|artifact| match artifact {
            GraphArtifactDto::Uml { id, .. } | GraphArtifactDto::Mermaid { id, .. } => id.clone(),
        })
        .collect::<HashSet<_>>();
    let retained_edge_ids = answer
        .graphs
        .iter()
        .filter_map(|artifact| match artifact {
            GraphArtifactDto::Uml { graph, .. } => Some(graph.edges.iter()),
            GraphArtifactDto::Mermaid { .. } => None,
        })
        .flatten()
        .map(|edge| edge.id.clone())
        .collect::<HashSet<_>>();
    let mut changed = false;

    for citation in &mut answer.citations {
        let original_edge_count = citation.evidence_edge_ids.len();
        citation
            .evidence_edge_ids
            .retain(|edge_id| retained_edge_ids.contains(edge_id));
        let mut seen = HashSet::new();
        citation
            .evidence_edge_ids
            .retain(|edge_id| seen.insert(edge_id.clone()));
        changed |= citation.evidence_edge_ids.len() != original_edge_count;
        if citation
            .subgraph_id
            .as_ref()
            .is_some_and(|graph_id| !retained_graph_ids.contains(graph_id))
        {
            citation.subgraph_id = None;
            changed = true;
        }
    }

    let original_subgraph_count = answer.subgraph_ids.len();
    let mut seen = HashSet::new();
    answer
        .subgraph_ids
        .retain(|graph_id| retained_graph_ids.contains(graph_id) && seen.insert(graph_id.clone()));
    changed |= answer.subgraph_ids.len() != original_subgraph_count;

    for section in &mut answer.sections {
        let original_block_count = section.blocks.len();
        section.blocks.retain(|block| match block {
            AgentResponseBlockDto::Mermaid { graph_id } => retained_graph_ids.contains(graph_id),
            AgentResponseBlockDto::Markdown { .. } => true,
        });
        changed |= section.blocks.len() != original_block_count;
    }
    changed
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
    let original_layout_source_edges = graph
        .canonical_layout
        .as_ref()
        .map(|layout| {
            layout
                .edges
                .iter()
                .map(|edge| edge.source_edge_ids.len())
                .sum::<usize>()
        })
        .unwrap_or_default();
    let original_center_id = graph.center_id.clone();
    let mut retained_node_ids = HashSet::new();
    let retained_edge_ids = graph
        .edges
        .iter()
        .map(|edge| edge.id.clone())
        .collect::<HashSet<_>>();

    for edge in &graph.edges {
        retained_node_ids.insert(edge.source.clone());
        retained_node_ids.insert(edge.target.clone());
    }

    if !retained_node_ids.contains(&graph.center_id)
        && let Some(edge) = graph.edges.first()
    {
        graph.center_id = edge.source.clone();
    }

    graph
        .nodes
        .retain(|node| retained_node_ids.contains(&node.id));

    let center_id = graph.center_id.clone();
    if let Some(layout) = graph.canonical_layout.as_mut() {
        layout.center_node_id = center_id.clone();
        layout.edges.retain_mut(|edge| {
            let endpoints_retained = retained_node_ids.contains(&edge.source)
                && retained_node_ids.contains(&edge.target);
            let had_source_edges = !edge.source_edge_ids.is_empty();
            let mut seen = HashSet::new();
            edge.source_edge_ids.retain(|edge_id| {
                retained_edge_ids.contains(edge_id) && seen.insert(edge_id.clone())
            });
            let source_edge_retained = !had_source_edges || !edge.source_edge_ids.is_empty();
            endpoints_retained && source_edge_retained
        });
        layout.nodes.retain_mut(|node| {
            node.center = node.id == center_id;
            retained_node_ids.contains(&node.id)
        });
    }

    let pruned = graph.center_id != original_center_id
        || graph.nodes.len() < original_nodes
        || graph
            .canonical_layout
            .as_ref()
            .map(|layout| layout.nodes.len() < original_layout_nodes)
            .unwrap_or(false)
        || graph
            .canonical_layout
            .as_ref()
            .map(|layout| layout.edges.len() < original_layout_edges)
            .unwrap_or(false)
        || graph
            .canonical_layout
            .as_ref()
            .map(|layout| {
                layout
                    .edges
                    .iter()
                    .map(|edge| edge.source_edge_ids.len())
                    .sum::<usize>()
                    < original_layout_source_edges
            })
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

/// Lower is truncated first. This mirrors `packet_section_order_rank`: what leads the packet
/// is what a capped consumer actually reads, so it is also what the hard-budget minimizer
/// must protect. The ledger renders structured citations that survive
/// truncation, so cutting its duplicate markdown loses nothing a consumer
/// cannot recover.
fn packet_markdown_truncation_priority(section_id: &str) -> u8 {
    if section_id == "diagrams" {
        return 0;
    }
    if section_id == "packet-evidence-ledger" || section_id == "packet-flow-claims" {
        return 1;
    }
    if section_id.starts_with("packet-subquery-") {
        return 2;
    }
    if section_id == "retrieval-evidence" {
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
    use codestory_contracts::api::{
        AgentAnswerDto, AgentPacketDto, AgentResponseBlockDto, AgentResponseSectionDto,
        AgentRetrievalPolicyModeDto, AgentRetrievalPresetDto, AgentRetrievalTraceDto,
        PacketBudgetDto, PacketBudgetLimitsDto, PacketBudgetModeDto, PacketBudgetUsageDto,
        PacketDispositionDto, PacketPlanDto, PacketRetrievalTraceSummaryDto,
    };

    pub(in crate::agent) fn test_packet(question: &str, max_output_bytes: u32) -> AgentPacketDto {
        let answer = AgentAnswerDto {
            source_coverage: Vec::new(),
            answer_id: "packet-budget-test".to_string(),
            prompt: question.to_string(),
            summary: "Packet budget test answer.".to_string(),
            freshness: None,
            sections: vec![AgentResponseSectionDto {
                id: "answer".to_string(),
                title: "Answer".to_string(),
                blocks: vec![AgentResponseBlockDto::Markdown {
                    markdown: "Bounded source-backed evidence.".to_string(),
                }],
            }],
            citations: Vec::new(),
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
                source_freshness_telemetry: None,
                steps: Vec::new(),
                packet_sidecar_diagnostics: Vec::new(),
                retrieval_shadow: None,
            },
        };
        let budget = PacketBudgetDto {
            requested: PacketBudgetModeDto::Compact,
            limits: PacketBudgetLimitsDto {
                max_anchors: 16,
                max_files: 16,
                max_snippets: 16,
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
        let retrieval_trace_summary = PacketRetrievalTraceSummaryDto {
            retrieval_trace: answer.retrieval_trace.clone(),
            source_read_steps: 0,
            search_steps: 0,
            trail_steps: 0,
        };

        AgentPacketDto {
            packet_id: answer.answer_id.clone(),
            question: question.to_string(),
            plan: PacketPlanDto {
                queries: Vec::new(),
                probe_resolutions: Vec::new(),
                trace: Vec::new(),
            },
            answer,
            budget,
            support: Vec::new(),
            disposition: PacketDispositionDto::supported(),
            retrieval_trace_summary,
            answer_sufficiency: Default::default(),
        }
    }

    #[test]
    fn public_packet_budget_fixture_contains_no_answer_shape_policy() {
        let packet = test_packet("Explain any repository", 16 * 1024);
        let value = serde_json::to_value(packet).expect("serialize packet");
        let plan = value.get("plan").expect("plan");
        assert!(plan.get("task_class").is_none());
        assert!(plan.get("obligations").is_none());
        assert_eq!(
            value.get("answer_sufficiency"),
            Some(&serde_json::json!("not_asserted"))
        );
    }
}
