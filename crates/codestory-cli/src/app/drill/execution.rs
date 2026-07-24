use super::super::artifacts::ensure_dot_only_for_trail;
use super::super::lifecycle::{OpenedAgentSurface, open_agent_surface};
use super::super::rendering::dedupe_verification_targets;
use super::super::{elapsed_ms, packet_sufficiency_label};
use super::reporting::{
    DrillReportContents, render_drill_contents, validate_drill_output_dir, write_drill_report_file,
};
use super::summary_decision::ensure_trailing_newline;
use super::summary_evidence::drill_summary;
use crate::args;
use crate::args::{
    DrillAnchorOutput, DrillAnchorTimingsOutput, DrillBridgeEvidenceOutput, DrillBridgeOutput,
    DrillCommand, DrillCommandStatusOutput, DrillExecutionBoundaryOutput, DrillMechanicalOutput,
    DrillOutput, DrillRuntimeTimingsOutput, SearchHitOutput, VerificationTargetOutput,
};
use crate::output::render_drill_markdown;
use crate::runtime;
use crate::runtime::{RuntimeContext, map_api_error, refresh_label};
use crate::{display, drill_targeting, retrieval};
use anyhow::{Context, Result};
use codestory_contracts::api::{
    AgentCitationDto, AgentPacketDto, AgentPacketRequestDto, ApiError, IndexingPhaseTimings,
    NodeKind, PacketBudgetModeDto, PacketProofStatusDto, SearchMatchQualityDto, StorageStatsDto,
};
use std::collections::HashSet;
use std::time::Instant;

pub(in crate::app) fn run_drill(cmd: DrillCommand) -> Result<()> {
    ensure_dot_only_for_trail(cmd.format, "drill")?;
    let operation = execute_drill(&cmd)?;
    let contents = write_drill_outputs(cmd.format, &cmd.output_dir, &operation)?;
    print!("{}", contents.selected);
    Ok(())
}

struct PreparedDrill {
    runtime: RuntimeContext,
    before_stats: Option<StorageStatsDto>,
    before_unavailable_reason: Option<String>,
    refresh: String,
    phase_timings: Option<IndexingPhaseTimings>,
    setup_ms: u64,
    anchors: Vec<String>,
    packet_request: AgentPacketRequestDto,
}

fn prepare_drill(cmd: &DrillCommand) -> Result<PreparedDrill> {
    let setup_timer = Instant::now();
    validate_drill_output_dir(&cmd.output_dir)?;
    let OpenedAgentSurface {
        runtime,
        before,
        opened,
    } = open_agent_surface(
        &cmd.project,
        cmd.profile,
        cmd.run_id.as_deref(),
        cmd.refresh,
        "drill",
    )?;
    if cmd.refresh != args::RefreshMode::None {
        retrieval::finalize_retrieval_index_for_runtime(&runtime)
            .context("drill retrieval index finalize")?;
    }
    let before_unavailable_reason = before.is_none().then(|| {
        opened
            .refresh_reason
            .clone()
            .unwrap_or_else(|| "pre_refresh_summary_unavailable".to_string())
    });
    let anchors = drill_targeting::validated_drill_anchors(&cmd.anchors, "drill")?;
    let question = cmd
        .question
        .clone()
        .unwrap_or_else(|| format!("Investigate anchors: {}", anchors.join(", ")));
    Ok(PreparedDrill {
        runtime,
        before_stats: before.map(|summary| summary.stats),
        before_unavailable_reason,
        refresh: refresh_label(cmd.refresh, opened.refresh_mode),
        phase_timings: opened.phase_timings,
        setup_ms: elapsed_ms(setup_timer),
        packet_request: AgentPacketRequestDto {
            question,
            budget: PacketBudgetModeDto::Standard,
            task_class: None,
            probes: Vec::new(),
            extra_probes: anchors.clone(),
            include_evidence: true,
            latency_budget_ms: None,
        },
        anchors,
    })
}

