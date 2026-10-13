use super::super::artifacts::{ensure_dot_only_for_trail, preflight_output_file};
use super::super::lifecycle::{OpenedAgentSurface, open_agent_surface};
use crate::args;
use crate::args::PacketCommand;
#[cfg(test)]
use crate::output::render_context_markdown;
use crate::output::{
    REPO_CONTENT_BOUNDARY_LINE, RenderedPublicOutput, emit_public_operation,
    render_public_operation_json_content, render_public_operation_json_content_with_byte_budget,
};
use crate::runtime;
use crate::runtime::map_api_error;
use anyhow::Result;
use codestory_contracts::api::AgentPacketRequestDto;
#[cfg(test)]
use codestory_contracts::api::{AgentPacketDto, PacketBudgetModeDto, PacketDispositionKindDto};
use codestory_contracts::packet_projection_v3::{
    EvidenceAvailabilityV3Dto, PacketProjectionV3Dto, RetrievalStateV3Dto,
};
#[cfg(feature = "benchmark-support")]
use sha2::{Digest, Sha256};
use std::fmt::Write as _;

#[cfg(feature = "benchmark-support")]
fn benchmark_request_receipt(request: &AgentPacketRequestDto) -> serde_json::Value {
    serde_json::json!({
        "question_sha256": format!("{:x}", Sha256::digest(request.question.as_bytes())),
        "parent_packet_id": request.parent_packet_id.as_deref(),
        "option_ids": &request.option_ids,
        "core_generation_id": request.core_generation_id.as_deref(),
        "retrieval_generation": request.retrieval_generation.as_deref(),
    })
}

// Formatting preference for the existing v3 16-KiB public result contract.
// Runtime finalization still owns enforcement and the typed oversized result.
const PACKET_CLI_RESULT_MAX_BYTES_V3: usize = 16 * 1024;

fn render_packet_projection_json<T>(
    operation: &codestory_runtime::PublicOperation<T>,
    projection: &PacketProjectionV3Dto,
) -> Result<String> {
    render_public_operation_json_content_with_byte_budget(
        operation,
        projection,
        PACKET_CLI_RESULT_MAX_BYTES_V3,
    )
}

fn measure_packet_projection<T>(
    operation: &codestory_runtime::PublicOperation<T>,
    format: args::OutputFormat,
    projection: &PacketProjectionV3Dto,
) -> Result<usize, ()> {
    let rendered = match format {
        args::OutputFormat::Json => render_packet_projection_json(operation, projection),
        _ => render_public_operation_json_content(operation, projection),
    };
    rendered.map(|content| content.len()).map_err(|_| ())
}

