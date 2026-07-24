use super::super::artifacts::{ensure_dot_only_for_trail, preflight_output_file};
use super::super::lifecycle::{OpenedAgentSurface, open_agent_surface};
use crate::args;
use crate::args::PacketCommand;
use crate::output::{
    REPO_CONTENT_BOUNDARY_LINE, RenderedPublicOutput, emit_public_operation, render_agent_citation,
    render_context_markdown,
};
use crate::runtime;
use crate::runtime::map_api_error;
use anyhow::Result;
use codestory_contracts::api::{
    AgentPacketDto, AgentPacketRequestDto, PacketBudgetModeDto, PacketSufficiencyStatusDto,
    PacketTaskClassDto,
};
use std::fmt::Write as _;

pub(in crate::app) fn run_packet(cmd: PacketCommand) -> Result<()> {
    ensure_dot_only_for_trail(cmd.format, "packet")?;
    preflight_output_file(cmd.output_file.as_deref())?;
    args::validate_packet_probe_arguments(&cmd.probes, &cmd.extra_probes)
        .map_err(anyhow::Error::msg)?;
    let OpenedAgentSurface { runtime, .. } = open_agent_surface(
        &cmd.project,
        cmd.profile,
        cmd.run_id.as_deref(),
        cmd.refresh,
        "packet",
    )?;

    let operation = runtime.run_public_operation("packet", || {
        let packet = runtime
            .browser
            .packet(AgentPacketRequestDto {
                question: cmd.question.clone(),
                budget: cmd.budget.into(),
                task_class: cmd.task_class.map(Into::into),
                probes: cmd.probes.clone(),
                extra_probes: cmd.extra_probes.clone(),
                include_evidence: !cmd.no_evidence,
                latency_budget_ms: cmd.latency_budget_ms,
            })
            .map_err(map_api_error)?;
        let step_trace = if cmd.step_trace_out.is_some() {
            let trace = codestory_runtime::packet_step_trace_json(&packet.answer);
            Some(serde_json::to_string_pretty(&trace)?)
        } else {
            None
        };
        let markdown = render_packet_markdown(&runtime.project_root, &packet);
        Ok((
            RenderedPublicOutput::structured(&packet, markdown)?,
            step_trace,
        ))
    })?;
    if let (Some(path), Some(trace)) = (&cmd.step_trace_out, &operation.value.1) {
        std::fs::write(path, trace)?;
    }
    let operation = runtime::map_public_operation(operation, |(rendered, _)| rendered);
    emit_public_operation(cmd.format, operation, cmd.output_file.as_deref())
}

fn render_packet_markdown(project_root: &std::path::Path, packet: &AgentPacketDto) -> String {
    let mut markdown = String::new();
    let _ = writeln!(markdown, "# Packet");
    append_packet_operator_header(&mut markdown, packet);
    let _ = writeln!(
        markdown,
        "question: `{}`",
        packet.question.replace('\n', " ")
    );
    let _ = writeln!(
        markdown,
        "budget: `{}`",
        packet_budget_mode_label(packet.budget.requested)
    );
    let _ = writeln!(
        markdown,
        "task_class: `{}`",
        packet_task_class_label(packet.plan.task_class)
    );
    let _ = writeln!(
        markdown,
        "sufficiency: `{}`",
        packet_sufficiency_label(packet.sufficiency.status)
    );
    if packet.budget.truncated {
        let _ = writeln!(
            markdown,
            "truncated: `{}` ({})",
            packet.budget.truncated,
            packet.budget.omitted_sections.join(", ")
        );
    }

    if !packet.plan.queries.is_empty() {
        let _ = writeln!(markdown, "\n## Plan");
        for query in &packet.plan.queries {
            let _ = writeln!(markdown, "- `{}` - {}", query.query, query.purpose);
        }
    }

    if !packet.sufficiency.covered_claims.is_empty() {
        let _ = writeln!(markdown, "\n## Covered Claims");
        let _ = writeln!(markdown, "{REPO_CONTENT_BOUNDARY_LINE}");
        for claim in &packet.sufficiency.covered_claims {
            let _ = writeln!(markdown, "- {}", claim.claim);
            for citation in claim.citations.iter().take(3) {
                let _ = writeln!(
                    markdown,
                    "  - {}",
                    render_agent_citation(project_root, citation, true)
                );
            }
        }
    }

    if !packet.sufficiency.gaps.is_empty() {
        let _ = writeln!(markdown, "\n## Gaps");
        for gap in &packet.sufficiency.gaps {
            let _ = writeln!(markdown, "- {gap}");
        }
    }

    if !packet.sufficiency.follow_up_commands.is_empty() {
        let _ = writeln!(markdown, "\n## Follow Up");
        for command in &packet.sufficiency.follow_up_commands {
            let _ = writeln!(markdown, "- `{command}`");
        }
    }

    if !packet.sufficiency.avoid_opening.is_empty() {
        let _ = writeln!(markdown, "\n## Avoid Opening");
        for item in &packet.sufficiency.avoid_opening {
            let _ = writeln!(markdown, "- {item}");
        }
    }

    markdown.push('\n');
    markdown.push_str(&render_context_markdown(project_root, &packet.answer));
    markdown
}

