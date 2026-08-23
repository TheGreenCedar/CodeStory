use super::super::artifacts::{ensure_dot_only_for_trail, preflight_output_file};
use super::super::lifecycle::{OpenedAgentSurface, open_agent_surface};
use crate::args;
use crate::args::PacketCommand;
use crate::output::{REPO_CONTENT_BOUNDARY_LINE, RenderedPublicOutput, emit_public_operation};
#[cfg(test)]
use crate::output::{render_context_markdown, render_public_operation_json_content};
use crate::runtime;
use crate::runtime::map_api_error;
use anyhow::Result;
use codestory_contracts::api::AgentPacketRequestDto;
#[cfg(test)]
use codestory_contracts::api::{
    AgentPacketDto, PacketBudgetModeDto, PacketDispositionKindDto, PacketTaskClassDto,
};
use codestory_contracts::packet_projection_v3::{
    EvidenceAvailabilityV3Dto, PacketProjectionV3Dto, RetrievalStateV3Dto,
};
use std::fmt::Write as _;

pub(in crate::app) fn run_packet(cmd: PacketCommand) -> Result<()> {
    ensure_dot_only_for_trail(cmd.format, "packet")?;
    preflight_output_file(cmd.output_file.as_deref())?;
    preflight_output_file(cmd.diagnostics_out.as_deref())?;
    args::validate_packet_probe_arguments(&cmd.probes, &cmd.extra_probes)
        .map_err(anyhow::Error::msg)?;
    let OpenedAgentSurface { runtime, .. } = open_agent_surface(
        &cmd.project,
        cmd.profile,
        cmd.run_id.as_deref(),
        cmd.refresh,
        "packet",
    )?;

    let request = packet_request_from_command(&cmd);
    let operation = runtime.run_public_operation("packet", || {
        let packet = runtime
            .browser
            .packet(request.clone())
            .map_err(map_api_error)?;
        codestory_runtime::project_packet_v3(
            &runtime.public_operation,
            "codestory-cli",
            &request,
            &packet,
            |candidate| {
                serde_json::to_vec(candidate)
                    .map(|bytes| bytes.len())
                    .map_err(|_| ())
            },
        )
        .map_err(map_api_error)
    })?;
    if let Some(path) = cmd.diagnostics_out.as_deref() {
        if std::fs::symlink_metadata(path).is_ok() {
            anyhow::bail!(
                "packet diagnostics destination already exists: {}",
                path.display()
            );
        }
        codestory_workspace::atomic_file::publish_new_private_file_atomic(
            path,
            "codestory-packet-diagnostics",
            &operation.value.diagnostics.bytes,
        )
        .map_err(anyhow::Error::new)?;
    }
    let markdown = render_packet_projection_markdown(&operation.value.projection);
    let rendered = RenderedPublicOutput::structured(&operation.value.projection, markdown)?;
    let operation = runtime::map_public_operation(operation, |_| rendered);
    emit_public_operation(cmd.format, operation, cmd.output_file.as_deref())
}

fn render_packet_projection_markdown(packet: &PacketProjectionV3Dto) -> String {
    let mut markdown = String::from("# Packet evidence\n\n");
    match packet {
        PacketProjectionV3Dto::Complete {
            identity,
            status,
            retrieval,
            evidence,
            gaps,
            continuation,
            diagnostics,
            ..
        } => {
            let _ = writeln!(markdown, "packet_id: `{}`", identity.packet_id.as_str());
            let _ = writeln!(
                markdown,
                "availability: `{}`",
                evidence_status_label(status)
            );
            let _ = writeln!(
                markdown,
                "retrieval: `{}`",
                retrieval_state_label(&retrieval.state)
            );
            if !evidence.as_slice().is_empty() {
                let _ = writeln!(markdown, "\n## Evidence");
                let _ = writeln!(markdown, "{REPO_CONTENT_BOUNDARY_LINE}");
                for row in evidence.as_slice() {
                    let summary = row
                        .summary
                        .as_ref()
                        .map_or("evidence row", |value| value.as_str());
                    let location = row.path.as_ref().map_or("", |value| value.as_str());
                    let _ = writeln!(markdown, "- {summary} ({location})");
                }
            }
            if !gaps.as_slice().is_empty() {
                let _ = writeln!(markdown, "\n## Gaps");
                for gap in gaps.as_slice() {
                    let message = gap
                        .message
                        .as_ref()
                        .map_or("additional evidence required", |value| value.as_str());
                    let _ = writeln!(markdown, "- {message}");
                }
            }
            if let Some(continuation) = continuation {
                let _ = writeln!(
                    markdown,
                    "\ncontinuation: `{}` (remaining_rounds={})",
                    continuation.continuation_id.as_str(),
                    continuation.remaining_rounds
                );
            }
            if let codestory_contracts::packet_projection_v3::DiagnosticsCapabilityV3Dto::Available { reference } = diagnostics {
                let _ = writeln!(
                    markdown,
                    "diagnostics_sha256: `{}`",
                    reference.sha256.as_str()
                );
            }
        }
        PacketProjectionV3Dto::BudgetExceeded {
            identity,
            gaps,
            maximum_bytes,
            required_complete_bytes,
            ..
        } => {
            let _ = writeln!(markdown, "packet_id: `{}`", identity.packet_id.as_str());
            let _ = writeln!(markdown, "availability: `unavailable`");
            for gap in gaps.as_slice() {
                let _ = writeln!(
                    markdown,
                    "gap: `output_budget_exceeded` (`{}`)",
                    gap.identity.gap_id.as_str()
                );
            }
            let _ = writeln!(
                markdown,
                "result_budget: `{maximum_bytes}` bytes; complete projection required `{required_complete_bytes}` bytes"
            );
        }
    }
    markdown
}

