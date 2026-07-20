use crate::agent::packet_capping::cap_packet_citations;
use crate::agent::packet_command_profiles::packet_command_exact_probe_queries;
use crate::agent::packet_plan::{packet_explicit_request_probe_queries, push_unique_term};
use crate::agent::packet_required_probes::packet_sufficiency_required_probe_queries_with_extra;
use crate::agent::packet_sufficiency::{
    PACKET_MARKDOWN_TRUNCATION_SUFFIX, build_packet_sufficiency_with_extra,
    quote_packet_command_value, quote_packet_project_arg,
};
use crate::agent::trace_export::packet_retrieval_trace_summary;
use codestory_contracts::api::{
    AgentAnswerDto, AgentPacketDto, AgentResponseBlockDto, AgentRetrievalStepKindDto,
    AgentRetrievalStepStatusDto, GraphArtifactDto, GraphResponse, PacketBudgetDto,
    PacketBudgetLimitsDto, PacketBudgetModeDto, PacketBudgetUsageDto, PacketTaskClassDto,
};
use std::collections::HashSet;
use std::path::Path;

const MARKDOWN_TRUNCATION_FLOOR_BYTES: usize = 256;
const AVOID_OPENING_OMISSION: &str = "avoid_opening";
const COVERAGE_REPORT_INELIGIBLE_OMISSION: &str = "coverage_report.ineligible";
const RETRIEVAL_TRACE_SUMMARY_OMISSION: &str = "retrieval_trace_summary";

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

pub(crate) fn apply_packet_budget_with_extra(
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
    for probe in
        packet_sufficiency_required_probe_queries_with_extra(question, task_class, extra_probes)
    {
        push_unique_term(&mut protected_probe_queries, &probe);
    }
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

pub(crate) fn enforce_packet_output_budget(project_root: &Path, packet: &mut AgentPacketDto) {
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

        let trimmed_verbose_sections = trim_packet_sufficiency_verbose_lists(packet);
        if !trimmed_verbose_sections.is_empty() {
            for section in trimmed_verbose_sections {
                push_omitted_section(&mut packet.budget, section);
            }
            continue;
        }

        if trim_packet_retrieval_trace_summary(packet) {
            push_omitted_section(&mut packet.budget, RETRIEVAL_TRACE_SUMMARY_OMISSION);
            continue;
        }

        if truncate_answer_markdown_to_byte_cap(&mut packet.answer, next_answer_cap) {
            push_omitted_section(&mut packet.budget, "markdown_blocks");
            packet.budget.used = packet_budget_usage(&packet.answer);
            rebuild_packet_budget_dependents(project_root, packet, &extra_probes);
            continue;
        }
        break;
    }

    let output_bytes = refresh_packet_output_bytes(packet);
    if output_bytes > packet.budget.limits.max_output_bytes as usize {
        packet.budget.truncated = true;
        push_omitted_section(&mut packet.budget, "output_bytes");
        push_omitted_section(&mut packet.budget, "packet_payload");
        rebuild_packet_budget_dependents(project_root, packet, &extra_probes);
        let _ = refresh_packet_output_bytes(packet);
    } else {
        remove_omitted_section(&mut packet.budget, "output_bytes");
        remove_omitted_section(&mut packet.budget, "packet_payload");
        rebuild_packet_budget_dependents(project_root, packet, &extra_probes);
        let _ = refresh_packet_output_bytes(packet);
    }
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
    let trimmed = !trace.request_id.is_empty()
        || trace.total_latency_ms != 0
        || trace.sla_target_ms.is_some()
        || trace.sla_missed
        || trace.semantic_fallback_count != 0
        || !trace.semantic_fallbacks.is_empty()
        || !trace.annotations.is_empty()
        || !trace.steps.is_empty()
        || !trace.packet_sidecar_diagnostics.is_empty()
        || trace.retrieval_shadow.is_some();

    if trimmed {
        trace.request_id.clear();
        trace.total_latency_ms = 0;
        trace.sla_target_ms = None;
        trace.sla_missed = false;
        trace.semantic_fallback_count = 0;
        trace.semantic_fallbacks.clear();
        trace.annotations.clear();
        trace.steps.clear();
        trace.packet_sidecar_diagnostics.clear();
        trace.retrieval_shadow = None;
    }

    trimmed
}

fn rebuild_packet_budget_dependents(
    project_root: &Path,
    packet: &mut AgentPacketDto,
    extra_probes: &[String],
) {
    packet.retrieval_trace_summary = packet_retrieval_trace_summary(&packet.answer);
    packet.sufficiency = build_packet_sufficiency_with_extra(
        project_root,
        &packet.question,
        packet
            .task_class
            .unwrap_or(PacketTaskClassDto::ArchitectureExplanation),
        &packet.answer,
        &packet.budget,
        extra_probes,
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

pub(crate) fn next_deeper_packet_command(
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

#[cfg(test)]
mod tests {
    use super::*;
    use codestory_contracts::api::{
        AgentCitationDto, AgentResponseSectionDto, AgentRetrievalPolicyModeDto,
        AgentRetrievalPresetDto, AgentRetrievalStepDto, AgentRetrievalTraceDto, NodeId, NodeKind,
        PacketClaimDto, PacketCoverageReportDto, PacketPlanDto, PacketPlanQueryDto,
        PacketRetrievalTraceSummaryDto, PacketSufficiencyDto, PacketSufficiencyStatusDto,
        SearchHitOrigin,
    };

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
                .any(|annotation| annotation.contains("canonical trace annotation"))
        );
    }

    #[test]
    fn compact_budget_keeps_hard_payload_omission_when_summary_trace_trim_is_not_enough() {
        let question = "Explain still oversized packet diagnostics.";
        let mut packet = test_packet(question, 512);
        install_duplicate_summary_trace_payload(&mut packet, 24);

        enforce_packet_output_budget(test_project_root(), &mut packet);

        let serialized_len = serialized_packet_len(&packet);
        assert!(
            serialized_len > packet.budget.limits.max_output_bytes as usize,
            "fixture should remain over cap after summary trace trimming"
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
        assert_eq!(packet.retrieval_trace_summary.search_steps, 1);
        assert_eq!(packet.retrieval_trace_summary.trail_steps, 1);
        assert_eq!(packet.retrieval_trace_summary.source_read_steps, 1);
        assert!(
            packet
                .retrieval_trace_summary
                .retrieval_trace
                .steps
                .is_empty()
        );
        assert_eq!(packet.answer.retrieval_trace.steps.len(), 3);
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

    fn test_packet(question: &str, max_output_bytes: u32) -> AgentPacketDto {
        let answer = AgentAnswerDto {
            answer_id: "packet-budget-test".to_string(),
            prompt: question.to_string(),
            summary: "Packet budget test answer.".to_string(),
            freshness: None,
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
                annotations: Vec::new(),
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

    fn install_duplicate_summary_trace_payload(packet: &mut AgentPacketDto, repeat: usize) {
        packet.answer.retrieval_trace.request_id = "canonical-answer-trace".to_string();
        packet.answer.retrieval_trace.total_latency_ms = 123;
        packet.answer.retrieval_trace.sla_target_ms = Some(1_000);
        packet.answer.retrieval_trace.sla_missed = true;
        packet.answer.retrieval_trace.annotations = vec![format!(
            "canonical trace annotation {}",
            "answer-retained ".repeat(repeat)
        )];
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

    fn test_project_root() -> &'static Path {
        Path::new("C:/workspace/project root")
    }
}