pub(in crate::app) fn run_packet(cmd: PacketCommand) -> Result<()> {
    ensure_dot_only_for_trail(cmd.format, "packet")?;
    preflight_output_file(cmd.output_file.as_deref())?;
    preflight_output_file(cmd.diagnostics_out.as_deref())?;
    #[cfg(feature = "benchmark-support")]
    preflight_output_file(cmd.benchmark_retrieval_proof_out.as_deref())?;
    args::validate_packet_probe_arguments(&cmd.probes).map_err(anyhow::Error::msg)?;
    let OpenedAgentSurface { runtime, .. } = open_agent_surface(
        &cmd.project,
        cmd.profile,
        cmd.run_id.as_deref(),
        cmd.refresh,
        "packet",
    )?;

    let request = packet_request_from_command(&cmd);
    let _latency_scope = codestory_runtime::enter_packet_latency_scope(request.latency_budget_ms);
    #[cfg(feature = "benchmark-support")]
    let benchmark_disable_dense_semantic = cmd.benchmark_disable_dense_semantic;
    #[cfg(feature = "benchmark-support")]
    let benchmark_retrieval_proof_out = cmd.benchmark_retrieval_proof_out.clone();
    let mut operation = runtime.run_public_operation("packet", || {
        #[cfg(feature = "benchmark-support")]
        {
        let packet = {
            let execution = runtime
                .browser
                .packet_for_benchmark(request.clone(), !benchmark_disable_dense_semantic)
                .map_err(map_api_error)?;
            if let Some(path) = benchmark_retrieval_proof_out.as_deref() {
                let publication = execution
                    .packet
                    .answer
                    .retrieval_trace
                    .retrieval_publication
                    .as_ref();
                let proof = serde_json::json!({
                    "contract": "codestory.packet-builder-ablation-receipt/v1",
                    "requested_dense_semantic": !benchmark_disable_dense_semantic,
                    "request": benchmark_request_receipt(&request),
                    "retrieval_proof": execution.retrieval_proof,
                    "core_generation_id": publication.map(|value| value.core_generation_id.as_str()),
                    "core_run_id": publication.map(|value| value.core_run_id.as_str()),
                    "retrieval_generation": publication.map(|value| value.retrieval_generation.as_str()),
                    "semantic_generation": publication.map(|value| value.semantic_generation.as_str()),
                });
                let bytes = serde_json::to_vec_pretty(&proof)?;
                codestory_workspace::atomic_file::publish_new_private_file_atomic(
                    path,
                    "codestory-packet-builder-ablation-proof",
                    &bytes,
                )
                .map_err(anyhow::Error::new)?;
            }
            execution.packet
        };
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
        }

        #[cfg(not(feature = "benchmark-support"))]
        {
            let active = runtime
                .public_operation
                .active_packet_representation_context_v3()
                .ok_or_else(|| {
                    map_api_error(codestory_contracts::api::ApiError::internal(
                        "Packet representation requires its actual fresh pinned operation.",
                    ))
                })?;
            let envelope = codestory_runtime::PublicOperation {
                value: (),
                core_publication: Some(active.publication.core_publication),
                retrieval_publication: active.publication.retrieval_publication,
                operation_id: active.operation_id,
                attempt: active.attempt,
                freshness: active.freshness,
            };
            runtime
                .browser
                .packet_for_representation_v3(
                    request.clone(),
                    "codestory-cli",
                    &mut |candidate| measure_packet_projection(&envelope, cmd.format, candidate),
                )
                .map(|represented| represented.product)
                .map_err(map_api_error)
        }
    })?;
    let envelope = codestory_runtime::PublicOperation {
        value: (),
        core_publication: operation.core_publication.clone(),
        retrieval_publication: operation.retrieval_publication.clone(),
        operation_id: operation.operation_id.clone(),
        attempt: operation.attempt,
        freshness: operation.freshness,
    };
    codestory_runtime::finalize_packet_projection_v3_for_representation(
        &mut operation.value.projection,
        |projection| measure_packet_projection(&envelope, cmd.format, projection),
    )
    .map_err(map_api_error)?;
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
    let rendered = match cmd.format {
        args::OutputFormat::Json => RenderedPublicOutput::text(render_packet_projection_json(
            &envelope,
            &operation.value.projection,
        )?),
        _ => RenderedPublicOutput::structured(&operation.value.projection, markdown)?,
    };
    // Render and finalize first. Only this actual Complete product can issue
    // prior-source authority; rejected previews and fallbacks never do.
    runtime
        .public_operation
        .commit_packet_served_sources_v3("codestory-cli", &operation.value)
        .map_err(map_api_error)?;
    let operation = runtime::map_public_operation(operation, |_| rendered);
    emit_public_operation(cmd.format, operation, cmd.output_file.as_deref())
}

/// Portable argv replacing prior continuation flags. The original invocation
/// retains project/question/budget/profile/run/cache; shell quoting is rendering.
fn packet_patch_cli_arguments(
    patch: &codestory_contracts::packet_projection_v3::PacketRequestPatchV3Dto,
) -> Vec<String> {
    let mut args = Vec::new();
    for probe in patch.probes.as_slice() {
        args.push("--probe".to_owned());
        args.push(serde_json::to_string(probe).expect("serializable probe"));
    }
    if let Some(parent) = &patch.parent_packet_id {
        args.push("--parent-packet-id".to_owned());
        args.push(parent.as_str().to_owned());
    }
    for option in patch.option_ids.as_slice() {
        args.push("--option-id".to_owned());
        args.push(option.as_str().to_owned());
    }
    args.push("--core-generation-id".to_owned());
    args.push(patch.core_generation_id.as_str().to_owned());
    if let Some(generation) = &patch.retrieval_generation {
        args.push("--retrieval-generation".to_owned());
        args.push(generation.as_str().to_owned());
    }
    args
}