pub(super) fn execute_drill(
    cmd: &DrillCommand,
) -> Result<codestory_runtime::PublicOperation<DrillOutput>> {
    let _ = cmd.jobs; // retained CLI compatibility; packet owns internal batch scheduling
    let total_timer = Instant::now();
    let PreparedDrill {
        runtime,
        before_stats,
        before_unavailable_reason,
        refresh,
        phase_timings,
        setup_ms,
        anchors: drill_anchors,
        packet_request,
    } = prepare_drill(cmd)?;
    let packet_timer = Instant::now();
    runtime.run_public_operation("drill", || {
        let pinned_summary = runtime.active_project_summary()?;
        let pinned_publication = runtime
            .public_operation
            .active_publication()
            .context("drill public operation has active publication identity")?;
        let sidecar_retrieval_mode = pinned_publication
            .retrieval_publication
            .as_ref()
            .map(|_| "full".to_string());
        let evidence_packet = execute_drill_packet(packet_request.clone(), |request| {
            runtime.browser.packet(request)
        })?;
        let question_search_ms = elapsed_ms(packet_timer);
        let evidence_assembly_timer = Instant::now();
        let citations = drill_packet_citations(&evidence_packet);
        let anchor_outputs =
            drill_packet_anchors(&runtime.project_root, &drill_anchors, &citations);
        let bridge_outputs = drill_packet_bridges(&runtime.project_root, &evidence_packet);
        let mut all_verification_targets =
            drill_packet_verification_targets(&runtime.project_root, &citations);
        dedupe_verification_targets(&mut all_verification_targets);
        let next_commands = evidence_packet.sufficiency.follow_up_commands.clone();
        let question_search = Some(DrillCommandStatusOutput {
            command: "packet".to_string(),
            status: packet_sufficiency_label(evidence_packet.sufficiency.status).to_string(),
            duration_ms: u64::from(evidence_packet.answer.retrieval_trace.total_latency_ms),
            artifact: None,
            error: None,
        });
        let evidence_assembly_ms = elapsed_ms(evidence_assembly_timer);
        let drill_timings = DrillRuntimeTimingsOutput {
            total_ms: elapsed_ms(total_timer),
            setup_ms,
            question_search_ms,
            anchor_resolution_ms: 0,
            supplemental_search_ms: 0,
            bridge_evidence_ms: 0,
            evidence_assembly_ms,
        };

        Ok(DrillOutput {
            project: display::clean_path_string(&pinned_summary.root),
            label: cmd.label.clone(),
            question: cmd.question.clone(),
            output_dir: display::clean_path_string(&cmd.output_dir.to_string_lossy()),
            mechanical: DrillMechanicalOutput {
                before_files: before_stats.as_ref().map(|stats| stats.file_count),
                before_nodes: before_stats.as_ref().map(|stats| stats.node_count),
                before_edges: before_stats.as_ref().map(|stats| stats.edge_count),
                before_errors: before_stats.as_ref().map(|stats| stats.error_count),
                before_unavailable_reason: before_unavailable_reason.clone(),
                after_files: pinned_summary.stats.file_count,
                after_nodes: pinned_summary.stats.node_count,
                after_edges: pinned_summary.stats.edge_count,
                after_errors: pinned_summary.stats.error_count,
                refresh: refresh.clone(),
                retrieval: pinned_summary.retrieval.clone(),
                sidecar_retrieval_mode,
                freshness: pinned_summary.freshness.clone(),
                phase_timings: phase_timings.clone(),
                drill_timings,
            },
            question_search,
            question_supplemental_searches: Vec::new(),
            anchors: anchor_outputs,
            bridges: bridge_outputs,
            execution_boundaries: drill_execution_boundaries(),
            verification_targets: all_verification_targets,
            evidence_packet,
            next_commands,
        })
    })
}

fn drill_execution_boundaries() -> Vec<DrillExecutionBoundaryOutput> {
    vec![DrillExecutionBoundaryOutput {
        command: "packet".to_string(),
        flow: vec![
            "plan question and explicit anchor probes".to_string(),
            "execute one bounded batch retrieval".to_string(),
            "adapt citations and sufficiency into drill reports".to_string(),
        ],
        source_files: vec![
            "crates/codestory-runtime/src/agent/orchestrator.rs".to_string(),
            "crates/codestory-runtime/src/agent/packet_batch.rs".to_string(),
        ],
    }]
}

pub(in crate::app) fn execute_drill_packet(
    request: AgentPacketRequestDto,
    execute: impl FnOnce(AgentPacketRequestDto) -> Result<AgentPacketDto, ApiError>,
) -> Result<AgentPacketDto> {
    execute(request).map_err(map_api_error)
}

pub(in crate::app) fn drill_packet_citations(packet: &AgentPacketDto) -> Vec<AgentCitationDto> {
    let mut citations = packet.answer.citations.clone();
    for claim in &packet.sufficiency.covered_claims {
        citations.extend(claim.citations.iter().cloned());
    }
    let mut seen = HashSet::new();
    citations.retain(|citation| {
        seen.insert((
            citation.node_id.0.clone(),
            citation.file_path.clone(),
            citation.line,
        ))
    });
    citations
}

