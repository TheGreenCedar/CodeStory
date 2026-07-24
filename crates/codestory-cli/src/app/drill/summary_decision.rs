use super::super::resolution::quote_command_path;
use super::summary_evidence::drill_bridge_status_is_graph;
use crate::args::{
    DrillOutput, DrillSummarySourceTruthTargetOutput, DrillSummaryStatsOutput,
    DrillSummaryVerdictOutput, VerificationTargetOutput,
};
use codestory_contracts::api::{IndexFreshnessDto, IndexFreshnessStatusDto};
use std::fmt::Write as _;

pub(super) struct DrillVerdictEvidence {
    pub(super) resolved_anchors: usize,
    pub(super) graph_path_bridges: usize,
    pub(super) partial_bridges: usize,
    pub(super) unresolved_or_error_bridges: usize,
    pub(super) needs_source_truth: bool,
    pub(super) open_gap_friendly: bool,
    pub(super) stale_freshness: bool,
}

pub(super) fn drill_summary_verdict(
    output: &DrillOutput,
    evidence: DrillVerdictEvidence,
) -> DrillSummaryVerdictOutput {
    let DrillVerdictEvidence {
        resolved_anchors,
        graph_path_bridges,
        partial_bridges,
        unresolved_or_error_bridges,
        needs_source_truth,
        open_gap_friendly,
        stale_freshness,
    } = evidence;
    let failed_anchor_commands = output
        .anchors
        .iter()
        .flat_map(|anchor| anchor.commands.iter())
        .filter(|command| command.status != "ok")
        .count();
    let unresolved_anchors = output.anchors.len().saturating_sub(resolved_anchors);
    if output.mechanical.after_files == 0 || output.mechanical.after_errors > 0 {
        return DrillSummaryVerdictOutput {
            status: "blocked".to_string(),
            reason: "index is not ready or contains indexing errors".to_string(),
            next_action: "inspect doctor/index output before trusting drill evidence".to_string(),
        };
    }
    if unresolved_anchors > 0 || failed_anchor_commands > 0 {
        return DrillSummaryVerdictOutput {
            status: "blocked".to_string(),
            reason: format!(
                "unresolved_anchors={unresolved_anchors} failed_anchor_commands={failed_anchor_commands}"
            ),
            next_action: "repair anchor selection or inspect command errors before answering"
                .to_string(),
        };
    }
    if stale_freshness {
        return DrillSummaryVerdictOutput {
            status: "degraded".to_string(),
            reason: format!(
                "index_freshness=stale source_truth_required={} graph_bridges={graph_path_bridges}/{} partial_bridges={partial_bridges} unresolved_or_error_bridges={unresolved_or_error_bridges} pending_source_truth_checks={}",
                needs_source_truth,
                output.bridges.len(),
                output.verification_targets.len()
            ),
            next_action: drill_stale_freshness_next_action(output),
        };
    }
    if needs_source_truth || open_gap_friendly || unresolved_or_error_bridges > 0 {
        return DrillSummaryVerdictOutput {
            status: "degraded".to_string(),
            reason: format!(
                "source_truth_required={} graph_bridges={graph_path_bridges}/{} partial_bridges={partial_bridges} unresolved_or_error_bridges={unresolved_or_error_bridges} pending_source_truth_checks={}",
                needs_source_truth,
                output.bridges.len(),
                output.verification_targets.len()
            ),
            next_action: drill_degraded_next_action(output, unresolved_or_error_bridges),
        };
    }
    DrillSummaryVerdictOutput {
        status: "ready".to_string(),
        reason: "all anchors resolved and no open source-truth blockers were reported".to_string(),
        next_action: "answer from the evidence packet and keep source verification focused"
            .to_string(),
    }
}

pub(super) fn drill_stale_freshness_next_action(output: &DrillOutput) -> String {
    let project = quote_command_path(std::path::Path::new(&output.project));
    let mut action = format!(
        "refresh stale index evidence first with `codestory-cli index --project {project} --refresh incremental`, then rerun drill before finalizing"
    );
    if let Some(freshness) = output.mechanical.freshness.as_ref() {
        let samples = freshness
            .samples
            .iter()
            .take(3)
            .map(|sample| sample.path.clone())
            .collect::<Vec<_>>();
        if !samples.is_empty() {
            let _ = write!(action, "; stale samples: {}", samples.join("; "));
        }
    }
    action
}

