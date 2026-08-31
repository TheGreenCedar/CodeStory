use crate::agent::packet_candidate::is_packet_candidate_selection_view_id;
use crate::agent::packet_capping::cap_packet_citations_with_obligation_carriers;
use crate::agent::packet_claims::{
    packet_flow_claims_markdown, packet_supported_claims_with_telemetry,
};
use crate::agent::packet_obligations::{
    bind_claims_to_packet_obligations, packet_claims_with_obligation_receipts,
    refinalize_packet_obligation_plan_after_rebuild,
};
use crate::agent::packet_plan::{packet_explicit_request_probe_queries, push_unique_term};
use crate::agent::packet_required_probes::packet_sufficiency_required_probe_queries_with_extra;
use crate::agent::trace_export::{
    PACKET_STEP_TRACE_ANNOTATION_PREFIX, compact_retained_packet_step_trace_for_budget,
    packet_retrieval_trace_summary, retain_packet_step_trace_for_export,
};
use codestory_contracts::api::{
    AgentAnswerDto, AgentPacketDto, AgentResponseBlockDto, AgentRetrievalStepKindDto,
    AgentRetrievalStepStatusDto, ApiError, EdgeId, EdgeKind, GraphArtifactDto, GraphResponse,
    PacketBudgetDto, PacketBudgetLimitsDto, PacketBudgetModeDto, PacketBudgetUsageDto,
    PacketObligationProofStatusDto, PacketQueryCompletionDto, PacketTaskClassDto,
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

#[allow(clippy::too_many_arguments)]
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
    let protected_edges = protected_graph_edge_ids_for_budget(answer, obligation_edge_ids);
    if cap_graph_edges(answer, limits.max_trail_edges, &protected_edges) {
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
    enforce_packet_output_budget_for_representation(project_root, packet, serialized_packet_len)
        .expect("supported packet budget must admit its minimal typed representation");
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
    let extra_probes = packet_explicit_request_probe_queries(&packet.plan);
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
                return Ok(());
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

    let mut gaps = Vec::new();
    packet
        .plan
        .obligations
        .claim_obligations
        .retain(|obligation| obligation.material);
    for obligation in &mut packet.plan.obligations.claim_obligations {
        obligation.binding_terms.clear();
        obligation.probe_binding = None;
        obligation.allowed_node_kinds.clear();
        obligation.required_edge_kind = None;
        obligation.requires_complete_discovery = false;
        obligation.proof_status = PacketObligationProofStatusDto::Reported;
        obligation.reason = Some("packet_budget_truncated".to_string());
        obligation.carrier_node_ids.clear();
        obligation.carrier_paths.clear();
        obligation.carrier_edge_proofs.clear();
        gaps.push(format!(
            "obligation {} ({:?}) is Reported: packet_budget_truncated",
            obligation.id, obligation.kind
        ));
    }
    packet
        .plan
        .obligations
        .query_obligations
        .retain(|obligation| obligation.material);
    for obligation in &packet.plan.obligations.query_obligations {
        if let Some(PacketQueryCompletionDto::Cancelled { reason }) = &obligation.completion {
            gaps.push(format!(
                "query obligation {} ({:?}) is cancelled: {}",
                obligation.id, obligation.kind, reason
            ));
        } else if obligation.completion.is_none() {
            gaps.push(format!(
                "query obligation {} ({:?}) is cancelled: completion_missing",
                obligation.id, obligation.kind
            ));
        }
    }
    packet.plan.obligations.binding_terms.clear();
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
    packet.answer.retrieval_trace.packet_claim_profile_telemetry = None;
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
    packet.disposition.omission_receipts = gaps;
    // Hard-budget minimizer may drop traces and duplicate ledgers. It must not
    // change the compiled disposition or drop the only retained support units.
    true
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
        let _ = prune_packet_graph_references(&mut packet.answer);
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

    // ATOM-AWARE SHAVE ORDER (gate 9). This trimmer removes exactly one edge
    // to fit `max_output_bytes`, and `cap_graph_edges` fills its selection
    // from the passed ids first and then by LEXICOGRAPHIC EDGE-ID STRING, so
    // the victim used to be whichever edge id happened to sort last —
    // arbitrary with respect to atom need. Ranking atom-required edges into
    // the selection order makes the victim a non-atom edge whenever one
    // exists, and falls back to an atom-required edge only when nothing else
    // is left (the selection stops one short of the total, so exactly one
    // edge is always dropped).
    //
    // TRIMMABILITY IS UNCHANGED, deliberately: the `total_edges <=
    // protected_present` check above still counts ONLY the obligation ids, so
    // the trimmer can never answer "cannot trim" because of atom need. Atom
    // need is a selection input, never a protection guarantee at a byte
    // budget — `max_output_bytes` is a publication invariant and does not
    // bend for it.
    let mut shave_order = protected;
    let atom_required = atom_required_graph_edge_ids(&packet.answer);
    for artifact in &packet.answer.graphs {
        let GraphArtifactDto::Uml { graph, .. } = artifact else {
            continue;
        };
        for edge in &graph.edges {
            if atom_required.contains(&edge.id) && !protected_set.contains(&edge.id) {
                shave_order.push(edge.id.clone());
            }
        }
    }
    cap_graph_edges(
        &mut packet.answer,
        total_edges.saturating_sub(1).try_into().unwrap_or(u32::MAX),
        &shave_order,
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
    _project_root: &Path,
    packet: &mut AgentPacketDto,
    _extra_probes: &[String],
) {
    packet.retrieval_trace_summary = packet_retrieval_trace_summary(&packet.answer);
    let task_class = packet
        .task_class
        .unwrap_or(PacketTaskClassDto::ArchitectureExplanation);
    // R7(d): the budget fixpoint is a REBUILD site, not a proving site. Legacy
    // obligations get today's full re-finalization bit-identically; formula
    // obligations are re-verified by receipt survival only against the
    // survivors at rebuild time (retained citations/support, current graphs) —
    // never re-proven, never promoted. Proving happened once, at the primary
    // finalize with the caller's evidence extras.
    refinalize_packet_obligation_plan_after_rebuild(
        &packet.question,
        task_class,
        &mut packet.plan.obligations,
        &packet.answer,
        &packet.budget,
        &packet.support,
    );
    refresh_packet_claim_markdown(packet);
    let trim_trace_summary = packet
        .budget
        .omitted_sections
        .iter()
        .any(|section| section == RETRIEVAL_TRACE_SUMMARY_OMISSION);
    if trim_trace_summary {
        let _ = trim_packet_retrieval_trace_summary(packet);
    }
}

fn refresh_packet_claim_markdown(packet: &mut AgentPacketDto) {
    if !packet
        .answer
        .sections
        .iter()
        .any(|section| section.id == "packet-flow-claims")
    {
        return;
    }
    let supported_claims_with_telemetry = packet_supported_claims_with_telemetry(&packet.answer);
    let mut claims = packet_claims_with_obligation_receipts(
        &packet.answer,
        packet.plan.task_class,
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
        boundary = refreshed[..boundary]
            .rfind('\n')
            .map(|newline| newline + 1)
            .unwrap_or(0);
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

/// Edges the compact graph cap must keep so a later reader can still name what
/// a cited carrier does. Obligation proofs come first; then protected-kind
/// edges whose both endpoints are cited; then any remaining incident
/// protected-kind and already-attached citation evidence ids. Compact used to
/// pick the first 20 edges by id, drop the rest, and strip
/// `evidence_edge_ids` that no longer appeared in `answer.graphs` — which is
/// how a packet that had resolved a CALL shipped a pointer receipt instead of
/// the relation. R2 widens the protected kinds from CALL|INHERITANCE to every
/// atom-required kind (TYPE_USAGE, USAGE, MEMBER, IMPORT), so retained atom
/// receipts survive the cap the same way CALL proof does.
/// The graph edges an atom receipt requires, read from the active proof
/// session: an edge in a recorded scan's narrowed coverage set (a rule-7
/// completeness claim is void the moment one of its enumerated edges leaves
/// the evidence), or an edge with an endpoint the formulas' typed patterns
/// put in the need-set.
///
/// This is a SELECTION input and never a PROOF input (contract rule 4), and
/// it is never a protection GUARANTEE at a byte-budget boundary — see
/// [`trim_one_optional_graph_unit`], which uses it to choose a victim, not to
/// refuse to trim. Empty without an active session and for every packet with
/// no formula-bearing requirement, which keeps Legacy behavior identical.
fn atom_required_graph_edge_ids(answer: &AgentAnswerDto) -> HashSet<EdgeId> {
    let Some(session) = crate::agent::packet_candidate::active_packet_proof_session() else {
        return HashSet::new();
    };
    let mut required = session
        .artifact_scans()
        .into_iter()
        .flat_map(|(_, scans)| scans)
        .flat_map(|scan| scan.coverage_edge_ids)
        .collect::<HashSet<_>>();
    let endpoint_is_atom_needed = |node_id: &codestory_contracts::api::NodeId| {
        node_id
            .0
            .parse::<i64>()
            .is_ok_and(|identity| session.identity_is_atom_needed(identity))
    };
    for artifact in &answer.graphs {
        let GraphArtifactDto::Uml { graph, .. } = artifact else {
            continue;
        };
        for edge in &graph.edges {
            if endpoint_is_atom_needed(&edge.source) || endpoint_is_atom_needed(&edge.target) {
                required.insert(edge.id.clone());
            }
        }
    }
    required
}

fn protected_graph_edge_ids_for_budget(
    answer: &AgentAnswerDto,
    obligation_edge_ids: &[EdgeId],
) -> Vec<EdgeId> {
    let cited = answer
        .citations
        .iter()
        .map(|citation| citation.node_id.clone())
        .collect::<HashSet<_>>();
    // ATOM-NEED PROTECTION (gate 9, contract R2 "protects atom-required
    // edges of any kind"). Everything the graph cap does not protect is
    // selected by LEXICOGRAPHIC EDGE-ID STRING (see `cap_graph_edges`), which
    // is arbitrary with respect to atom need: on a real CSS packet the
    // post-pass built fourteen honest hydration artifacts and the cap kept
    // thirteen edges of one of them, deleting every artifact that lost all
    // its edges — taking the MEMBER receipts C2/C3/C4 need with it.
    //
    // Two things make an edge atom-required, both read from the active proof
    // session and neither of them a proof input (contract rule 4 — atom need
    // selects which receipts survive a bounded stage; receipts alone
    // discharge): the edge is in a recorded scan's narrowed coverage set (a
    // rule-7 completeness claim is void the moment one of those edges leaves
    // the evidence), or one of its endpoints is an identity the formulas'
    // typed patterns put in the need-set.
    //
    // Legacy and M-shard packets install no promotion patterns, so the
    // session yields no coverage sets and an empty need-set, this tier stays
    // empty, and the protection order is exactly what it was. Out-of-process
    // rebuilds have no session either and likewise keep today's behavior —
    // their protection rides the obligation edge ids the DTO carries.
    let atom_required_ids = atom_required_graph_edge_ids(answer);

    let mut both_endpoints = Vec::new();
    let mut atom_required = Vec::new();
    let mut one_endpoint = Vec::new();
    let mut seen_incident = HashSet::new();
    for artifact in &answer.graphs {
        let GraphArtifactDto::Uml { graph, .. } = artifact else {
            continue;
        };
        for edge in &graph.edges {
            if !matches!(
                edge.kind,
                EdgeKind::CALL
                    | EdgeKind::INHERITANCE
                    | EdgeKind::TYPE_USAGE
                    | EdgeKind::USAGE
                    | EdgeKind::MEMBER
                    | EdgeKind::IMPORT
            ) {
                continue;
            }
            if !seen_incident.insert(edge.id.clone()) {
                continue;
            }
            let source_cited = cited.contains(&edge.source);
            let target_cited = cited.contains(&edge.target);
            if source_cited && target_cited {
                both_endpoints.push(edge.id.clone());
            } else if atom_required_ids.contains(&edge.id) {
                atom_required.push(edge.id.clone());
            } else if source_cited || target_cited {
                one_endpoint.push(edge.id.clone());
            }
        }
    }

    let mut protected = Vec::new();
    let mut seen = HashSet::new();
    let push = |id: EdgeId, protected: &mut Vec<EdgeId>, seen: &mut HashSet<EdgeId>| {
        if seen.insert(id.clone()) {
            protected.push(id);
        }
    };
    for id in obligation_edge_ids {
        push(id.clone(), &mut protected, &mut seen);
    }
    for id in both_endpoints {
        push(id, &mut protected, &mut seen);
    }
    for citation in &answer.citations {
        for id in &citation.evidence_edge_ids {
            push(id.clone(), &mut protected, &mut seen);
        }
    }
    for id in atom_required {
        push(id, &mut protected, &mut seen);
    }
    for id in one_endpoint {
        push(id, &mut protected, &mut seen);
    }
    protected
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

#[cfg(test)]
pub(crate) fn cap_packet_graph_edges_for_test(
    answer: &mut AgentAnswerDto,
    max_edges: u32,
    protected_edge_ids: &[EdgeId],
) -> bool {
    cap_graph_edges(answer, max_edges, protected_edge_ids)
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
/// must protect. The ledger and claims sections render `answer.citations` and
/// `sufficiency.covered_claims`, which survive truncation as structured fields, so cutting
/// their markdown loses nothing a consumer cannot recover.
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
    if section_id == "retrieval-evidence" || section_id == "packet-carrier-source" {
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
    use crate::agent::packet_obligations::{
        PacketProofEvidenceExtras, build_packet_obligation_plan, finalize_packet_obligation_plan,
    };
    use crate::agent::trace_export::packet_step_trace_json;
    use codestory_contracts::api::{
        AgentCitationDto, AgentResponseSectionDto, AgentRetrievalPolicyModeDto,
        AgentRetrievalPresetDto, AgentRetrievalStepDto, AgentRetrievalTraceDto, EdgeId, EdgeKind,
        GraphEdgeDto, GraphNodeDto, NodeId, NodeKind, PacketClaimObligationDto,
        PacketClaimObligationKindDto, PacketDispositionDto, PacketDispositionKindDto,
        PacketEvidenceResolutionDto, PacketEvidenceTierDto, PacketObligationCarrierEdgeProofDto,
        PacketObligationProofStatusDto, PacketPlanDto, PacketPlanQueryDto, PacketProbeDto,
        PacketProbeRejectionCodeDto, PacketProbeRejectionDto, PacketProbeResolutionDto,
        PacketProbeResolutionStatusDto, PacketQueryCompletionDto, PacketQueryObligationDto,
        PacketQueryObligationKindDto, PacketRetrievalTraceSummaryDto,
        PacketSidecarQueryDiagnosticDto, SearchHitOrigin,
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

    fn candidate_view_artifact(
        fingerprint: char,
        edge_ids: &[&str],
        omitted_edge_count: u32,
    ) -> GraphArtifactDto {
        let id = format!(
            "packet-search-provenance-{}",
            fingerprint.to_string().repeat(64)
        );
        let mut artifact = budget_graph_artifact(&id, edge_ids);
        let GraphArtifactDto::Uml { graph, .. } = &mut artifact else {
            unreachable!();
        };
        graph.truncated = omitted_edge_count > 0;
        graph.omitted_edge_count = omitted_edge_count;
        artifact
    }

    /// A hydration-shaped artifact with NUMERIC endpoint ids, so the
    /// atom-need protection tier can key on them.
    fn atom_hydration_artifact(artifact_id: &str, edges: &[(&str, i64, i64)]) -> GraphArtifactDto {
        let mut nodes = Vec::new();
        let mut dtos = Vec::new();
        for (edge_id, source, target) in edges {
            for endpoint in [source, target] {
                if !nodes
                    .iter()
                    .any(|node: &GraphNodeDto| node.id.0 == endpoint.to_string())
                {
                    nodes.push(GraphNodeDto {
                        id: NodeId(endpoint.to_string()),
                        label: endpoint.to_string(),
                        kind: NodeKind::FILE,
                        depth: 1,
                        label_policy: None,
                        badge_visible_members: None,
                        badge_total_members: None,
                        merged_symbol_examples: Vec::new(),
                        file_path: None,
                        qualified_name: None,
                        member_access: None,
                    });
                }
            }
            dtos.push(GraphEdgeDto {
                id: EdgeId((*edge_id).to_string()),
                source: NodeId(source.to_string()),
                target: NodeId(target.to_string()),
                kind: EdgeKind::MEMBER,
                confidence: None,
                certainty: None,
                callsite_identity: None,
                candidate_targets: Vec::new(),
            });
        }
        GraphArtifactDto::Uml {
            id: artifact_id.to_string(),
            title: artifact_id.to_string(),
            graph: GraphResponse {
                center_id: NodeId(edges[0].1.to_string()),
                nodes,
                edges: dtos,
                truncated: false,
                omitted_edge_count: 0,
                canonical_layout: None,
            },
        }
    }

    /// A C-family session whose need-set carries `needed`.
    fn atom_session_needing(
        needed: &[i64],
    ) -> std::rc::Rc<crate::agent::packet_candidate::PacketProofSession> {
        let requirements =
            Vec::new();
        let session = std::rc::Rc::new(crate::agent::packet_candidate::PacketProofSession::new(
            crate::agent::packet_candidate::packet_atom_hydration_spec(&requirements),
        ));
        let node = |id: i64| GraphNodeDto {
            id: NodeId(id.to_string()),
            label: id.to_string(),
            kind: NodeKind::FILE,
            depth: 1,
            label_policy: None,
            badge_visible_members: None,
            badge_total_members: None,
            merged_symbol_examples: Vec::new(),
            file_path: None,
            qualified_name: None,
            member_access: None,
        };
        for (index, identity) in needed.iter().enumerate() {
            let partner = 900_000 + index as i64;
            session.record_atom_needed_identities(&GraphResponse {
                center_id: NodeId(identity.to_string()),
                nodes: vec![node(*identity), node(partner)],
                edges: vec![GraphEdgeDto {
                    id: EdgeId(format!("need-{identity}")),
                    source: NodeId(partner.to_string()),
                    target: NodeId(identity.to_string()),
                    kind: EdgeKind::IMPORT,
                    confidence: None,
                    certainty: None,
                    callsite_identity: None,
                    candidate_targets: Vec::new(),
                }],
                truncated: false,
                omitted_edge_count: 0,
                canonical_layout: None,
            });
            assert!(session.identity_is_atom_needed(*identity));
        }
        session
    }

    /// FIX B: the graph cap's unprotected fill order is lexicographic by edge
    /// id, which is arbitrary with respect to atom need — on a real CSS
    /// packet it kept thirteen edges of one hydration artifact and deleted
    /// the other thirteen artifacts outright. An edge an atom receipt
    /// requires now outranks that lexicographic order, and a non-atom edge is
    /// dropped in its place. The cap size itself is untouched.
    #[ignore = "domain role/carrier taxonomy removed (phase9-r2)"]
    #[test]
    fn atom_required_edges_outrank_lexicographic_order_under_the_graph_cap() {
        // "aaa" sorts first and is needed by nothing; "zzz" is a MEMBER
        // receipt whose target identity the formulas require.
        let artifact = atom_hydration_artifact(
            "packet-atom-hydration-77",
            &[("aaa-unrelated", 10, 11), ("zzz-atom-required", 20, 21)],
        );
        let mut answer = test_packet("Trace the animation structure.", 96 * 1024).answer;
        answer.citations.clear();
        answer.graphs = vec![artifact];

        let surviving = |answer: &AgentAnswerDto| {
            let mut capped = answer.clone();
            let protected = protected_graph_edge_ids_for_budget(&capped, &[]);
            cap_graph_edges(&mut capped, 1, &protected);
            capped
                .graphs
                .iter()
                .filter_map(|artifact| match artifact {
                    GraphArtifactDto::Uml { graph, .. } => Some(graph.edges.iter()),
                    GraphArtifactDto::Mermaid { .. } => None,
                })
                .flatten()
                .map(|edge| edge.id.0.clone())
                .collect::<Vec<_>>()
        };

        assert_eq!(
            surviving(&answer),
            vec!["aaa-unrelated".to_string()],
            "without a session the cap keeps whichever edge id sorts first"
        );

        let session = atom_session_needing(&[21]);
        let _guard = crate::agent::packet_candidate::install_packet_proof_session(
            std::rc::Rc::clone(&session),
        );
        assert_eq!(
            surviving(&answer),
            vec!["zzz-atom-required".to_string()],
            "the atom-required edge survives and the unrelated edge is dropped in its place"
        );
    }

    /// FIX B: a recorded scan's narrowed COVERAGE set is protected too — a
    /// rule-7 completeness claim is void the moment one of its enumerated
    /// edges leaves the evidence, so the cap must not be the thing that
    /// voids it.
    #[test]
    fn recorded_coverage_edges_are_protected_from_the_graph_cap() {
        let artifact = atom_hydration_artifact(
            "packet-atom-hydration-88",
            &[("aaa-unrelated", 30, 31), ("zzz-covered", 40, 41)],
        );
        let mut answer = test_packet("Trace the animation structure.", 96 * 1024).answer;
        answer.citations.clear();
        answer.graphs = vec![artifact];

        // A session that needs no identity at all, but recorded a scan whose
        // coverage claim enumerates the late-sorting edge.
        let session = atom_session_needing(&[]);
        session.record_artifact_scans(
            "packet-atom-hydration-88",
            &[crate::agent::packet_candidate::PacketCandidateTrailScan {
                root: "40".into(),
                direction: crate::agent::packet_candidate::PacketGraphDirection::Outgoing,
                depth: 2,
                edge_kinds: vec![EdgeKind::MEMBER, EdgeKind::USAGE, EdgeKind::IMPORT],
                truncated: false,
                coverage_edge_ids: vec![EdgeId("zzz-covered".into())],
            }],
        );
        let _guard = crate::agent::packet_candidate::install_packet_proof_session(
            std::rc::Rc::clone(&session),
        );

        let protected = protected_graph_edge_ids_for_budget(&answer, &[]);
        assert!(
            protected.contains(&EdgeId("zzz-covered".into())),
            "the coverage set is protected: {protected:?}"
        );
        cap_graph_edges(&mut answer, 1, &protected);
        let surviving = answer
            .graphs
            .iter()
            .filter_map(|artifact| match artifact {
                GraphArtifactDto::Uml { graph, .. } => Some(graph.edges.iter()),
                GraphArtifactDto::Mermaid { .. } => None,
            })
            .flatten()
            .map(|edge| edge.id.0.clone())
            .collect::<Vec<_>>();
        assert_eq!(surviving, vec!["zzz-covered".to_string()]);
    }

    /// Gate 9 item 1: the byte-budget trimmer removes exactly one edge, and
    /// its victim used to be whichever edge id sorted last. It now shaves a
    /// NON-atom edge first — while remaining just as trimmable, because the
    /// trimmability check still counts only obligation ids. `max_output_bytes`
    /// is a publication invariant and atom need never blocks it.
    #[ignore = "domain role/carrier taxonomy removed (phase9-r2)"]
    #[test]
    fn the_byte_budget_trimmer_shaves_a_non_atom_edge_first() {
        let build = || {
            let mut packet = test_packet("Trace the animation structure.", 96 * 1024);
            packet.answer.citations.clear();
            packet.answer.graphs = vec![atom_hydration_artifact(
                "packet-atom-hydration-101",
                &[("aaa-unrelated", 70, 71), ("zzz-atom-required", 80, 81)],
            )];
            packet
        };
        let surviving = |packet: &AgentPacketDto| {
            packet
                .answer
                .graphs
                .iter()
                .filter_map(|artifact| match artifact {
                    GraphArtifactDto::Uml { graph, .. } => Some(graph.edges.iter()),
                    GraphArtifactDto::Mermaid { .. } => None,
                })
                .flatten()
                .map(|edge| edge.id.0.clone())
                .collect::<Vec<_>>()
        };

        let mut baseline = build();
        assert!(trim_one_optional_graph_unit(&mut baseline));
        assert_eq!(
            surviving(&baseline),
            vec!["aaa-unrelated".to_string()],
            "without a session the lexicographically-last edge is the victim"
        );

        let session = atom_session_needing(&[81]);
        let _guard = crate::agent::packet_candidate::install_packet_proof_session(
            std::rc::Rc::clone(&session),
        );
        let mut atom_aware = build();
        assert!(
            trim_one_optional_graph_unit(&mut atom_aware),
            "trimmability is unchanged — one edge is still removable"
        );
        assert_eq!(
            surviving(&atom_aware),
            vec!["zzz-atom-required".to_string()],
            "the atom-required edge survives and the unrelated edge is shaved"
        );
    }

    /// Gate 9 item 1, the fallback: when EVERY edge is atom-required the
    /// trimmer still trims one. Atom need chooses the victim; it never
    /// refuses to produce one.
    #[ignore = "domain role/carrier taxonomy removed (phase9-r2)"]
    #[test]
    fn the_trimmer_still_shaves_when_every_edge_is_atom_required() {
        let session = atom_session_needing(&[91, 93]);
        let _guard = crate::agent::packet_candidate::install_packet_proof_session(
            std::rc::Rc::clone(&session),
        );
        let mut packet = test_packet("Trace the animation structure.", 96 * 1024);
        packet.answer.citations.clear();
        packet.answer.graphs = vec![atom_hydration_artifact(
            "packet-atom-hydration-102",
            &[("aaa-atom", 90, 91), ("zzz-atom", 92, 93)],
        )];
        let before = packet
            .answer
            .graphs
            .iter()
            .filter_map(|artifact| match artifact {
                GraphArtifactDto::Uml { graph, .. } => Some(graph.edges.len()),
                GraphArtifactDto::Mermaid { .. } => None,
            })
            .sum::<usize>();
        assert_eq!(before, 2);
        assert!(
            trim_one_optional_graph_unit(&mut packet),
            "an all-atom graph must still be trimmable — the byte budget cannot bend"
        );
        let after = packet
            .answer
            .graphs
            .iter()
            .filter_map(|artifact| match artifact {
                GraphArtifactDto::Uml { graph, .. } => Some(graph.edges.len()),
                GraphArtifactDto::Mermaid { .. } => None,
            })
            .sum::<usize>();
        assert_eq!(after, 1, "exactly one edge is removed, as before");
    }

    /// FIX B non-regression: an all-Legacy packet installs no promotion
    /// pattern, so the session yields no coverage sets and an empty
    /// need-set — the protection order, and therefore the cap outcome, is
    /// exactly what it was before the tier existed.
    #[test]
    fn legacy_packets_keep_their_existing_graph_cap_protection_order() {
        let artifact = atom_hydration_artifact(
            "packet-atom-hydration-99",
            &[("aaa-unrelated", 50, 51), ("zzz-other", 60, 61)],
        );
        let mut answer = test_packet("Trace the request flow.", 96 * 1024).answer;
        answer.citations.clear();
        answer.graphs = vec![artifact];
        let baseline = protected_graph_edge_ids_for_budget(&answer, &[]);

        let legacy_requirements =
            Vec::new();
        let session = std::rc::Rc::new(crate::agent::packet_candidate::PacketProofSession::new(
            crate::agent::packet_candidate::packet_atom_hydration_spec(&legacy_requirements),
        ));
        let _guard = crate::agent::packet_candidate::install_packet_proof_session(
            std::rc::Rc::clone(&session),
        );
        assert!(!session.has_atom_needed_identities());
        assert_eq!(
            protected_graph_edge_ids_for_budget(&answer, &[]),
            baseline,
            "Legacy protection is bit-identical with a session installed"
        );
    }

    #[test]
    fn final_graph_cap_preserves_overlapping_candidate_view_omissions_and_replay() {
        // View A retains {a,b} and omits {c}; view B retains {b,c} and omits {a}. The
        // unconditional final canonicalization must keep both physical `b` occurrences because
        // their opaque counts are local to different bounded views.
        let mut answer = test_packet("Trace overlapping candidate views.", 96 * 1024).answer;
        answer.graphs = vec![
            candidate_view_artifact('a', &["a", "b"], 1),
            candidate_view_artifact('b', &["b", "c"], 1),
        ];
        answer.subgraph_ids = answer
            .graphs
            .iter()
            .map(|artifact| match artifact {
                GraphArtifactDto::Uml { id, .. } | GraphArtifactDto::Mermaid { id, .. } => {
                    id.clone()
                }
            })
            .collect();

        assert!(!canonicalize_packet_graphs_and_references(&mut answer));
        assert!(!canonicalize_packet_graphs_and_references(&mut answer));
        assert_eq!(answer.graphs.len(), 2);
        assert_eq!(packet_budget_usage(&answer).trail_edges, 4);
        let graph_shapes = answer
            .graphs
            .iter()
            .map(|artifact| match artifact {
                GraphArtifactDto::Uml { graph, .. } => {
                    let mut ids = graph
                        .edges
                        .iter()
                        .map(|edge| edge.id.0.as_str())
                        .collect::<Vec<_>>();
                    ids.sort_unstable();
                    (ids, graph.truncated, graph.omitted_edge_count)
                }
                GraphArtifactDto::Mermaid { .. } => unreachable!(),
            })
            .collect::<Vec<_>>();
        assert_eq!(graph_shapes[0], (vec!["a", "b"], true, 1));
        assert_eq!(graph_shapes[1], (vec!["b", "c"], true, 1));

        let mut capped = answer.clone();
        assert!(cap_graph_edges(&mut capped, 3, &[]));
        let first_pass = capped.clone();
        assert!(!cap_graph_edges(&mut capped, 3, &[]));
        assert_eq!(
            serde_json::to_value(&capped).unwrap(),
            serde_json::to_value(&first_pass).unwrap()
        );
        assert_eq!(packet_budget_usage(&capped).trail_edges, 3);
        let local_counts = capped
            .graphs
            .iter()
            .map(|artifact| match artifact {
                GraphArtifactDto::Uml { graph, .. } => {
                    (graph.edges.len(), graph.truncated, graph.omitted_edge_count)
                }
                GraphArtifactDto::Mermaid { .. } => unreachable!(),
            })
            .collect::<Vec<_>>();
        assert_eq!(local_counts, [(2, true, 1), (1, true, 2)]);
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
    fn graph_cap_counts_duplicate_protected_edges_once_and_prunes_stale_references() {
        let mut packet = test_packet("Trace one protected edge.", 96 * 1024);
        let protected = EdgeId("shared-proof".to_string());
        packet.answer.graphs = vec![
            budget_graph_artifact("z-graph", &["shared-proof", "ordinary-z"]),
            budget_graph_artifact("a-graph", &["shared-proof", "ordinary-a"]),
        ];
        packet.answer.subgraph_ids = vec![
            "z-graph".to_string(),
            "a-graph".to_string(),
            "missing-graph".to_string(),
            "z-graph".to_string(),
        ];
        packet.answer.citations[0].subgraph_id = Some("missing-graph".to_string());
        packet.answer.citations[0].evidence_edge_ids = vec![
            protected.clone(),
            EdgeId("missing-edge".to_string()),
            protected.clone(),
        ];
        packet.answer.sections.push(AgentResponseSectionDto {
            id: "stale-diagram".to_string(),
            title: "Stale diagram".to_string(),
            blocks: vec![AgentResponseBlockDto::Mermaid {
                graph_id: "missing-graph".to_string(),
            }],
        });

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
        assert_eq!(
            retained.iter().filter(|edge| **edge == protected).count(),
            1
        );
        assert_eq!(packet_budget_usage(&packet.answer).trail_edges, 2);
        assert_eq!(
            packet.answer.citations[0].evidence_edge_ids,
            vec![protected]
        );
        assert_eq!(packet.answer.citations[0].subgraph_id, None);
        assert_eq!(packet.answer.subgraph_ids, vec!["a-graph".to_string()]);
        assert!(
            packet
                .answer
                .sections
                .last()
                .is_some_and(|section| section.blocks.is_empty()),
            "stale diagram blocks must not reference a removed graph"
        );
        assert!(packet.answer.graphs.iter().all(|artifact| match artifact {
            GraphArtifactDto::Uml { graph, .. } => !graph.edges.is_empty(),
            GraphArtifactDto::Mermaid { .. } => true,
        }));
    }

    #[test]
    fn graph_cap_keeps_cited_incident_calls_ahead_of_unrelated_trail() {
        let mut packet = test_packet("Trace a cited CALL.", 96 * 1024);
        let cited = packet.answer.citations[0].node_id.clone();
        let neighbor = NodeId("cited-call-target".to_string());
        let cited_edge = EdgeId("cited-call".to_string());
        packet.answer.graphs = vec![GraphArtifactDto::Uml {
            id: "cited-graph".to_string(),
            title: "Cited graph".to_string(),
            graph: GraphResponse {
                center_id: cited.clone(),
                nodes: vec![
                    budget_graph_node(&cited.0),
                    GraphNodeDto {
                        id: neighbor.clone(),
                        label: "handle".to_string(),
                        kind: NodeKind::METHOD,
                        depth: 1,
                        label_policy: None,
                        badge_visible_members: None,
                        badge_total_members: None,
                        merged_symbol_examples: Vec::new(),
                        file_path: Some("src/lib.rs".to_string()),
                        qualified_name: None,
                        member_access: None,
                    },
                ],
                edges: vec![GraphEdgeDto {
                    id: cited_edge.clone(),
                    source: cited,
                    target: neighbor,
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
        packet.answer.graphs.push(budget_graph_artifact(
            "noise-graph",
            &[
                "noise-a", "noise-b", "noise-c", "noise-d", "noise-e", "noise-f",
            ],
        ));

        let protected = protected_graph_edge_ids_for_budget(&packet.answer, &[]);
        assert!(
            protected.contains(&cited_edge),
            "cited incident CALL must be protected: {protected:?}"
        );
        assert!(cap_graph_edges(&mut packet.answer, 2, &protected));
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
        assert!(
            retained.contains(&cited_edge),
            "compact cap must keep the cited CALL: {retained:?}"
        );
    }

    #[test]
    fn graph_cap_selection_is_invariant_to_artifact_insertion_order() {
        let protected = EdgeId("shared-proof".to_string());
        let mut first = test_packet("Trace stable graph selection.", 96 * 1024).answer;
        first.graphs = vec![
            budget_graph_artifact("z-graph", &["ordinary-z", "shared-proof"]),
            budget_graph_artifact("a-graph", &["ordinary-a", "shared-proof"]),
        ];
        let mut second = first.clone();
        second.graphs.reverse();

        let selected_ids = |answer: &AgentAnswerDto| {
            let mut ids = answer
                .graphs
                .iter()
                .filter_map(|artifact| match artifact {
                    GraphArtifactDto::Uml { graph, .. } => Some(graph.edges.iter()),
                    GraphArtifactDto::Mermaid { .. } => None,
                })
                .flatten()
                .map(|edge| edge.id.0.clone())
                .collect::<Vec<_>>();
            ids.sort();
            ids
        };

        assert!(cap_graph_edges(
            &mut first,
            2,
            std::slice::from_ref(&protected),
        ));
        assert!(cap_graph_edges(
            &mut second,
            2,
            std::slice::from_ref(&protected),
        ));
        assert_eq!(selected_ids(&first), selected_ids(&second));
        assert_eq!(selected_ids(&first), vec!["ordinary-a", "shared-proof"]);
    }

    #[test]
    fn zero_graph_cap_retains_no_protected_or_ordinary_edge() {
        let protected = EdgeId("protected".to_string());
        let mut answer = test_packet("Drop every graph edge.", 96 * 1024).answer;
        answer.graphs = vec![budget_graph_artifact("graph", &["protected", "ordinary"])];

        assert!(cap_graph_edges(
            &mut answer,
            0,
            std::slice::from_ref(&protected),
        ));
        assert!(answer.graphs.is_empty());
        assert_eq!(packet_budget_usage(&answer).trail_edges, 0);
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
                "structural_source_citation",
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
        packet.disposition = PacketDispositionDto::supported();
        packet.disposition.omission_receipts.clear();

        enforce_packet_output_budget(project_root, &mut packet);

        assert_eq!(
            packet.disposition.kind,
            PacketDispositionKindDto::Supported,
            "budget must not reclassify a compiled disposition: {:?}",
            packet.disposition
        );
        crate::agent::packet_compiler::apply_compiled_evidence(&mut packet, None);
        assert_eq!(
            packet.disposition.kind,
            PacketDispositionKindDto::DrillOnce,
            "unread requested path must be closable by one drill, not auto-Supported: {:?}",
            packet.disposition
        );
        assert!(
            packet
                .disposition
                .drill
                .as_ref()
                .map(|drill| drill
                    .options
                    .iter()
                    .any(|option| option.path.as_deref() == Some(runtime_path)))
                .unwrap_or(false),
            "the compiled drill should name the uncovered requested path: {:?}",
            packet.disposition
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
            &[],
            &PacketProofEvidenceExtras::default(),
        );
        let supported_claims_with_telemetry =
            packet_supported_claims_with_telemetry(&packet.answer);
        let mut initial_claims = packet_claims_with_obligation_receipts(
            &packet.answer,
            packet.plan.task_class,
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
            &[],
            &PacketProofEvidenceExtras::default(),
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
        assert!(refreshed.len() <= initial_markdown.len());
    }

    #[test]
    fn compact_budget_trims_optional_trace_diagnostics_before_hard_payload_omission() {
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
        assert_eq!(packet.retrieval_trace_summary.search_steps, 0);
        assert_eq!(packet.retrieval_trace_summary.trail_steps, 0);
        assert_eq!(packet.retrieval_trace_summary.source_read_steps, 0);
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
        assert!(packet.answer.retrieval_trace.steps.is_empty());
        assert!(
            packet
                .budget
                .omitted_sections
                .contains(&ANSWER_RETRIEVAL_DIAGNOSTICS_OMISSION.to_string())
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

        let trace_path = std::env::temp_dir().join(format!(
            "codestory-over-cap-packet-step-trace-{}.json",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&trace_path);
        std::fs::write(
            &trace_path,
            serde_json::to_string_pretty(&packet_step_trace_json(&packet.answer))
                .expect("serialize exported packet step trace"),
        )
        .expect("write exported packet step trace");
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
        )
        .expect("represented packet should converge");

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
        )
        .expect("represented packet should converge");

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
    fn impossible_adapter_cap_returns_a_typed_error_instead_of_an_oversized_packet() {
        let question = "Explain still oversized packet diagnostics.";
        let mut packet = test_packet(question, 512);
        install_duplicate_summary_trace_payload(&mut packet, 24);

        let error = enforce_packet_output_budget_for_representation(
            test_project_root(),
            &mut packet,
            serialized_packet_len,
        )
        .expect_err("a 512-byte envelope cannot carry the mandatory typed packet");

        let serialized_len = serialized_packet_len(&packet);
        assert!(
            serialized_len > packet.budget.limits.max_output_bytes as usize,
            "fixture should remain over an impossible cap after diagnostic trimming"
        );
        assert_eq!(error.code, "packet_output_budget_exceeded");
        assert!(error.message.contains("mandatory envelope"));
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
    fn supported_hard_cap_converges_to_a_partial_packet_naming_exact_omissions() {
        let mut packet = test_packet("Explain the exact request dispatch proof.", 24 * 1024);
        packet.answer.summary = "untrimmable summary ".repeat(20_000);
        packet.plan.obligations.claim_obligations = vec![PacketClaimObligationDto {
            id: "request_dispatch".to_string(),
            kind: PacketClaimObligationKindDto::Dispatch,
            binding_terms: vec!["request dispatch".to_string()],
            probe_binding: None,
            material: true,
            allowed_node_kinds: vec![NodeKind::METHOD],
            required_edge_kind: Some(EdgeKind::CALL),
            requires_complete_discovery: false,
            proof_status: PacketObligationProofStatusDto::Proven,
            reason: None,
            carrier_node_ids: vec![NodeId("Session.send".to_string())],
            carrier_paths: vec!["src/sessions.rs".to_string()],
            carrier_edge_proofs: vec![PacketObligationCarrierEdgeProofDto {
                carrier_node_id: NodeId("Session.send".to_string()),
                edge_id: EdgeId("request-send".to_string()),
                edge_kind: EdgeKind::CALL,
            }],
            open_next_candidates: vec!["Session.send".to_string()],
        }];
        packet.plan.obligations.query_obligations = vec![PacketQueryObligationDto {
            id: "query:dispatch".to_string(),
            kind: PacketQueryObligationKindDto::RequiredFlow,
            query: "request dispatch".to_string(),
            material: true,
            completion: Some(PacketQueryCompletionDto::Cancelled {
                reason: "stage_deadline".to_string(),
            }),
        }];

        enforce_packet_output_budget_for_representation(
            test_project_root(),
            &mut packet,
            serialized_packet_len,
        )
        .expect("the supported tiny envelope must converge");

        let final_len = serialized_packet_len(&packet);
        assert!(final_len <= 24 * 1024, "{final_len} > 24576");
        assert_eq!(packet.budget.used.output_bytes as usize, final_len);
        assert_eq!(
            packet.disposition.kind,
            PacketDispositionKindDto::Supported,
            "hard-budget minimizer must not reclassify disposition: {:?}",
            packet.disposition
        );
        assert!(packet.disposition.omission_receipts.iter().any(|gap| {
            gap.contains("request_dispatch") && gap.contains("packet_budget_truncated")
        }));
        assert!(
            packet
                .disposition
                .omission_receipts
                .iter()
                .any(|gap| { gap.contains("query:dispatch") && gap.contains("stage_deadline") })
        );
        assert_eq!(
            packet.plan.obligations.claim_obligations[0].proof_status,
            PacketObligationProofStatusDto::Reported
        );
        assert!(!packet.answer.citations.is_empty());

        let converged = serde_json::to_vec(&packet).expect("serialize converged packet");
        enforce_packet_output_budget_for_representation(
            test_project_root(),
            &mut packet,
            serialized_packet_len,
        )
        .expect("repeated enforcement should preserve the fixpoint");
        assert_eq!(
            serde_json::to_vec(&packet).expect("serialize repeated packet"),
            converged
        );
    }

    #[test]
    fn compact_budget_trims_plan_trace_before_payload_omission() {
        let question = "Explain symbol ownership for PacketBudget.";
        let mut packet = test_packet(question, 1);
        packet.plan.trace = (0..48)
            .map(|index| format!("diagnostic claim {index} {}", "padding ".repeat(80)))
            .collect();

        let mut trimmed_probe = packet.clone();
        let trimmed_sections = trim_packet_sufficiency_verbose_lists(&mut trimmed_probe);
        assert_eq!(trimmed_sections, vec!["plan.trace", "plan.queries"]);
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
            "trimming plan traces should bring the packet under cap: {serialized_len} > {max_output_bytes}"
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
        assert!(packet.plan.trace.is_empty());
        assert_eq!(packet.disposition.kind, PacketDispositionKindDto::Supported);
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
        )
        .expect("retained proof packet should converge");

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
        )
        .expect("marker-present packet should converge");

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
        )
        .expect("repeated marker-present packet should converge");
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
        )
        .expect("pretty packet should converge");

        let rendered_len = serde_json::to_vec_pretty(&packet)
            .expect("serialize budgeted represented packet")
            .len()
            + 1;
        assert!(rendered_len <= public_cap, "{rendered_len} > {public_cap}");
        assert_eq!(packet.budget.used.output_bytes as usize, rendered_len);
    }

    #[test]
    fn verbose_plan_trimming_clears_trace_and_queries_without_changing_disposition() {
        let mut packet = test_packet("Explain route dispatch gaps.", 4096);
        packet.plan.trace = vec!["planner trace".to_string()];
        packet.plan.queries = vec![PacketPlanQueryDto {
            query: "route dispatch".to_string(),
            purpose: "fixture".to_string(),
        }];
        let kind = packet.disposition.kind;

        let trimmed_sections = trim_packet_sufficiency_verbose_lists(&mut packet);

        assert_eq!(trimmed_sections, vec!["plan.trace", "plan.queries"]);
        assert!(packet.plan.trace.is_empty());
        assert!(packet.plan.queries.is_empty());
        assert_eq!(packet.disposition.kind, kind);
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
            support: Vec::new(),
            disposition: PacketDispositionDto::supported(),
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
            source_excerpt: None,
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

    // -----------------------------------------------------------------------
    // Stage 2: byte-budget rebuild agreement for typed-proof obligations
    // -----------------------------------------------------------------------

    pub(in crate::agent) const MAPPER_PROOF_QUESTION: &str =
        "How does the mapper build its configuration and execution plan?";

    fn eligible_proof_citation(name: &str, path: &str, kind: NodeKind) -> AgentCitationDto {
        AgentCitationDto {
            node_id: NodeId(name.to_string()),
            display_name: name.to_string(),
            kind,
            file_path: Some(path.to_string()),
            line: Some(10),
            score: 0.9,
            origin: SearchHitOrigin::IndexedSymbol,
            target: None,
            resolvable: true,
            subgraph_id: None,
            evidence_edge_ids: Vec::new(),
            retrieval_score_breakdown: None,
            evidence_tier: Some(PacketEvidenceTierDto::ResolvedGraph),
            evidence_producer: Some("test".to_string()),
            resolution_status: Some(PacketEvidenceResolutionDto::Resolved),
            loss_reason: None,
            coverage_role: None,
            eligible_for_sufficiency: Some(true),
            source_excerpt: None,
        }
    }

    fn mapper_proof_graph_node(citation: &AgentCitationDto) -> GraphNodeDto {
        GraphNodeDto {
            id: citation.node_id.clone(),
            label: citation.display_name.clone(),
            kind: citation.kind,
            depth: 1,
            label_policy: None,
            badge_visible_members: None,
            badge_total_members: None,
            merged_symbol_examples: Vec::new(),
            file_path: citation.file_path.clone(),
            qualified_name: None,
            member_access: None,
        }
    }

    /// A packet whose live state proves `mapper_config` through the atom
    /// matcher: a certain TYPE_USAGE receipt (A1) and the builder's own
    /// MEMBER-onto-METHOD receipt (A5) in `answer.graphs`, plus a reread
    /// source range for the configuration type (A2) in `packet.support`,
    /// owned by a retained sufficiency-eligible citation.
    pub(in crate::agent) fn mapper_proof_packet() -> AgentPacketDto {
        let builder =
            eligible_proof_citation("PlanBuilder", "src/plan_builder.cs", NodeKind::CLASS);
        let config = eligible_proof_citation(
            "MapperConfiguration",
            "src/mapper_configuration.cs",
            NodeKind::CLASS,
        );
        let mut packet = test_packet(MAPPER_PROOF_QUESTION, 98_304);
        packet.task_class = Some(PacketTaskClassDto::ArchitectureExplanation);
        packet.plan.task_class = PacketTaskClassDto::ArchitectureExplanation;
        packet.plan.obligations = build_packet_obligation_plan(
            MAPPER_PROOF_QUESTION,
            PacketTaskClassDto::ArchitectureExplanation,
            &[],
        );
        packet.answer.citations = vec![builder.clone(), config.clone()];
        let mut builder_method = mapper_proof_graph_node(&builder);
        builder_method.id = NodeId("PlanBuilder.Build".to_string());
        builder_method.label = "Build".to_string();
        builder_method.kind = NodeKind::METHOD;
        packet.answer.graphs = vec![GraphArtifactDto::Uml {
            id: "mapper-plan".to_string(),
            title: "Mapper plan".to_string(),
            graph: GraphResponse {
                center_id: builder.node_id.clone(),
                nodes: vec![
                    mapper_proof_graph_node(&builder),
                    mapper_proof_graph_node(&config),
                    builder_method.clone(),
                ],
                edges: vec![
                    GraphEdgeDto {
                        id: EdgeId("builder-uses-config".to_string()),
                        source: builder.node_id.clone(),
                        target: config.node_id.clone(),
                        kind: EdgeKind::TYPE_USAGE,
                        confidence: Some(1.0),
                        certainty: Some("certain".to_string()),
                        callsite_identity: None,
                        candidate_targets: Vec::new(),
                    },
                    GraphEdgeDto {
                        id: EdgeId("builder-owns-method".to_string()),
                        source: builder.node_id.clone(),
                        target: builder_method.id.clone(),
                        kind: EdgeKind::MEMBER,
                        confidence: Some(1.0),
                        certainty: None,
                        callsite_identity: None,
                        candidate_targets: Vec::new(),
                    },
                ],
                truncated: false,
                omitted_edge_count: 0,
                canonical_layout: None,
            },
        }];
        packet.support = mapper_proof_raw_source_support();
        packet
    }

    /// The reread source units exactly as the orchestrator holds them in its
    /// `source_support` local — constructed independently of any packet, so
    /// the agreement test below exercises the production asymmetry (raw units
    /// at site A, `packet.support` at site B), not one vector compared with
    /// itself.
    fn mapper_proof_raw_source_support() -> Vec<codestory_contracts::api::SupportUnitDto> {
        vec![codestory_contracts::api::SupportUnitDto {
            id: "range:MapperConfiguration".to_string(),
            kind: codestory_contracts::api::SupportUnitKindDto::SourceRange,
            summary: "MapperConfiguration source".to_string(),
            path: Some("src/mapper_configuration.cs".to_string()),
            symbol_id: Some("MapperConfiguration".to_string()),
            start_line: Some(10),
            end_line: Some(30),
            snippet: Some("public class MapperConfiguration {}".to_string()),
            edge_kind: None,
            from_symbol: None,
            to_symbol: None,
            query: None,
        }]
    }

    /// The round-1 review's central hazard, restated for R7: the primary
    /// finalize (site A, WITH extras — the one proving site) and the budget
    /// rebuild (site B, receipt SURVIVAL — never re-proves) must agree on
    /// `proof_status` when every recorded receipt is present at rebuild time.
    ///
    /// The two sites' support inputs are genuinely different expressions in
    /// production: the orchestrator feeds its raw `source_support` local
    /// (which only later becomes `packet.support` — the move happens before
    /// compile rewrites it), while the rebuild feeds the current
    /// `packet.support`. Site A therefore gets independently constructed raw
    /// units here, not `packet.support` itself.
    #[ignore = "domain role/carrier taxonomy removed (phase9-r2)"]
    #[test]
    fn primary_finalize_and_budget_rebuild_survival_agree_on_proof_status() {
        let mut packet = mapper_proof_packet();
        let raw_source_support = mapper_proof_raw_source_support();
        assert_eq!(
            raw_source_support, packet.support,
            "fixture invariant: the packet carries exactly the raw reread units"
        );

        // Site A: the primary orchestrator-style finalize over the raw units,
        // with the (empty-default) proving extras. This is the state the
        // packet ships with.
        finalize_packet_obligation_plan(
            &packet.question,
            packet.plan.task_class,
            &mut packet.plan.obligations,
            &packet.answer,
            &packet.budget,
            &raw_source_support,
            &PacketProofEvidenceExtras::default(),
        );
        let primary_plan = packet.plan.obligations.clone();
        // The agreement must bite on a formula-proven obligation, not only on
        // trivially unproven ones — and the manifest must be recorded.
        let proven = primary_plan
            .claim_obligations
            .iter()
            .find(|obligation| obligation.id == "mapper_config")
            .expect("mapper_config obligation");
        assert_eq!(
            proven.proof_status,
            PacketObligationProofStatusDto::Proven,
            "fixture must prove mapper_config through receipts: {proven:?}"
        );
        assert!(
            !proven.carrier_edge_proofs.is_empty(),
            "the primary finalize must record the edge-receipt manifest: {proven:?}"
        );

        // Site B: the byte-budget rebuild — receipt survival. Every recorded
        // receipt is present, so every proof_status is retained and the two
        // sites agree.
        let mut rebuilt = packet.clone();
        rebuild_packet_budget_dependents(test_project_root(), &mut rebuilt, &[]);
        let statuses = |plan: &codestory_contracts::api::PacketObligationPlanDto| {
            plan.claim_obligations
                .iter()
                .map(|obligation| (obligation.id.clone(), obligation.proof_status))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            statuses(&primary_plan),
            statuses(&rebuilt.plan.obligations),
            "survival must retain every status when all recorded receipts survive"
        );

        // Removing a recorded edge receipt from the live graphs demotes at the
        // next rebuild — fail-closed, with the rebuild reason (distinguishable
        // from the R5 compile reason).
        let recorded_edge_id = proven.carrier_edge_proofs[0].edge_id.clone();
        let mut cut = packet.clone();
        for artifact in &mut cut.answer.graphs {
            if let GraphArtifactDto::Uml { graph, .. } = artifact {
                graph.edges.retain(|edge| edge.id != recorded_edge_id);
            }
        }
        rebuild_packet_budget_dependents(test_project_root(), &mut cut, &[]);
        let demoted = cut
            .plan
            .obligations
            .claim_obligations
            .iter()
            .find(|obligation| obligation.id == "mapper_config")
            .expect("mapper_config obligation");
        assert_eq!(
            demoted.proof_status,
            PacketObligationProofStatusDto::Unsupported,
            "{demoted:?}"
        );
        assert_eq!(
            demoted.reason.as_deref(),
            Some("flow_proof_receipts_missing_after_rebuild")
        );
    }

    /// R2 protection widening (landed together with the compiler allow-list
    /// widening): the graph-cap protection buckets consider every
    /// atom-required edge kind, so cited TYPE_USAGE/USAGE/MEMBER/IMPORT
    /// receipts outrank unrelated CALL context that sorts earlier by id.
    #[test]
    fn widened_atom_kind_edges_with_cited_endpoints_survive_the_graph_cap() {
        let mut packet = test_packet("Trace the widened protection.", 96 * 1024);
        let builder =
            eligible_proof_citation("PlanBuilder", "src/plan_builder.cs", NodeKind::CLASS);
        let config = eligible_proof_citation(
            "MapperConfiguration",
            "src/mapper_configuration.cs",
            NodeKind::CLASS,
        );
        packet.answer.citations = vec![builder.clone(), config.clone()];
        let structural_kinds = [
            ("zz-type-usage", EdgeKind::TYPE_USAGE),
            ("zz-usage", EdgeKind::USAGE),
            ("zz-member", EdgeKind::MEMBER),
            ("zz-import", EdgeKind::IMPORT),
        ];
        let mut nodes = vec![
            mapper_proof_graph_node(&builder),
            mapper_proof_graph_node(&config),
            budget_graph_node("uncited-center"),
        ];
        let mut edges = Vec::new();
        for index in 0..4 {
            let target = format!("uncited-target-{index}");
            nodes.push(budget_graph_node(&target));
            edges.push(GraphEdgeDto {
                id: EdgeId(format!("aa-context-{index}")),
                source: NodeId("uncited-center".to_string()),
                target: NodeId(target),
                kind: EdgeKind::CALL,
                confidence: Some(1.0),
                certainty: Some("certain".to_string()),
                callsite_identity: None,
                candidate_targets: Vec::new(),
            });
        }
        for (edge_id, kind) in structural_kinds {
            edges.push(GraphEdgeDto {
                id: EdgeId(edge_id.to_string()),
                source: builder.node_id.clone(),
                target: config.node_id.clone(),
                kind,
                confidence: None,
                certainty: None,
                callsite_identity: None,
                candidate_targets: Vec::new(),
            });
        }
        packet.answer.graphs = vec![GraphArtifactDto::Uml {
            id: "widened".to_string(),
            title: "Widened".to_string(),
            graph: GraphResponse {
                center_id: builder.node_id.clone(),
                nodes,
                edges,
                truncated: false,
                omitted_edge_count: 0,
                canonical_layout: None,
            },
        }];

        let protected = protected_graph_edge_ids_for_budget(&packet.answer, &[]);
        for (edge_id, _) in structural_kinds {
            assert!(
                protected.contains(&EdgeId(edge_id.to_string())),
                "{edge_id} must be protected: {protected:?}"
            );
        }
        assert!(cap_graph_edges(&mut packet.answer, 4, &protected));
        let retained = packet
            .answer
            .graphs
            .iter()
            .filter_map(|artifact| match artifact {
                GraphArtifactDto::Uml { graph, .. } => Some(graph.edges.iter()),
                GraphArtifactDto::Mermaid { .. } => None,
            })
            .flatten()
            .map(|edge| edge.id.0.clone())
            .collect::<Vec<_>>();
        assert_eq!(retained.len(), 4);
        for (edge_id, _) in structural_kinds {
            assert!(
                retained.contains(&edge_id.to_string()),
                "cited {edge_id} must survive over earlier-id uncited CALL context: {retained:?}"
            );
        }
    }

    /// The budget fixpoint's rebuild pass is remove-and-demote only: running
    /// it twice from the same state changes nothing (monotone idempotent
    /// steps are what the loop-convergence argument and the marker-shape
    /// revert rely on), and no evidence collection ever grows.
    #[test]
    fn budget_rebuild_dependents_pass_is_remove_only_and_idempotent() {
        let mut packet = mapper_proof_packet();
        finalize_packet_obligation_plan(
            &packet.question,
            packet.plan.task_class,
            &mut packet.plan.obligations,
            &packet.answer,
            &packet.budget,
            &packet.support.clone(),
            &PacketProofEvidenceExtras::default(),
        );
        let citations_before = packet.answer.citations.len();
        let edges_before = packet_budget_usage(&packet.answer).trail_edges;
        let support_before = packet.support.len();

        let mut first = packet.clone();
        rebuild_packet_budget_dependents(test_project_root(), &mut first, &[]);
        assert!(first.answer.citations.len() <= citations_before);
        assert!(packet_budget_usage(&first.answer).trail_edges <= edges_before);
        assert!(first.support.len() <= support_before);

        let mut second = first.clone();
        rebuild_packet_budget_dependents(test_project_root(), &mut second, &[]);
        assert_eq!(
            serde_json::to_value(&first).expect("first rebuild"),
            serde_json::to_value(&second).expect("second rebuild"),
            "a second rebuild from the same state must be a no-op"
        );
    }
}