fn evidence_status_label(status: &EvidenceAvailabilityV3Dto) -> &'static str {
    match status {
        EvidenceAvailabilityV3Dto::Available => "available",
        EvidenceAvailabilityV3Dto::ContinuationAvailable => "continuation_available",
        EvidenceAvailabilityV3Dto::NoUsefulEvidence => "no_useful_evidence",
        EvidenceAvailabilityV3Dto::Unavailable => "unavailable",
    }
}

fn retrieval_state_label(state: &RetrievalStateV3Dto) -> &'static str {
    match state {
        RetrievalStateV3Dto::Full => "full",
        RetrievalStateV3Dto::Degraded => "degraded",
        RetrievalStateV3Dto::Unavailable => "unavailable",
    }
}

pub(in crate::app) fn packet_request_from_command(cmd: &PacketCommand) -> AgentPacketRequestDto {
    AgentPacketRequestDto {
        question: cmd.question.clone(),
        budget: cmd.budget.into(),
        task_class: cmd.task_class.map(Into::into),
        probes: cmd.probes.clone(),
        extra_probes: cmd.extra_probes.clone(),
        latency_budget_ms: cmd.latency_budget_ms,
        parent_packet_id: cmd.parent_packet_id.clone(),
        option_ids: cmd.option_ids.clone(),
        core_generation_id: cmd.core_generation_id.clone(),
        retrieval_generation: cmd.retrieval_generation.clone(),
    }
}

#[cfg(test)]
pub(in crate::app) fn enforce_packet_cli_json_output_budget(
    project_root: &std::path::Path,
    operation: &mut codestory_runtime::PublicOperation<AgentPacketDto>,
    executable: &std::path::Path,
) -> Result<()> {
    let envelope = codestory_runtime::PublicOperation {
        value: (),
        core_publication: operation.core_publication.clone(),
        retrieval_publication: operation.retrieval_publication.clone(),
        operation_id: operation.operation_id.clone(),
        attempt: operation.attempt,
    };
    let _ = render_public_operation_json_content(&envelope, &operation.value)?;
    codestory_runtime::enforce_packet_output_budget_for_representation(
        project_root,
        &mut operation.value,
        |packet| {
            let mut public_packet = packet.clone();
            codestory_runtime::bind_packet_follow_up_program(
                project_root,
                &mut public_packet,
                executable,
            );
            render_public_operation_json_content(&envelope, &public_packet)
                .expect("packet public JSON rendering was validated before budget enforcement")
                .len()
        },
    )
    .map_err(map_api_error)?;
    codestory_runtime::bind_packet_follow_up_program(
        project_root,
        &mut operation.value,
        executable,
    );
    Ok(())
}

#[cfg(test)]
pub(in crate::app) fn render_packet_markdown(
    project_root: &std::path::Path,
    packet: &AgentPacketDto,
) -> String {
    let mut markdown = String::new();
    let _ = writeln!(markdown, "# Packet");
    let _ = writeln!(
        markdown,
        "question: `{}`",
        packet.question.replace('\n', " ")
    );
    if !packet.support.is_empty() {
        let _ = writeln!(markdown, "\n## Support");
        let _ = writeln!(markdown, "{REPO_CONTENT_BOUNDARY_LINE}");
        for unit in &packet.support {
            let _ = writeln!(markdown, "- {}", unit.summary);
        }
    }
    let _ = writeln!(
        markdown,
        "\ndisposition: `{}`",
        packet_disposition_label(packet.disposition.kind)
    );
    if let Some(reason) = packet.disposition.reason.as_deref() {
        let _ = writeln!(markdown, "- {}", reason);
        if let Some(drill) = &packet.disposition.drill {
            for option in &drill.options {
                let _ = writeln!(markdown, "- drill `{}`", option.id);
            }
        }
    }
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
    if packet.budget.truncated {
        let _ = writeln!(
            markdown,
            "truncated: `{}` ({})",
            packet.budget.truncated,
            packet.budget.omitted_sections.join(", ")
        );
    }
    append_packet_operator_header(&mut markdown, packet);

    markdown.push('\n');
    markdown.push_str(&render_context_markdown(project_root, &packet.answer));
    markdown
}