fn append_packet_operator_header(markdown: &mut String, packet: &AgentPacketDto) {
    let _ = writeln!(markdown, "## Status");
    let _ = writeln!(
        markdown,
        "status: {}",
        packet_operator_status(packet.sufficiency.status)
    );
    let _ = writeln!(markdown, "## Trust");
    let _ = writeln!(
        markdown,
        "trust: sufficiency={} budget_truncated={} omitted_sections={}",
        packet_sufficiency_label(packet.sufficiency.status),
        packet.budget.truncated,
        packet_budget_omitted_sections(packet)
    );
    let _ = writeln!(markdown, "## Next Action");
    let _ = writeln!(
        markdown,
        "next_action: {}",
        packet_operator_next_action(packet)
    );
    let _ = writeln!(markdown, "## Proof Tier");
    let _ = writeln!(markdown, "proof_tier: packet_evidence");
}

pub(super) fn packet_operator_status(status: PacketSufficiencyStatusDto) -> &'static str {
    match status {
        PacketSufficiencyStatusDto::Sufficient => "ready",
        PacketSufficiencyStatusDto::Partial => "needs_attention",
        PacketSufficiencyStatusDto::Insufficient => "blocked",
    }
}

pub(super) fn packet_budget_omitted_sections(packet: &AgentPacketDto) -> String {
    if packet.budget.omitted_sections.is_empty() {
        "none".to_string()
    } else {
        packet.budget.omitted_sections.join(",")
    }
}

fn packet_operator_next_action(packet: &AgentPacketDto) -> &str {
    packet
        .sufficiency
        .follow_up_commands
        .first()
        .map(String::as_str)
        .unwrap_or("Inspect cited source before relying on claims not covered by packet citations.")
}

pub(super) fn packet_budget_mode_label(mode: PacketBudgetModeDto) -> &'static str {
    match mode {
        PacketBudgetModeDto::Tiny => "tiny",
        PacketBudgetModeDto::Compact => "compact",
        PacketBudgetModeDto::Standard => "standard",
        PacketBudgetModeDto::Deep => "deep",
    }
}

fn packet_task_class_label(task_class: PacketTaskClassDto) -> &'static str {
    match task_class {
        PacketTaskClassDto::ArchitectureExplanation => "architecture_explanation",
        PacketTaskClassDto::BugLocalization => "bug_localization",
        PacketTaskClassDto::ChangeImpact => "change_impact",
        PacketTaskClassDto::RouteTracing => "route_tracing",
        PacketTaskClassDto::SymbolOwnership => "symbol_ownership",
        PacketTaskClassDto::DataFlow => "data_flow",
        PacketTaskClassDto::EditPlanning => "edit_planning",
    }
}

pub(crate) fn packet_sufficiency_label(status: PacketSufficiencyStatusDto) -> &'static str {
    match status {
        PacketSufficiencyStatusDto::Sufficient => "sufficient",
        PacketSufficiencyStatusDto::Partial => "partial",
        PacketSufficiencyStatusDto::Insufficient => "blocked",
    }
}
