use super::summary_decision::{
    DrillAvailabilityEvidence, dedupe_and_rank_drill_files, drill_summary_availability,
    drill_summary_evidence_target_details, drill_summary_freshness_samples,
    drill_summary_freshness_status, drill_summary_retrieval_status, drill_summary_stale_file_count,
    drill_summary_stats,
};
use crate::args::{
    DrillAnchorOutput, DrillOutput, DrillSummaryAnchorStatusOutput, DrillSummaryAnchorsOutput,
    DrillSummaryBridgeStatusOutput, DrillSummaryBridgesOutput, DrillSummaryEvidenceReviewOutput,
    DrillSummaryMechanicalOutput, DrillSummaryOpenGapsOutput, DrillSummaryOutput,
};
use codestory_contracts::api::IndexFreshnessStatusDto;
use codestory_contracts::packet_projection_v3::{
    ContinuationStateV3Dto, EvidenceAvailabilityV3Dto, GapKindV3Dto, PacketProjectionV3Dto,
    ProjectionGapRowV3Dto,
};

struct DrillPacketEvidenceView<'a> {
    availability: &'a EvidenceAvailabilityV3Dto,
    evidence_count: usize,
    gaps: &'a [ProjectionGapRowV3Dto],
    continuation: Option<&'a ContinuationStateV3Dto>,
}

pub(super) fn drill_summary(output: &DrillOutput) -> DrillSummaryOutput {
    let anchors = drill_summary_anchors(output);
    let bridges = drill_summary_bridges(output);
    let packet = drill_packet_evidence_view(&output.evidence_packet);
    let stale_freshness = output
        .mechanical
        .freshness
        .as_ref()
        .is_some_and(|freshness| freshness.status == IndexFreshnessStatusDto::Stale);
    let evidence_review = drill_summary_evidence_review(output, &packet, stale_freshness);
    let open_gaps = drill_summary_open_gaps(&packet, &evidence_review, stale_freshness);
    let availability = drill_summary_availability(
        output,
        DrillAvailabilityEvidence {
            resolved_anchors: anchors.resolved,
            graph_path_bridges: bridges.graph_path,
            partial_bridges: bridges.partial,
            unresolved_or_error_bridges: bridges.unresolved_or_error,
            packet_availability: packet.availability.clone(),
            evidence_count: packet.evidence_count,
            gap_count: packet.gaps.len(),
            continuation_available: packet.continuation.is_some(),
            pending_target_count: evidence_review.pending_target_count,
            stale_freshness,
        },
    );

    DrillSummaryOutput {
        summary_version: 2,
        project: output.project.clone(),
        label: output.label.clone(),
        question: output.question.clone(),
        output_dir: output.output_dir.clone(),
        full_report_json: "drill-report.json".to_string(),
        full_report_markdown: "drill-report.md".to_string(),
        mechanical: drill_summary_mechanical(output),
        anchors,
        bridges,
        evidence_review,
        open_gaps,
        availability,
    }
}

fn drill_summary_mechanical(output: &DrillOutput) -> DrillSummaryMechanicalOutput {
    let before_stats = match (
        output.mechanical.before_files,
        output.mechanical.before_nodes,
        output.mechanical.before_edges,
        output.mechanical.before_errors,
    ) {
        (Some(files), Some(nodes), Some(edges), Some(errors)) => {
            Some(drill_summary_stats(files, nodes, edges, errors))
        }
        _ => None,
    };
    DrillSummaryMechanicalOutput {
        refresh: output.mechanical.refresh.clone(),
        before: before_stats,
        before_unavailable_reason: output.mechanical.before_unavailable_reason.clone(),
        after: drill_summary_stats(
            output.mechanical.after_files,
            output.mechanical.after_nodes,
            output.mechanical.after_edges,
            output.mechanical.after_errors,
        ),
        index_available: output.mechanical.after_files > 0 && output.mechanical.after_errors == 0,
        error_delta: output.mechanical.before_errors.map(|before_errors| {
            i64::from(output.mechanical.after_errors) - i64::from(before_errors)
        }),
        retrieval_status: output
            .mechanical
            .retrieval
            .as_ref()
            .map(|retrieval| {
                drill_summary_retrieval_status(
                    retrieval,
                    output.mechanical.sidecar_retrieval_mode.as_deref(),
                )
            })
            .or_else(|| output.mechanical.sidecar_retrieval_mode.clone()),
        freshness_status: output
            .mechanical
            .freshness
            .as_ref()
            .map(drill_summary_freshness_status),
        stale_file_count: output
            .mechanical
            .freshness
            .as_ref()
            .map(drill_summary_stale_file_count)
            .unwrap_or_default(),
        freshness_samples: output
            .mechanical
            .freshness
            .as_ref()
            .map(drill_summary_freshness_samples)
            .unwrap_or_default(),
        phase_timing_available: output.mechanical.phase_timings.is_some(),
        drill_timings: output.mechanical.drill_timings.clone(),
    }
}