pub(in crate::app) fn drill_packet_anchors(
    project_root: &std::path::Path,
    anchors: &[String],
    citations: &[AgentCitationDto],
) -> Vec<DrillAnchorOutput> {
    anchors
        .iter()
        .map(|anchor| {
            let normalized = codestory_runtime::normalize_symbol_query(anchor);
            let citation = citations
                .iter()
                .filter(|citation| drill_packet_citation_is_typed_resolvable(citation))
                .filter(|citation| {
                    let display = codestory_runtime::normalize_symbol_query(&citation.display_name);
                    display == normalized
                        || codestory_runtime::terminal_symbol_segment(&citation.display_name)
                            == normalized
                })
                .max_by(|left, right| left.score.total_cmp(&right.score));
            let chosen_anchor = citation.map(|citation| {
                drill_search_hit_from_packet_citation(project_root, anchor, citation)
            });
            let verification_targets = citation
                .and_then(|citation| drill_packet_verification_target(project_root, citation))
                .into_iter()
                .collect();
            DrillAnchorOutput {
                anchor: anchor.clone(),
                typed_hit_count: usize::from(citation.is_some()),
                chosen_anchor,
                verification_targets,
                consumer_summary: None,
                timings: DrillAnchorTimingsOutput::default(),
                commands: Vec::new(),
            }
        })
        .collect()
}

pub(in crate::app) fn drill_search_hit_from_packet_citation(
    project_root: &std::path::Path,
    query: &str,
    citation: &AgentCitationDto,
) -> SearchHitOutput {
    let file_path = citation
        .file_path
        .as_deref()
        .map(|path| display::relative_path(project_root, path));
    let match_quality = if codestory_runtime::normalize_symbol_query(query)
        == codestory_runtime::normalize_symbol_query(&citation.display_name)
    {
        SearchMatchQualityDto::NormalizedExact
    } else {
        SearchMatchQualityDto::SemanticSuggestion
    };
    let verification_targets = drill_packet_verification_target(project_root, citation)
        .into_iter()
        .collect();
    SearchHitOutput {
        number: None,
        node_id: citation.node_id.0.clone(),
        node_ref: crate::output::node_ref(
            project_root,
            citation.file_path.as_deref(),
            citation.line,
            &citation.display_name,
        ),
        display_name: citation.display_name.clone(),
        kind: citation.kind,
        file_path,
        line: citation.line,
        score: citation.score,
        origin: citation.origin,
        match_quality,
        resolvable: citation.resolvable,
        evidence_tier: citation.evidence_tier,
        evidence_producer: citation.evidence_producer.clone(),
        resolution_status: citation.resolution_status,
        eligible_for_sufficiency: citation.eligible_for_sufficiency,
        score_breakdown: citation.retrieval_score_breakdown.clone(),
        duplicate_of: None,
        excerpt: None,
        primary_occurrence_kind: None,
        symbol_role: citation.coverage_role.clone(),
        paired_refs: Vec::new(),
        verification_targets,
        resolution_hints: Vec::new(),
        why: citation
            .evidence_producer
            .iter()
            .map(|producer| format!("packet evidence producer: {producer}"))
            .collect(),
    }
}

pub(super) fn drill_packet_verification_target(
    project_root: &std::path::Path,
    citation: &AgentCitationDto,
) -> Option<VerificationTargetOutput> {
    if !drill_packet_citation_is_typed_resolvable(citation) {
        return None;
    }
    Some(VerificationTargetOutput {
        role: citation
            .coverage_role
            .clone()
            .unwrap_or_else(|| "packet citation".to_string()),
        path: display::relative_path(project_root, citation.file_path.as_deref()?),
        line: citation.line.unwrap_or(1),
        node_ref: None,
        reason: format!("packet citation for {}", citation.display_name),
    })
}

pub(in crate::app) fn drill_packet_citation_is_typed_resolvable(
    citation: &AgentCitationDto,
) -> bool {
    citation.resolvable
        && citation.kind != NodeKind::UNKNOWN
        && citation.evidence_tier
            != Some(codestory_contracts::api::PacketEvidenceTierDto::StructuralText)
        && citation.resolution_status
            != Some(codestory_contracts::api::PacketEvidenceResolutionDto::SourceRangeOnly)
}