pub(super) fn drill_degraded_next_action(
    output: &DrillOutput,
    unresolved_or_error_bridges: usize,
) -> String {
    let failed_bridge_count = output
        .bridges
        .iter()
        .filter(|bridge| bridge.command.status != "ok" || bridge.evidence.status == "error")
        .count();
    if failed_bridge_count > 0 {
        return format!(
            "repair or rerun {failed_bridge_count} failed bridge evidence command(s) before treating degraded bridges as verification targets"
        );
    }
    let degraded_bridge_count = output
        .bridges
        .iter()
        .filter(|bridge| !drill_bridge_status_is_graph(&bridge.evidence.status))
        .count()
        .max(unresolved_or_error_bridges);
    let mut files = output
        .verification_targets
        .iter()
        .map(|target| target.path.clone())
        .collect::<Vec<_>>();
    dedupe_and_rank_drill_files(&mut files);

    let mut action = "write a CodeStory-only draft".to_string();
    let pending_claim_count = output.evidence_packet.sufficiency.gaps.len();
    if pending_claim_count > 0 && degraded_bridge_count > 0 {
        let _ = write!(
            action,
            ", then verify {pending_claim_count} pending claim(s), starting with {degraded_bridge_count} degraded bridge(s)"
        );
    } else if pending_claim_count > 0 {
        let _ = write!(
            action,
            ", then verify {pending_claim_count} pending claim(s)"
        );
    } else if degraded_bridge_count > 0 {
        let _ = write!(
            action,
            ", then verify {degraded_bridge_count} degraded bridge(s)"
        );
    } else {
        action.push_str(", then verify source-truth targets");
    }
    if !files.is_empty() {
        let preview = files.into_iter().take(3).collect::<Vec<_>>().join("; ");
        let _ = write!(action, " including {preview}");
    }
    if !output
        .evidence_packet
        .sufficiency
        .follow_up_commands
        .is_empty()
    {
        action.push_str("; use emitted packet follow-up commands before finalizing");
    }
    action
}

pub(super) fn drill_summary_stats(
    files: u32,
    nodes: u32,
    edges: u32,
    errors: u32,
) -> DrillSummaryStatsOutput {
    DrillSummaryStatsOutput {
        files,
        nodes,
        edges,
        errors,
    }
}

pub(super) fn drill_summary_retrieval_status(
    retrieval: &codestory_contracts::api::RetrievalStateDto,
    sidecar_retrieval_mode: Option<&str>,
) -> String {
    if let Some(mode) = sidecar_retrieval_mode {
        if mode == "full" {
            return "full".to_string();
        }
        return format!(
            "{mode}:retrieval_degraded; legacy={}",
            drill_summary_legacy_retrieval_status(retrieval)
        );
    }
    drill_summary_legacy_retrieval_status(retrieval)
}

pub(super) fn drill_summary_legacy_retrieval_status(
    retrieval: &codestory_contracts::api::RetrievalStateDto,
) -> String {
    let mode = match retrieval.mode {
        codestory_contracts::api::RetrievalModeDto::Hybrid => "hybrid",
        codestory_contracts::api::RetrievalModeDto::Symbolic => "symbolic",
    };
    let readiness = if retrieval.semantic_ready {
        "semantic_ready"
    } else {
        "semantic_unavailable"
    };
    match retrieval.fallback_reason {
        Some(reason) => format!("{mode}:{readiness}:diagnostic={reason:?}"),
        None => format!("{mode}:{readiness}"),
    }
}

pub(super) fn drill_suite_retrieval_label(status: Option<&str>) -> &str {
    match status {
        Some("full") => "full",
        Some(value) if value.contains("retrieval_degraded") => "needs-retrieval-refresh",
        Some(value) if value.contains("semantic_ready") || value == "hybrid-ready" => "degraded",
        Some(value) if value.contains("semantic_unavailable") => "needs-retrieval-refresh",
        Some("hybrid") => "degraded",
        Some("symbolic") => "needs-retrieval-refresh",
        Some(_) => "partial",
        None => "unknown",
    }
}

pub(super) fn drill_summary_source_truth_target_details(
    target_files: &[String],
    targets: &[VerificationTargetOutput],
) -> Vec<DrillSummarySourceTruthTargetOutput> {
    target_files
        .iter()
        .map(|path| {
            let check_reasons = targets
                .iter()
                .filter(|target| normalize_drill_path(&target.path) == normalize_drill_path(path))
                .map(|target| target.reason.clone())
                .collect::<Vec<_>>();
            let role = drill_source_truth_target_role(path, &check_reasons);
            DrillSummarySourceTruthTargetOutput {
                path: path.clone(),
                role: role.clone(),
                rank_reason: drill_source_truth_target_rank_reason(path, &role),
                check_reasons,
            }
        })
        .collect()
}