#[cfg(test)]
fn append_packet_operator_header(markdown: &mut String, packet: &AgentPacketDto) {
    let _ = writeln!(markdown, "## Status");
    let _ = writeln!(
        markdown,
        "status: {}",
        packet_operator_status(packet.disposition.kind)
    );
    let _ = writeln!(markdown, "## Trust");
    let _ = writeln!(
        markdown,
        "trust: disposition={} budget_truncated={} omitted_sections={}",
        packet_disposition_label(packet.disposition.kind),
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

#[cfg(test)]
pub(super) fn packet_operator_status(kind: PacketDispositionKindDto) -> &'static str {
    match kind {
        PacketDispositionKindDto::Supported => "ready",
        PacketDispositionKindDto::DrillOnce => "needs_attention",
        PacketDispositionKindDto::NotEstablished | PacketDispositionKindDto::Unavailable => {
            "blocked"
        }
    }
}

#[cfg(test)]
pub(super) fn packet_budget_omitted_sections(packet: &AgentPacketDto) -> String {
    if packet.budget.omitted_sections.is_empty() {
        "none".to_string()
    } else {
        packet.budget.omitted_sections.join(",")
    }
}

#[cfg(test)]
fn packet_operator_next_action(packet: &AgentPacketDto) -> String {
    match packet.disposition.kind {
        PacketDispositionKindDto::Supported | PacketDispositionKindDto::NotEstablished => {
            "Answer from compiled support units, or say the repository does not establish the claim."
                .to_string()
        }
        PacketDispositionKindDto::Unavailable => packet
            .disposition
            .reason
            .clone()
            .unwrap_or_else(|| "Typed preparation is required before another packet.".to_string()),
        PacketDispositionKindDto::DrillOnce => packet
            .disposition
            .drill
            .as_ref()
            .and_then(|drill| drill.options.first())
            .map(|option| format!("Execute drill option `{}` once.", option.id))
            .unwrap_or_else(|| "Execute the listed drill option ids once.".to_string()),
    }
}

#[cfg(test)]
pub(in crate::app) fn packet_budget_mode_label(mode: PacketBudgetModeDto) -> &'static str {
    match mode {
        PacketBudgetModeDto::Tiny => "tiny",
        PacketBudgetModeDto::Compact => "compact",
        PacketBudgetModeDto::Standard => "standard",
        PacketBudgetModeDto::Deep => "deep",
    }
}

#[cfg(test)]
pub(in crate::app) fn packet_task_class_label(task_class: PacketTaskClassDto) -> &'static str {
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

#[cfg(test)]
pub(crate) fn packet_disposition_label(kind: PacketDispositionKindDto) -> &'static str {
    match kind {
        PacketDispositionKindDto::Supported => "supported",
        PacketDispositionKindDto::DrillOnce => "drill_once",
        PacketDispositionKindDto::NotEstablished => "not_established",
        PacketDispositionKindDto::Unavailable => "unavailable",
    }
}

#[cfg(test)]
mod tests {
    use super::packet_request_from_command;
    use crate::args::{Cli, Command};
    use clap::Parser;

    #[test]
    fn packet_request_forwards_typed_drill_flags() {
        let cli = Cli::try_parse_from([
            "codestory-cli",
            "packet",
            "--project",
            "/tmp/project",
            "--question",
            "explain indexing",
            "--parent-packet-id",
            "packet-1",
            "--option-id",
            "omitted_mandatory_support:symbol%3A42",
            "--core-generation-id",
            "core-1",
            "--retrieval-generation",
            "retrieval-1",
        ])
        .expect("parse packet drill flags");
        let Command::Packet(cmd) = cli.command else {
            panic!("expected packet command");
        };
        let request = packet_request_from_command(&cmd);
        assert_eq!(request.parent_packet_id.as_deref(), Some("packet-1"));
        assert_eq!(
            request.option_ids,
            vec!["omitted_mandatory_support:symbol%3A42".to_string()]
        );
        assert_eq!(request.core_generation_id.as_deref(), Some("core-1"));
        assert_eq!(request.retrieval_generation.as_deref(), Some("retrieval-1"));
        assert_eq!(request.question, "explain indexing");
    }
}