fn drill_summary_anchors(output: &DrillOutput) -> DrillSummaryAnchorsOutput {
    let anchor_statuses: Vec<_> = output
        .anchors
        .iter()
        .map(drill_summary_anchor_status)
        .collect();
    let resolved = anchor_statuses
        .iter()
        .filter(|anchor| anchor.status == "resolved")
        .count();
    let failed_anchor_commands = anchor_statuses
        .iter()
        .map(|anchor| anchor.failed_command_count)
        .sum();
    DrillSummaryAnchorsOutput {
        requested: output.anchors.len(),
        resolved,
        unresolved: output.anchors.len().saturating_sub(resolved),
        failed_command_count: failed_anchor_commands,
        statuses: anchor_statuses,
    }
}

fn drill_summary_anchor_status(anchor: &DrillAnchorOutput) -> DrillSummaryAnchorStatusOutput {
    let failed_command_count = anchor
        .commands
        .iter()
        .filter(|command| command.status != "ok")
        .count();
    let command_duration_ms = anchor
        .commands
        .iter()
        .map(|command| command.duration_ms)
        .sum();
    let slowest = anchor
        .commands
        .iter()
        .max_by_key(|command| command.duration_ms);
    DrillSummaryAnchorStatusOutput {
        anchor: anchor.anchor.clone(),
        status: if anchor.chosen_anchor.is_some() {
            "resolved".to_string()
        } else {
            "unresolved".to_string()
        },
        typed_hit_count: anchor.typed_hit_count,
        selected: anchor
            .chosen_anchor
            .as_ref()
            .map(|hit| hit.display_name.clone()),
        selected_node_id: anchor.chosen_anchor.as_ref().map(|hit| hit.node_id.clone()),
        selected_node_ref: anchor
            .chosen_anchor
            .as_ref()
            .and_then(|hit| hit.node_ref.clone()),
        selected_kind: anchor.chosen_anchor.as_ref().map(|hit| hit.kind),
        selected_file_path: anchor
            .chosen_anchor
            .as_ref()
            .and_then(|hit| hit.file_path.clone()),
        selected_line: anchor.chosen_anchor.as_ref().and_then(|hit| hit.line),
        caller_count: anchor
            .consumer_summary
            .as_ref()
            .map(|summary| summary.caller_count)
            .unwrap_or_default(),
        consumer_count: anchor
            .consumer_summary
            .as_ref()
            .map(|summary| summary.consumer_count)
            .unwrap_or_default(),
        text_hint_count: anchor
            .consumer_summary
            .as_ref()
            .map(|summary| summary.text_hint_count)
            .unwrap_or_default(),
        command_count: anchor.commands.len(),
        failed_command_count,
        command_duration_ms,
        total_duration_ms: anchor.timings.total_ms,
        resolution_duration_ms: anchor.timings.resolution_ms,
        consumer_summary_duration_ms: anchor.timings.consumer_summary_ms,
        slowest_command: slowest.map(|command| command.command.clone()),
        slowest_command_ms: slowest
            .map(|command| command.duration_ms)
            .unwrap_or_default(),
        evidence_target_count: anchor.verification_targets.len(),
    }
}

