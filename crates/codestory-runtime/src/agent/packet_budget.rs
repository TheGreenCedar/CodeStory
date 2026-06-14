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
    } else {
        remove_omitted_section(&mut packet.budget, "output_bytes");
        remove_omitted_section(&mut packet.budget, "packet_payload");
        rebuild_packet_budget_dependents(project_root, packet, &extra_probes);
        let _ = refresh_packet_output_bytes(packet);
    }
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