/// Exact POSIX display of the portable replacement argv, labelled by the caller.
fn packet_patch_cli_fragment(
    patch: &codestory_contracts::packet_projection_v3::PacketRequestPatchV3Dto,
) -> String {
    packet_patch_cli_arguments(patch)
        .into_iter()
        .enumerate()
        .map(|(index, value)| {
            if index % 2 == 0 {
                value
            } else {
                format!("'{}'", value.replace('\'', "'\\''"))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
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
                    if let Some(patch) = &row.packet_request_patch {
                        let _ = writeln!(
                            markdown,
                            "packet_request_patch: `{}`",
                            serde_json::to_string(patch).expect("serializable packet patch")
                        );
                        let _ = writeln!(
                            markdown,
                            "Replacement CLI arguments (POSIX): `{}`",
                            packet_patch_cli_fragment(patch)
                        );
                    }
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
            if evidence
                .as_slice()
                .iter()
                .any(|row| row.packet_request_patch.is_some())
                || continuation
                    .as_ref()
                    .is_some_and(|value| value.packet_request_patch.is_some())
            {
                let _ = writeln!(
                    markdown,
                    "\nApply packet_request_patch to the same packet request, retaining project/question/budget and transport configuration. Replace old probes, parent/options and generation pins; do not append them. For CLI, use --probe JSON for each probe, --parent-packet-id, --option-id, --core-generation-id and --retrieval-generation for populated fields, omitting cleared fields. This is a bounded packet operation and may perform retrieval."
                );
            }
            if let Some(continuation) = continuation {
                let _ = writeln!(
                    markdown,
                    "\ncontinuation: `{}` (remaining_rounds={})",
                    continuation.continuation_id.as_str(),
                    continuation.remaining_rounds
                );
                if let Some(patch) = &continuation.packet_request_patch {
                    let _ = writeln!(
                        markdown,
                        "packet_request_patch: `{}`",
                        serde_json::to_string(patch).expect("serializable packet patch")
                    );
                    let _ = writeln!(
                        markdown,
                        "Replacement CLI arguments (POSIX): `{}`",
                        packet_patch_cli_fragment(patch)
                    );
                }
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
        RetrievalStateV3Dto::Symbolic => "symbolic",
        RetrievalStateV3Dto::Degraded => "degraded",
        RetrievalStateV3Dto::Unavailable => "unavailable",
    }
}

pub(in crate::app) fn packet_request_from_command(cmd: &PacketCommand) -> AgentPacketRequestDto {
    AgentPacketRequestDto {
        question: cmd.question.clone(),
        budget: cmd.budget.into(),
        probes: cmd.probes.clone(),
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
        freshness: operation.freshness,
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
    #[cfg(feature = "benchmark-support")]
    use super::benchmark_request_receipt;
    use super::packet_request_from_command;
    use crate::args::{Cli, Command};
    use clap::Parser;
    use codestory_contracts::packet_projection_v3::PacketProjectionV3Dto;

    fn packet_json_fixture(lines: u32, path_padding: usize) -> PacketProjectionV3Dto {
        let source = "let value = \"雪\";\r\n".repeat(lines as usize);
        let rows = (0..16).map(|index| serde_json::json!({
            "identity": {"evidence_id": format!("fixture-evidence-{index:02}")},
            "kind": "exact_source",
            "path": format!("src/{}/generic_module_{index:02}/implementation.rs", "p".repeat(path_padding)),
            "symbol_id": null, "start_line": 1, "end_line": lines,
            "summary": source,
            "source_delivery": {
                "requested_range": {"start_line": 1, "end_line": lines + 1},
                "source_sha256": "d".repeat(64), "remaining_source_bytes": "let value = \"雪\";\r\n".len(),
                "next_start_line": lines + 1, "blocked_line_rendered_bytes": null
            }
        })).collect::<Vec<_>>();
        serde_json::from_value(serde_json::json!({
            "kind": "complete", "schema_version": 3,
            "identity": {"packet_id": "fixture-packet", "request_id": "fixture-request", "question_sha256": "a".repeat(64)},
            "publication": {
                "core": {"project_id": "fixture-project", "generation_id": "b".repeat(36), "run_id": "c".repeat(36)},
                "retrieval": {"core_generation_id": "b".repeat(36), "core_run_id": "c".repeat(36),
                    "retrieval_generation": "r".repeat(64), "retrieval_input_sha256": "e".repeat(64),
                    "semantic_generation": "s".repeat(64)}
            },
            "status": "available", "retrieval": {"state": "full", "generation_id": "r".repeat(64)},
            "evidence": rows, "gaps": [], "continuation": null,
            "diagnostics": {"availability": "unavailable"}, "answer_sufficiency": "not_asserted"
        })).expect("bounded complete packet fixture")
    }

    fn packet_json_operation() -> codestory_runtime::PublicOperation<()> {
        codestory_runtime::PublicOperation {
            value: (),
            core_publication: Some(codestory_contracts::api::IndexPublicationDto {
                generation: 7,
                generation_id: "b".repeat(36),
                run_id: "c".repeat(36),
                mode: codestory_contracts::api::IndexPublicationModeDto::Full,
                published_at_epoch_ms: 1234567890,
            }),
            retrieval_publication: Some(
                codestory_contracts::api::EmbeddingVectorPublicationIdentityDto {
                    core_generation_id: "b".repeat(36),
                    core_run_id: "c".repeat(36),
                    retrieval_generation: "r".repeat(64),
                    retrieval_input_hash: "e".repeat(64),
                    semantic_generation: "s".repeat(64),
                },
            ),
            operation_id: "fixture-public-operation".to_string(),
            attempt: 1,
            freshness: codestory_runtime::OperationFreshness::Fresh,
        }
    }

    #[test]
    fn packet_cli_selects_whole_pages_against_its_complete_envelope() {
        use codestory_contracts::compilation::*;
        use codestory_runtime::representation_budget_test_support as fixture;
        let input = PacketCompilationInputV1 {
            contract_version: PACKET_COMPILATION_CONTRACT_VERSION_V1,
            publication: PacketCompilationPublicationV1 {
                project_id: "project-1".into(),
                core_generation_id: "core-generation-1".into(),
                retrieval_generation: Some("retrieval-generation-1".into()),
            },
            admissions: (0..16)
                .map(|ordinal| PacketAdmissionReceiptV1 {
                    packet_ordinal: ordinal,
                    stable_identity: format!("node:{ordinal}"),
                    score_version: PACKET_RETRIEVAL_SCORE_VERSION_V1.into(),
                    reserved_source_bytes: 512,
                    origin: PacketAdmissionOriginV1::Retrieval,
                })
                .collect(),
            sources: (0..16)
                .map(|ordinal| PacketHydratedSourceRangeV1 {
                    callable_body: None,
                    stable_identity: format!("node:{ordinal}"),
                    path: format!("src/{}/page_{ordinal}.rs", "directory".repeat(30)),
                    symbol: Some(format!("page_{ordinal}")),
                    start_line: 1,
                    end_line: 1,
                    source: format!(
                        "```text\n>    1 | fn page_{ordinal}() {{ /* {} */ }}\n```",
                        "quoted\\\"value ".repeat(17)
                    ),
                    parser_completeness: PacketParserCompletenessV1::Complete,
                    delivery: Some(PacketSourceDeliveryV1 {
                        requested_range: PacketSourceRangeV1 {
                            start_line: 1,
                            end_line: 3,
                        },
                        source_sha256: "a".repeat(64),
                        remaining_source_bytes: 40,
                        next_start_line: Some(2),
                        blocked_line_rendered_bytes: None,
                    }),
                })
                .collect(),
            relations: vec![],
            ambiguities: vec![],
            admission_gaps: vec![],
        };
        let request: codestory_contracts::api::AgentPacketRequestDto =
            serde_json::from_value(serde_json::json!({"question":"Inspect repository source"}))
                .unwrap();
        let (packet, _) = fixture::default_packet(input.clone(), &request);
        let original = fixture::project_packet(&request, &packet, |_| Ok(0)).unwrap();
        let operation = packet_json_operation();
        assert!(
            super::measure_packet_projection(
                &operation,
                crate::args::OutputFormat::Json,
                &original.projection,
            )
            .unwrap()
                > super::PACKET_CLI_RESULT_MAX_BYTES_V3
        );
        let selected = fixture::select_prepared(input.clone(), &request, |p| {
            super::measure_packet_projection(&operation, crate::args::OutputFormat::Json, p)
        })
        .unwrap();
        let PacketProjectionV3Dto::Complete {
            evidence,
            continuation,
            ..
        } = &selected.product.projection
        else {
            panic!("A useful whole-page prefix must fit the unchanged CLI envelope");
        };
        assert!(!evidence.as_slice().is_empty());
        assert!(selected.packet.support.len() < packet.support.len());
        assert!(continuation.is_some());
        for row in evidence.as_slice() {
            let Some(delivery) = &row.source_delivery else {
                continue;
            };
            let source = input
                .sources
                .iter()
                .find(|s| row.path.as_ref().is_some_and(|p| p.as_str() == s.path))
                .unwrap();
            assert_eq!(row.summary.as_ref().unwrap().as_str(), source.source);
            assert_eq!(Some(delivery), source.delivery.as_ref());
        }
        let rendered =
            super::render_packet_projection_json(&operation, &selected.product.projection).unwrap();
        assert!(rendered.len() <= super::PACKET_CLI_RESULT_MAX_BYTES_V3);
        assert_eq!(
            rendered.len(),
            super::measure_packet_projection(
                &operation,
                crate::args::OutputFormat::Json,
                &selected.product.projection,
            )
            .unwrap()
        );
    }

    #[test]
    fn packet_json_budget_preserves_pretty_output_when_it_fits() {
        let operation = packet_json_operation();
        let projection = packet_json_fixture(1, 0);
        let pretty = super::render_public_operation_json_content(&operation, &projection).unwrap();
        assert!(pretty.len() < super::PACKET_CLI_RESULT_MAX_BYTES_V3);
        assert_eq!(
            super::render_packet_projection_json(&operation, &projection).unwrap(),
            pretty
        );
    }

    #[test]
    fn packet_json_budget_preserves_complete_source_and_exact_emitted_bytes() {
        let operation = packet_json_operation();
        let original = packet_json_fixture(19, 0);
        let pretty = super::render_public_operation_json_content(&operation, &original).unwrap();
        let rendered = super::render_packet_projection_json(&operation, &original).unwrap();
        assert!(pretty.len() > super::PACKET_CLI_RESULT_MAX_BYTES_V3);
        assert!(rendered.len() <= super::PACKET_CLI_RESULT_MAX_BYTES_V3);
        assert!(rendered.ends_with('\n'));
        let expected = crate::runtime::public_operation_json_value(&operation, &original).unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&rendered).unwrap(),
            expected
        );
        let mut previous = original.clone();
        codestory_runtime::finalize_packet_projection_v3_for_representation(&mut previous, |p| {
            Ok(super::render_public_operation_json_content(&operation, p)
                .unwrap()
                .len())
        })
        .unwrap();
        assert!(matches!(
            previous,
            PacketProjectionV3Dto::BudgetExceeded { .. }
        ));
        let mut projection = original.clone();
        let measured = codestory_runtime::finalize_packet_projection_v3_for_representation(
            &mut projection,
            |p| {
                Ok(super::render_packet_projection_json(&operation, p)
                    .unwrap()
                    .len())
            },
        )
        .unwrap();
        assert_eq!(projection, original);
        assert_eq!(measured, rendered.len());
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("packet.json");
        let response = crate::runtime::map_public_operation(operation, |_| {
            crate::output::RenderedPublicOutput::text(rendered.clone())
        });
        crate::output::emit_public_operation(
            crate::args::OutputFormat::Json,
            response,
            Some(&path),
        )
        .unwrap();
        assert_eq!(std::fs::read(path).unwrap(), rendered.as_bytes());
        eprintln!(
            "packet-json proof: pretty={} compact={} measured={} emitted={}",
            pretty.len(),
            rendered.len(),
            measured,
            rendered.len()
        );
    }

    #[test]
    fn packet_json_budget_keeps_typed_refusal_when_compact_is_oversized() {
        let operation = packet_json_operation();
        let mut projection = packet_json_fixture(19, 1024);
        let original_bytes = super::render_packet_projection_json(&operation, &projection)
            .unwrap()
            .len();
        assert!(original_bytes > super::PACKET_CLI_RESULT_MAX_BYTES_V3);
        let measured = codestory_runtime::finalize_packet_projection_v3_for_representation(
            &mut projection,
            |p| {
                Ok(super::render_packet_projection_json(&operation, p)
                    .unwrap()
                    .len())
            },
        )
        .unwrap();
        let PacketProjectionV3Dto::BudgetExceeded {
            maximum_bytes,
            required_complete_bytes,
            ..
        } = &projection
        else {
            panic!("mandatory source remains too large for the unchanged contract");
        };
        assert_eq!(*maximum_bytes, super::PACKET_CLI_RESULT_MAX_BYTES_V3 as u64);
        assert_eq!(*required_complete_bytes, original_bytes as u64);
        assert!(measured <= super::PACKET_CLI_RESULT_MAX_BYTES_V3);
        assert_eq!(
            measured,
            super::render_packet_projection_json(&operation, &projection)
                .unwrap()
                .len()
        );
    }

    #[test]
    fn packet_json_budget_counts_utf8_escaping_and_final_newline_at_boundary() {
        let operation = packet_json_operation();
        let projection = packet_json_fixture(19, 0);
        let expected =
            crate::runtime::public_operation_json_value(&operation, &projection).unwrap();
        let pretty = super::render_public_operation_json_content(&operation, &projection).unwrap();
        let at = crate::output::render_public_operation_json_content_with_byte_budget(
            &operation,
            &projection,
            pretty.len(),
        )
        .unwrap();
        assert_eq!(at, pretty);
        let below = crate::output::render_public_operation_json_content_with_byte_budget(
            &operation,
            &projection,
            pretty.len() - 1,
        )
        .unwrap();
        assert!(below.len() < pretty.len());
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&below).unwrap(),
            expected
        );
        assert!(below.ends_with('\n'));
        assert!(below.contains("雪"));
        assert!(below.contains("\\r\\n"));
    }

    #[test]
    fn packet_patch_portable_arguments_round_trip_the_real_parser_and_replace_old_state() {
        use codestory_contracts::{
            api::{AgentPacketRequestDto, DrillOptionDto},
            packet_projection_v3::PacketRequestPatchV3Dto,
        };
        let original: AgentPacketRequestDto = serde_json::from_value(serde_json::json!({"question":"inspect source","budget":"compact","latency_budget_ms":900,"probes":[{"kind":"symbol_id","id":"old"}],"parent_packet_id":"old-parent","option_ids":["old-option"],"core_generation_id":"old-core","retrieval_generation":"old-retrieval"})).unwrap();
        let page = serde_json::json!({"probes":[{"kind":"continuation","contract_version":1,"project_id":"project-1","core_generation_id":"core-1","retrieval_generation":"retrieval-1","selector":{"stable_identity":"node:7","symbol_id":"7","path":"src/reader's-雪.rs","reason":"source_budget_exceeded","source_range":{"start_line":4,"end_line":99}}}],"parent_packet_id":null,"option_ids":[],"core_generation_id":"core-1","retrieval_generation":"retrieval-1"});
        let broad = serde_json::json!({"probes":[],"parent_packet_id":"new-parent","option_ids":[DrillOptionDto::omitted_symbol("disconnected_seed:node:7","7").id],"core_generation_id":"core-1","retrieval_generation":"retrieval-1"});
        for value in [page, broad] {
            let patch: PacketRequestPatchV3Dto = serde_json::from_value(value).unwrap();
            let expected = patch.apply_to(&original).unwrap();
            let mut argv = [
                "codestory-cli",
                "packet",
                "--project",
                "/tmp/project",
                "--question",
                "inspect source",
                "--budget",
                "compact",
                "--latency-budget-ms",
                "900",
            ]
            .map(str::to_owned)
            .to_vec();
            argv.extend(super::packet_patch_cli_arguments(&patch));
            let cli = Cli::try_parse_from(argv)
                .expect("actual CLI must accept executable replacement arguments");
            let Command::Packet(cmd) = cli.command else {
                panic!("expected packet command");
            };
            assert_eq!(
                serde_json::to_value(packet_request_from_command(&cmd)).unwrap(),
                serde_json::to_value(expected).unwrap()
            );
            let fragment = super::packet_patch_cli_fragment(&patch);
            assert!(fragment.contains("--core-generation-id 'core-1'"));
            if !patch.probes.as_slice().is_empty() {
                assert!(fragment.contains("'\\''"));
                assert!(fragment.contains("雪"));
            }
        }
    }

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

    #[cfg(feature = "benchmark-support")]
    #[test]
    fn benchmark_receipt_binds_the_exact_packet_request() {
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
            "gap-1",
            "--core-generation-id",
            "core-1",
            "--retrieval-generation",
            "retrieval-1",
        ])
        .expect("parse packet request");
        let Command::Packet(cmd) = cli.command else {
            panic!("expected packet command");
        };
        let request = packet_request_from_command(&cmd);
        assert_eq!(
            benchmark_request_receipt(&request),
            serde_json::json!({
                "question_sha256": "467ace0f0ea2c522ed79a393b5c1258c96c5a10fc8740d736fab1d3aca8dad7c",
                "parent_packet_id": "packet-1",
                "option_ids": ["gap-1"],
                "core_generation_id": "core-1",
                "retrieval_generation": "retrieval-1",
            }),
        );
    }
}