fn drill_summary_bridges(output: &DrillOutput) -> DrillSummaryBridgesOutput {
    let bridge_statuses: Vec<_> = output
        .bridges
        .iter()
        .map(|bridge| DrillSummaryBridgeStatusOutput {
            from_anchor: bridge.evidence.from_anchor.clone(),
            to_anchor: bridge.evidence.to_anchor.clone(),
            status: bridge.evidence.status.clone(),
            confidence: bridge.evidence.confidence.clone(),
            strategy: bridge.evidence.strategy.clone(),
            command_status: bridge.command.status.clone(),
        })
        .collect();
    let graph_path = bridge_statuses
        .iter()
        .filter(|bridge| drill_bridge_status_is_graph(&bridge.status))
        .count();
    let partial = bridge_statuses
        .iter()
        .filter(|bridge| drill_bridge_status_is_partial(&bridge.status))
        .count();
    let unresolved_or_error = bridge_statuses
        .iter()
        .filter(|bridge| {
            drill_bridge_status_is_unresolved(&bridge.status)
                || drill_bridge_command_status_is_unavailable(&bridge.command_status)
        })
        .count();
    DrillSummaryBridgesOutput {
        total: output.bridges.len(),
        graph_path,
        partial,
        unresolved_or_error,
        statuses: bridge_statuses,
    }
}

pub(super) fn drill_bridge_command_status_is_unavailable(status: &str) -> bool {
    matches!(status, "error" | "unavailable" | "no_useful_evidence")
}

fn drill_packet_evidence_view(projection: &PacketProjectionV3Dto) -> DrillPacketEvidenceView<'_> {
    match projection {
        PacketProjectionV3Dto::Complete {
            status,
            evidence,
            gaps,
            continuation,
            ..
        } => DrillPacketEvidenceView {
            availability: status,
            evidence_count: evidence.as_slice().len(),
            gaps: gaps.as_slice(),
            continuation: continuation.as_ref(),
        },
        PacketProjectionV3Dto::BudgetExceeded { status, gaps, .. } => DrillPacketEvidenceView {
            availability: status,
            evidence_count: 0,
            gaps: gaps.as_slice(),
            continuation: None,
        },
    }
}

fn drill_summary_evidence_review(
    output: &DrillOutput,
    packet: &DrillPacketEvidenceView<'_>,
    stale_freshness: bool,
) -> DrillSummaryEvidenceReviewOutput {
    let mut target_files: Vec<_> = output
        .verification_targets
        .iter()
        .map(|target| target.path.clone())
        .collect();
    dedupe_and_rank_drill_files(&mut target_files);
    let target_file_count = target_files.len();
    let target_file_details =
        drill_summary_evidence_target_details(&target_files, &output.verification_targets);
    let continuation_gap_count = packet
        .gaps
        .iter()
        .filter(|gap| gap.kind == GapKindV3Dto::ContinuationRequired)
        .count();
    let follow_up_required = packet.availability != &EvidenceAvailabilityV3Dto::Available
        || !packet.gaps.is_empty()
        || packet.continuation.is_some()
        || stale_freshness;
    DrillSummaryEvidenceReviewOutput {
        follow_up_required,
        evidence_count: packet.evidence_count,
        gap_count: packet.gaps.len(),
        continuation_gap_count,
        target_file_count,
        target_files,
        target_file_details,
        pending_target_count: usize::from(follow_up_required) * target_file_count,
    }
}

fn drill_summary_open_gaps(
    packet: &DrillPacketEvidenceView<'_>,
    evidence_review: &DrillSummaryEvidenceReviewOutput,
    stale_freshness: bool,
) -> DrillSummaryOpenGapsOutput {
    DrillSummaryOpenGapsOutput {
        availability_status: packet.availability.clone(),
        evidence_count: packet.evidence_count,
        gap_count: packet.gaps.len(),
        continuation_gap_count: evidence_review.continuation_gap_count,
        pending_target_count: evidence_review.pending_target_count,
        continuation_available: packet.continuation.is_some(),
        stale_freshness,
        status: if !packet.gaps.is_empty() {
            "gaps_reported".to_string()
        } else if packet.continuation.is_some() {
            "continuation_reported".to_string()
        } else if stale_freshness {
            "stale_observation".to_string()
        } else {
            "no_open_gaps_reported".to_string()
        },
    }
}

pub(super) fn drill_bridge_status_is_graph(status: &str) -> bool {
    matches!(
        status,
        "graph_path" | "reverse_graph_path" | "graph_shared_file"
    )
}

pub(super) fn drill_bridge_status_is_partial(status: &str) -> bool {
    status == "evidence_hint_only"
}

pub(super) fn drill_bridge_status_is_unresolved(status: &str) -> bool {
    matches!(status, "no_bridge_found" | "unresolved_anchor" | "error")
}