pub(super) fn normalize_drill_path(path: &str) -> String {
    path.replace('\\', "/").to_ascii_lowercase()
}

pub(super) fn drill_path_is_framework_route_or_page(path: &str) -> bool {
    let normalized = normalize_drill_path(path);
    normalized.ends_with("/route.ts")
        || normalized.ends_with("/route.tsx")
        || normalized.ends_with("/route.js")
        || normalized.ends_with("/route.jsx")
        || normalized.ends_with("/page.tsx")
        || normalized.ends_with("/page.jsx")
        || ((normalized.contains("/app/") || normalized.contains("/pages/"))
            && (normalized.ends_with(".tsx") || normalized.ends_with(".jsx")))
}

pub(super) fn drill_source_truth_target_role(path: &str, reasons: &[String]) -> String {
    let path = normalize_drill_path(path);
    let reason_text = reasons.join(" ").to_ascii_lowercase();
    if drill_path_is_framework_route_or_page(&path) {
        return "public_surface".to_string();
    }
    if path.contains("/components/") && !path.contains("/components/admin") {
        return "runtime_entrypoint".to_string();
    }
    if path.contains("/collections/") || reason_text.contains("collection") {
        return "data_store".to_string();
    }
    if path.contains("comment-auth") || reason_text.contains("auth") {
        return "comment_auth".to_string();
    }
    if path.contains("/tests/") || path.contains(".spec.") || path.contains(".test.") {
        return "test_support".to_string();
    }
    if path.contains("/admin/") || path.contains("/components/admin") {
        return "admin_support".to_string();
    }
    if drill_bridge_evidence_is_generated_path(&format!("/{path}")) {
        return "generated_or_auxiliary".to_string();
    }
    "anchor_definition".to_string()
}

pub(super) fn drill_source_truth_target_rank_reason(path: &str, role: &str) -> String {
    match role {
        "public_surface" => "ranked ahead as public runtime surface evidence".to_string(),
        "runtime_entrypoint" => "ranked ahead as runtime/component evidence".to_string(),
        "data_store" => "kept as Payload/data-store evidence".to_string(),
        "comment_auth" => "kept as comment authentication evidence".to_string(),
        "test_support" => "demoted behind runtime evidence as test support".to_string(),
        "admin_support" => "demoted behind public runtime evidence as admin support".to_string(),
        "generated_or_auxiliary" => {
            "demoted behind source files as generated or auxiliary evidence".to_string()
        }
        _ if normalize_drill_path(path).contains("/src/") => {
            "ranked as production source evidence".to_string()
        }
        _ => "ranked after primary source surfaces".to_string(),
    }
}

pub(super) fn drill_summary_freshness_status(freshness: &IndexFreshnessDto) -> String {
    match freshness.status {
        IndexFreshnessStatusDto::Fresh => "fresh".to_string(),
        IndexFreshnessStatusDto::Stale => "stale".to_string(),
        IndexFreshnessStatusDto::NotChecked => "not_checked".to_string(),
    }
}

pub(super) fn drill_summary_stale_file_count(freshness: &IndexFreshnessDto) -> u32 {
    if freshness.status == IndexFreshnessStatusDto::Stale {
        freshness
            .changed_file_count
            .saturating_add(freshness.new_file_count)
            .saturating_add(freshness.removed_file_count)
    } else {
        0
    }
}

pub(super) fn drill_summary_freshness_samples(freshness: &IndexFreshnessDto) -> Vec<String> {
    freshness
        .samples
        .iter()
        .take(8)
        .map(|sample| format!("{:?}: {}", sample.kind, sample.path))
        .collect()
}

pub(super) fn ensure_trailing_newline(mut content: String) -> String {
    if !content.ends_with('\n') {
        content.push('\n');
    }
    content
}

pub(super) fn output_slug(value: &str) -> String {
    let slug = value.chars().fold(String::new(), |mut slug, ch| {
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
            slug.push(ch);
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
        slug
    });
    let slug = slug.trim_matches('-');
    if slug.is_empty() {
        "anchor".to_string()
    } else {
        slug.to_string()
    }
}

pub(super) fn dedupe_and_rank_drill_files(files: &mut Vec<String>) {
    files.sort_by_cached_key(|path| normalize_drill_path(path));
    files.dedup_by(|left, right| normalize_drill_path(left) == normalize_drill_path(right));
}

pub(super) fn drill_bridge_evidence_is_generated_path(normalized_with_root: &str) -> bool {
    normalized_with_root.contains("/target/")
        || normalized_with_root.contains("/dist/")
        || normalized_with_root.contains("/build/")
        || normalized_with_root.contains("/node_modules/")
}