pub(in crate::app) fn drill_packet_verification_targets(
    project_root: &std::path::Path,
    citations: &[AgentCitationDto],
) -> Vec<VerificationTargetOutput> {
    citations
        .iter()
        .filter_map(|citation| drill_packet_verification_target(project_root, citation))
        .collect()
}

pub(in crate::app) fn drill_packet_bridges(
    project_root: &std::path::Path,
    packet: &AgentPacketDto,
) -> Vec<DrillBridgeOutput> {
    packet
        .sufficiency
        .covered_claims
        .iter()
        .filter_map(|claim| {
            let from = claim
                .citations
                .iter()
                .find(|citation| drill_packet_citation_is_typed_resolvable(citation))?;
            let to = claim.citations.iter().find(|citation| {
                citation.node_id != from.node_id
                    && drill_packet_citation_is_typed_resolvable(citation)
            })?;
            let graph_backed = drill_packet_citations_share_graph_evidence(from, to);
            let mut endpoint_files = [from.file_path.clone(), to.file_path.clone()]
                .into_iter()
                .flatten()
                .map(|path| display::relative_path(project_root, &path))
                .collect::<Vec<_>>();
            endpoint_files.sort();
            endpoint_files.dedup();
            Some(DrillBridgeOutput {
                evidence: DrillBridgeEvidenceOutput {
                    from_anchor: from.display_name.clone(),
                    to_anchor: to.display_name.clone(),
                    status: if graph_backed {
                        "graph_path".to_string()
                    } else {
                        "source_truth_only".to_string()
                    },
                    strategy: "packet_claim".to_string(),
                    confidence: match claim.proof_status {
                        Some(PacketProofStatusDto::Proven) => "high",
                        Some(PacketProofStatusDto::Likely) => "medium",
                        _ => "low",
                    }
                    .to_string(),
                    evidence_kind: "packet_citations".to_string(),
                    from_node: Some(drill_search_hit_from_packet_citation(
                        project_root,
                        &from.display_name,
                        from,
                    )),
                    to_node: Some(drill_search_hit_from_packet_citation(
                        project_root,
                        &to.display_name,
                        to,
                    )),
                    graph_path: None,
                    shared_files: Vec::new(),
                    endpoint_files: endpoint_files.clone(),
                    evidence_files: endpoint_files,
                    next_commands: packet.sufficiency.follow_up_commands.clone(),
                    notes: vec![claim.claim.clone()],
                },
                command: DrillCommandStatusOutput {
                    command: "packet".to_string(),
                    status: packet_sufficiency_label(packet.sufficiency.status).to_string(),
                    duration_ms: 0,
                    artifact: None,
                    error: None,
                },
            })
        })
        .collect()
}

pub(super) fn drill_packet_citations_share_graph_evidence(
    from: &AgentCitationDto,
    to: &AgentCitationDto,
) -> bool {
    from.evidence_edge_ids
        .iter()
        .any(|edge| to.evidence_edge_ids.contains(edge))
}

pub(in crate::app) fn write_drill_outputs(
    format: args::OutputFormat,
    output_dir: &std::path::Path,
    operation: &codestory_runtime::PublicOperation<DrillOutput>,
) -> Result<DrillReportContents> {
    let output = &operation.value;
    let report_ext = match format {
        args::OutputFormat::Markdown => "md",
        args::OutputFormat::Json => "json",
        args::OutputFormat::Dot => unreachable!("dot was rejected above"),
    };
    let markdown = render_drill_markdown(output);
    let contents = render_drill_contents(format, operation, &markdown)?;
    let report_path = output_dir.join(format!("drill-report.{report_ext}"));
    write_drill_report_file(&report_path, &contents.selected)?;
    let markdown_path = output_dir.join("drill-report.md");
    if report_path != markdown_path {
        write_drill_report_file(&markdown_path, &contents.markdown)?;
    }
    let json_path = output_dir.join("drill-report.json");
    if report_path != json_path {
        write_drill_report_file(&json_path, &contents.json)?;
    }
    let summary = drill_summary(output);
    let summary = runtime::public_operation_json_value(operation, &summary)?;
    let summary_json = ensure_trailing_newline(
        serde_json::to_string_pretty(&summary).context("Failed to serialize drill summary JSON")?,
    );
    write_drill_report_file(&output_dir.join("drill-summary.json"), &summary_json)?;
    Ok(contents)
}
