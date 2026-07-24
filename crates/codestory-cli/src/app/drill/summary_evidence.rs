use super::super::packet_sufficiency_label;
use super::summary_decision::{
    DrillVerdictEvidence, dedupe_and_rank_drill_files, drill_summary_freshness_samples,
    drill_summary_freshness_status, drill_summary_retrieval_status,
    drill_summary_source_truth_target_details, drill_summary_stale_file_count, drill_summary_stats,
    drill_summary_verdict,
};
use crate::args::{
    DrillAnchorOutput, DrillOutput, DrillSummaryAnchorStatusOutput, DrillSummaryAnchorsOutput,
    DrillSummaryBridgeStatusOutput, DrillSummaryBridgesOutput, DrillSummaryMechanicalOutput,
    DrillSummaryOpenGapsOutput, DrillSummaryOutput, DrillSummarySourceTruthOutput,
};
use codestory_contracts::api::{
    ClaimReadinessDto, IndexFreshnessStatusDto, PacketProofStatusDto, PacketSufficiencyStatusDto,
};

pub(super) fn drill_summary(output: &DrillOutput) -> DrillSummaryOutput {
    let anchors = drill_summary_anchors(output);
    let bridges = drill_summary_bridges(output);
    let source_truth = drill_summary_source_truth(output);
    let stale_freshness = output
        .mechanical
        .freshness
        .as_ref()
        .is_some_and(|freshness| freshness.status == IndexFreshnessStatusDto::Stale);
    let open_gaps = drill_summary_open_gaps(output, &source_truth, stale_freshness);
    let verdict = drill_summary_verdict(
        output,
        DrillVerdictEvidence {
            resolved_anchors: anchors.resolved,
            graph_path_bridges: bridges.graph_path,
            partial_bridges: bridges.partial,
            unresolved_or_error_bridges: bridges.unresolved_or_error,
            needs_source_truth: source_truth.required,
            open_gap_friendly: open_gaps.open_gap_friendly,
            stale_freshness,
        },
    );

    DrillSummaryOutput {
        summary_version: 1,
        project: output.project.clone(),
        label: output.label.clone(),
        question: output.question.clone(),
        output_dir: output.output_dir.clone(),
        full_report_json: "drill-report.json".to_string(),
        full_report_markdown: "drill-report.md".to_string(),
        mechanical: drill_summary_mechanical(output),
        anchors,
        bridges,
        source_truth,
        open_gaps,
        verdict,
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
        index_ready: output.mechanical.after_files > 0 && output.mechanical.after_errors == 0,
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
        source_truth_target_count: anchor.verification_targets.len(),
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
            drill_bridge_status_is_unresolved(&bridge.status) || bridge.command_status != "ok"
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

fn drill_summary_source_truth(output: &DrillOutput) -> DrillSummarySourceTruthOutput {
    let sufficiency = &output.evidence_packet.sufficiency;
    let mut target_files: Vec<_> = output
        .verification_targets
        .iter()
        .map(|target| target.path.clone())
        .collect();
    dedupe_and_rank_drill_files(&mut target_files);
    let target_file_count = target_files.len();
    let target_file_details =
        drill_summary_source_truth_target_details(&target_files, &output.verification_targets);
    let has_source_truth_checks = !target_files.is_empty();
    let needs_source_truth = sufficiency.status != PacketSufficiencyStatusDto::Sufficient;
    DrillSummarySourceTruthOutput {
        required: needs_source_truth,
        check_count: target_file_count,
        pending_check_count: if has_source_truth_checks {
            usize::from(needs_source_truth) * target_file_count
        } else {
            0
        },
        verified_check_count: if needs_source_truth {
            0
        } else {
            target_file_count
        },
        target_file_count,
        target_files,
        target_file_details,
        checklist_item_count: 0,
        claim_count: sufficiency.covered_claims.len(),
        pending_claim_count: sufficiency.gaps.len(),
        verified_claim_count: sufficiency.covered_claims.len(),
    }
}

fn drill_summary_open_gaps(
    output: &DrillOutput,
    source_truth: &DrillSummarySourceTruthOutput,
    stale_freshness: bool,
) -> DrillSummaryOpenGapsOutput {
    let sufficiency = &output.evidence_packet.sufficiency;
    let open_gap_friendly = !sufficiency.gaps.is_empty()
        || !sufficiency.open_next.is_empty()
        || source_truth.required
        || stale_freshness;
    DrillSummaryOpenGapsOutput {
        overall_status: drill_packet_claim_readiness(sufficiency.status),
        answer_quality_status: packet_sufficiency_label(sufficiency.status).to_string(),
        safe_to_say_count: sufficiency.covered_claims.len(),
        inferred_claim_count: sufficiency
            .covered_claims
            .iter()
            .filter(|claim| claim.proof_status != Some(PacketProofStatusDto::Proven))
            .count(),
        needs_verification_count: sufficiency.gaps.len(),
        needs_verification_claim_count: sufficiency.gaps.len(),
        pending_claim_count: if source_truth.required {
            sufficiency.gaps.len()
        } else {
            0
        },
        pending_source_truth_check_count: if source_truth.required {
            source_truth.target_file_count
        } else {
            0
        },
        next_command_count: sufficiency.follow_up_commands.len(),
        open_gap_friendly,
        status: if open_gap_friendly {
            "open_gaps_explicit".to_string()
        } else {
            "no_open_gaps_reported".to_string()
        },
    }
}

pub(in crate::app) fn drill_packet_claim_readiness(
    status: PacketSufficiencyStatusDto,
) -> ClaimReadinessDto {
    match status {
        PacketSufficiencyStatusDto::Sufficient => ClaimReadinessDto::Supported,
        PacketSufficiencyStatusDto::Partial => ClaimReadinessDto::Partial,
        PacketSufficiencyStatusDto::Insufficient => ClaimReadinessDto::NeedsSourceRead,
    }
}

pub(super) fn drill_bridge_status_is_graph(status: &str) -> bool {
    matches!(
        status,
        "graph_path" | "reverse_graph_path" | "graph_shared_file"
    )
}

pub(super) fn drill_bridge_status_is_partial(status: &str) -> bool {
    matches!(
        status,
        "shared_file_only"
            | "evidence_hint_only"
            | "framework_route"
            | "component_usage"
            | "data_collection_usage"
            | "source_truth_only"
    )
}

pub(super) fn drill_bridge_status_is_unresolved(status: &str) -> bool {
    matches!(status, "no_bridge_found" | "unresolved_anchor" | "error")
}
