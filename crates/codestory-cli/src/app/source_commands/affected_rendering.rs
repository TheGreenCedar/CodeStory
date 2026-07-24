use super::super::resolution::quote_command_argument_value;
use codestory_contracts::api::{AffectedFollowUpInvocationDto, FrameworkRouteCoverageDto};
use std::fmt::Write as _;

pub(super) fn render_files_markdown(output: &codestory_contracts::api::IndexedFilesDto) -> String {
    let mut markdown = String::new();
    markdown.push_str("# indexed files\n\n");
    render_files_summary(&mut markdown, output);
    render_framework_route_coverage(&mut markdown, output);
    render_source_policy_exclusions(&mut markdown, output);
    render_indexed_file_rows(&mut markdown, output);
    markdown
}

pub(super) fn render_files_summary(
    markdown: &mut String,
    output: &codestory_contracts::api::IndexedFilesDto,
) {
    let status = if output.usable { "usable" } else { "empty" };
    let _ = writeln!(
        markdown,
        "- index: {status}; whole index files: {}; indexed: {}; incomplete: {}; error files: {}; policy exclusions: {}; filtered files: {}; visible rows: {}; truncated: {}",
        output.summary.file_count,
        output.summary.indexed_file_count,
        output.summary.incomplete_file_count,
        output.summary.error_file_count,
        output.summary.policy_exclusion_count,
        output.summary.filtered_file_count,
        output.summary.visible_file_count,
        output.summary.truncated
    );
    if !output.summary.language_counts.is_empty() {
        let languages = output
            .summary
            .language_counts
            .iter()
            .map(|entry| {
                format!(
                    "{}={} [{}; {}]",
                    entry.language, entry.file_count, entry.support_mode, entry.evidence_tier
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(markdown, "- languages: {languages}");
        let claim_labels = output
            .summary
            .language_counts
            .iter()
            .map(|entry| format!("{}={}", entry.language, entry.claim_label))
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(markdown, "- language_support_claims: {claim_labels}");
    }
    if !output.summary.incomplete_reason_counts.is_empty() {
        let reasons = output
            .summary
            .incomplete_reason_counts
            .iter()
            .map(|entry| format!("{}={} ({})", entry.reason, entry.file_count, entry.detail))
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(markdown, "- incomplete_reasons: {reasons}");
    }
    for note in &output.summary.coverage_notes {
        let _ = writeln!(markdown, "- coverage: {note}");
    }
}

pub(super) fn render_source_policy_exclusions(
    markdown: &mut String,
    output: &codestory_contracts::api::IndexedFilesDto,
) {
    if output.policy_exclusions.is_empty() {
        return;
    }
    markdown.push_str(
        "\nverified policy exclusions (source inventory only; no graph or semantic coverage):\n",
    );
    for exclusion in &output.policy_exclusions {
        let _ = writeln!(
            markdown,
            "- {} ({:?}, {} bytes, {} structural units, policy={} byte_cap={} unit_cap={}, core={}/{})",
            exclusion.path,
            exclusion.role,
            exclusion.observed_size,
            exclusion.observed_unit_count,
            exclusion.policy_version,
            exclusion.byte_cap,
            exclusion.structural_unit_cap,
            exclusion.core_generation_id,
            exclusion.core_run_id,
        );
    }
}

pub(super) fn render_framework_route_coverage(
    markdown: &mut String,
    output: &codestory_contracts::api::IndexedFilesDto,
) {
    if !output.summary.framework_route_coverage.is_empty() {
        markdown.push_str("\nframework route coverage:\n");
        for entry in &output.summary.framework_route_coverage {
            let _ = writeln!(markdown, "{}", framework_route_coverage_row(entry));
        }
    }
}

pub(super) fn framework_route_coverage_row(entry: &FrameworkRouteCoverageDto) -> String {
    format!(
        "- {} ({}) status={} coverage_evidence={} confidence_floor={} handler_link={} promotable={} unsupported={} known_gaps={}",
        entry.framework,
        entry.language,
        entry.status,
        entry.coverage_evidence,
        entry.confidence_floor,
        entry.handler_link_support,
        entry.promotable,
        joined_or_none_recorded(&entry.unsupported_patterns),
        joined_or_none_recorded(&entry.known_gaps)
    )
}

pub(super) fn joined_or_none_recorded(values: &[String]) -> String {
    if values.is_empty() {
        "none recorded".to_string()
    } else {
        values.join("; ")
    }
}

pub(super) fn render_indexed_file_rows(
    markdown: &mut String,
    output: &codestory_contracts::api::IndexedFilesDto,
) {
    markdown.push_str("\nfiles:\n");
    for file in &output.files {
        let markers = [
            (!file.indexed).then_some("not-indexed"),
            (!file.complete).then_some("incomplete"),
            (file.error_count > 0).then_some("errors"),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
        let marker = if markers.is_empty() {
            String::new()
        } else {
            format!(" [{}]", markers.join(", "))
        };
        let _ = writeln!(
            markdown,
            "- {} ({}, {:?}, {} lines){}",
            file.path, file.language, file.role, file.line_count, marker
        );
    }
    if output.summary.truncated {
        markdown.push_str("- ... truncated by limit\n");
    }
}

pub(super) fn render_affected_markdown(
    output: &codestory_contracts::api::AffectedAnalysisDto,
) -> String {
    let mut markdown = String::new();
    markdown.push_str("# affected analysis\n\n");
    render_affected_summary(&mut markdown, output);
    render_affected_matched_files(&mut markdown, output);
    render_affected_routes(&mut markdown, output);
    render_affected_tests(&mut markdown, output);
    render_affected_symbols(&mut markdown, output);
    render_affected_footer(&mut markdown, output);
    markdown
}

pub(super) fn render_affected_summary(
    markdown: &mut String,
    output: &codestory_contracts::api::AffectedAnalysisDto,
) {
    let _ = writeln!(
        markdown,
        "- matched files: {}; depth: {}; impacted symbols: {}; impacted routes: {}; impacted tests: {}",
        output.matched_file_count,
        output.depth,
        output.impacted_symbols.len(),
        output.impacted_routes.len(),
        output.impacted_tests.len()
    );
    let _ = writeln!(
        markdown,
        "- completeness: complete={} confidence={} direct={} propagated={} uncovered={} unavailable={} truncated={}",
        output.completeness.complete,
        output.completeness.confidence,
        output.completeness.direct_impact_count,
        output.completeness.propagated_impact_count,
        output.completeness.uncovered_input_count,
        output.completeness.unavailable_evidence_count,
        output.completeness.truncated
    );
    let _ = writeln!(
        markdown,
        "- bounds: requested_depth={} maximum_depth={} visited_nodes={} visited_edges={} symbol_limit={} route_limit={}",
        output.bounds.requested_depth,
        output.bounds.maximum_depth,
        output.bounds.visited_node_count,
        output.bounds.visited_edge_count,
        output.bounds.impacted_symbol_limit,
        output.bounds.impacted_route_limit
    );
    if !output.changed_paths.is_empty() {
        markdown.push_str("- changed paths:\n");
        for path in &output.changed_paths {
            let _ = writeln!(markdown, "  - {path}");
        }
    }
    if !output.change_records.is_empty() {
        markdown.push_str("- change records:\n");
        for record in &output.change_records {
            let previous = record
                .previous_path
                .as_deref()
                .map(|path| format!(" previous={path}"))
                .unwrap_or_default();
            let _ = writeln!(
                markdown,
                "  - {:?} {} status={}{}",
                record.kind, record.path, record.status, previous
            );
        }
    }
    for note in &output.notes {
        let _ = writeln!(markdown, "- note: {note}");
    }
}

pub(super) fn render_affected_matched_files(
    markdown: &mut String,
    output: &codestory_contracts::api::AffectedAnalysisDto,
) {
    if !output.matched_files.is_empty() {
        markdown.push_str("\nmatched files:\n");
        for file in &output.matched_files {
            let mut markers = Vec::new();
            if !file.complete {
                markers.push("incomplete".to_string());
            }
            if file.error_count > 0 {
                markers.push(format!("errors={}", file.error_count));
            }
            if let Some(kind) = file.change_kind.as_ref() {
                markers.push(format!("change={kind:?}"));
            }
            if let Some(status) = file.change_status.as_deref() {
                markers.push(format!("status={status}"));
            }
            if let Some(previous_path) = file.previous_path.as_deref() {
                markers.push(format!("previous={previous_path}"));
            }
            let marker = if markers.is_empty() {
                String::new()
            } else {
                format!(" ({})", markers.join(", "))
            };
            let _ = writeln!(markdown, "- {} [{:?}]{marker}", file.path, file.role);
        }
    }
    if !output.unmatched_paths.is_empty() {
        markdown.push_str("\nunmatched paths:\n");
        for path in &output.unmatched_paths {
            let mut markers = vec![format!("classification={:?}", path.classification)];
            if let Some(kind) = path.change_kind.as_ref() {
                markers.push(format!("change={kind:?}"));
            }
            if let Some(status) = path.change_status.as_deref() {
                markers.push(format!("status={status}"));
            }
            if let Some(previous_path) = path.previous_path.as_deref() {
                markers.push(format!("previous={previous_path}"));
            }
            let marker = if markers.is_empty() {
                String::new()
            } else {
                format!(" ({})", markers.join(", "))
            };
            let _ = writeln!(markdown, "- {}{marker}: {}", path.path, path.reason);
        }
    }
}

pub(super) fn render_affected_routes(
    markdown: &mut String,
    output: &codestory_contracts::api::AffectedAnalysisDto,
) {
    if !output.impacted_routes.is_empty() {
        markdown.push_str("\nimpacted routes:\n");
        for route in output.impacted_routes.iter().take(30) {
            let handler = route
                .route
                .handler
                .as_ref()
                .map(|handler| format!(" handler={}", handler.display_name))
                .unwrap_or_default();
            let framework = route
                .route
                .framework
                .as_deref()
                .map(|framework| format!(" framework={framework}"))
                .unwrap_or_default();
            let _ = writeln!(
                markdown,
                "- d{} {} {}{}{} [{}]: {}",
                route.graph_depth,
                route.route.method,
                route.route.path,
                framework,
                handler,
                route.confidence,
                route.reason
            );
        }
    }
}

pub(super) fn render_affected_tests(
    markdown: &mut String,
    output: &codestory_contracts::api::AffectedAnalysisDto,
) {
    if !output.impacted_tests.is_empty() {
        markdown.push_str("\nlikely impacted tests:\n");
        for test in &output.impacted_tests {
            let _ = writeln!(
                markdown,
                "- d{} {} ({} symbols, {}): {}",
                test.graph_depth,
                test.path,
                test.impacted_symbol_count,
                test.confidence,
                test.reason
            );
        }
    }
}

pub(super) fn render_affected_symbols(
    markdown: &mut String,
    output: &codestory_contracts::api::AffectedAnalysisDto,
) {
    markdown.push_str("\nimpacted symbols:\n");
    for symbol in output.impacted_symbols.iter().take(40) {
        let location = symbol
            .file_path
            .as_deref()
            .map(|path| match symbol.line {
                Some(line) => format!("{path}:{line}"),
                None => path.to_string(),
            })
            .unwrap_or_else(|| "unknown".to_string());
        let _ = writeln!(
            markdown,
            "- d{} {} [{:?}] at {} ({}, {}): {}",
            symbol.graph_depth,
            symbol.display_name,
            symbol.kind,
            location,
            symbol.node_id.0,
            symbol.confidence,
            symbol.reason
        );
    }
    if output.impacted_symbols.len() > 40 {
        let _ = writeln!(
            markdown,
            "- ... {} more symbols omitted",
            output.impacted_symbols.len() - 40
        );
    }
}

pub(super) fn render_affected_invocation(invocation: &AffectedFollowUpInvocationDto) -> String {
    std::iter::once(invocation.program.clone())
        .chain(
            invocation
                .args
                .iter()
                .map(|arg| quote_command_argument_value(arg)),
        )
        .collect::<Vec<_>>()
        .join(" ")
}

pub(super) fn render_affected_footer(
    markdown: &mut String,
    output: &codestory_contracts::api::AffectedAnalysisDto,
) {
    if !output.blind_spots.is_empty() {
        markdown.push_str("\nblind spots:\n");
        for blind_spot in &output.blind_spots {
            let _ = writeln!(markdown, "- {blind_spot}");
        }
    }
    if !output.follow_ups.is_empty() {
        markdown.push_str("\nfollow-ups:\n");
        for follow_up in &output.follow_ups {
            let invocation = follow_up
                .invocation
                .as_ref()
                .map(render_affected_invocation)
                .map(|invocation| format!(" invocation=`{invocation}`"))
                .unwrap_or_default();
            let _ = writeln!(
                markdown,
                "- {} [{}]: {}{}",
                follow_up.action, follow_up.confidence, follow_up.reason, invocation
            );
        }
    }
}
