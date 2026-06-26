use anyhow::{Result, bail};
use std::fmt::Write as _;

use codestory_runtime::graph_analysis::{RepoReport, RepoReportHandoff, ReportNodeSummary};

use crate::args::{OutputFormat, ReportCommand, ReportProfile};
use crate::display::clean_path_string;
use crate::output::{emit, validate_output_file_parent};
use crate::runtime::{RuntimeContext, ensure_index_ready, map_cache_busy_anyhow};

pub(crate) fn run_report(cmd: ReportCommand) -> Result<()> {
    if matches!(cmd.format, OutputFormat::Dot) {
        bail!("--format dot is only supported by `trail`; use markdown or json");
    }
    if let Some(path) = cmd.output_file.as_deref() {
        validate_output_file_parent(path)?;
    }

    let runtime = RuntimeContext::new_inspect_only(&cmd.project)?;
    let opened = runtime.ensure_open(crate::args::RefreshMode::None)?;
    ensure_index_ready(&opened, "report")?;
    let sidecar = report_sidecar_status(&runtime);
    match cmd.format {
        OutputFormat::Markdown => {
            let mut output = codestory_runtime::graph_analysis::build_report(
                &runtime.project_root,
                &runtime.storage_path,
                cmd.limit,
            )
            .map_err(|error| map_cache_busy_anyhow(error, &runtime.project_root))?;
            attach_report_handoff(&mut output, &opened.summary, &sidecar);
            let markdown = render_report_markdown(&output, cmd.profile);
            emit(cmd.format, &output, markdown, cmd.output_file.as_deref())
        }
        OutputFormat::Json => {
            let mut output = codestory_runtime::graph_analysis::build_report_export(
                &runtime.project_root,
                &runtime.storage_path,
                cmd.limit,
            )
            .map_err(|error| map_cache_busy_anyhow(error, &runtime.project_root))?;
            attach_report_handoff(&mut output.report, &opened.summary, &sidecar);
            let markdown = render_report_markdown(&output.report, cmd.profile);
            emit(cmd.format, &output, markdown, cmd.output_file.as_deref())
        }
        OutputFormat::Dot => {
            bail!("--format dot is only supported by `trail`; use markdown or json")
        }
    }
}

fn render_report_markdown(output: &RepoReport, profile: ReportProfile) -> String {
    let mut markdown = String::new();
    let _ = writeln!(markdown, "# CodeStory Repo Report");
    let _ = writeln!(
        markdown,
        "artifact: `{}` from `{}`; generated output, not source-of-truth state.",
        output.metadata.artifact_role, output.metadata.source
    );
    let _ = writeln!(
        markdown,
        "project: `{}`",
        clean_path_string(&output.metadata.project_root)
    );
    let _ = writeln!(
        markdown,
        "storage: `{}`",
        clean_path_string(&output.metadata.storage_path)
    );
    let _ = writeln!(
        markdown,
        "generated_at_epoch_ms: `{}`",
        output.metadata.generated_at_epoch_ms
    );
    let _ = writeln!(markdown);

    append_handoff_header(&mut markdown, output);
    if profile == ReportProfile::Handoff {
        append_follow_ups(&mut markdown, output);
        return markdown;
    }

    append_summary(&mut markdown, output);
    append_node_section(&mut markdown, "Hotspots", &output.hotspots);
    append_node_section(&mut markdown, "Entry Points", &output.entry_points);
    append_node_section(
        &mut markdown,
        "Bridge / High-Connectivity Nodes",
        &output.bridge_nodes,
    );
    append_follow_ups(&mut markdown, output);
    let _ = writeln!(
        markdown,
        "JSON graph export: rerun with `--format json` to get the full current graph, including nodes, edges, confidence/certainty, source locations, and generation metadata."
    );
    markdown
}

#[derive(Debug, Clone)]
struct ReportSidecarStatus {
    retrieval_mode: String,
    degraded_reason: Option<String>,
    embedding_device_policy: String,
    embedding_device_state: String,
    embedding_device_observation_source: String,
    embedding_detected_provider: Option<String>,
    embedding_detected_gpu: Option<String>,
    embedding_accelerator_requested: bool,
    embedding_accelerator_request_provider: Option<String>,
    embedding_accelerator_request_device: Option<String>,
    embedding_cpu_allowed: bool,
    manifest_generation: Option<String>,
    manifest_input_hash: Option<String>,
}

fn report_sidecar_status(runtime: &RuntimeContext) -> ReportSidecarStatus {
    match codestory_retrieval::strict_sidecar_status(
        &runtime.project_root,
        Some(&runtime.storage_path),
    ) {
        Ok(report) => {
            let manifest_generation = report
                .manifest
                .as_ref()
                .and_then(|manifest| manifest.sidecar_generation.clone());
            let manifest_input_hash = report
                .manifest
                .as_ref()
                .and_then(|manifest| manifest.sidecar_input_hash.clone());
            ReportSidecarStatus {
                retrieval_mode: report.retrieval_mode,
                degraded_reason: report.degraded_reason,
                embedding_device_policy: report.embedding_device_policy,
                embedding_device_state: report.embedding_device_state,
                embedding_device_observation_source: report.embedding_device_observation_source,
                embedding_detected_provider: report.embedding_detected_provider,
                embedding_detected_gpu: report.embedding_detected_gpu,
                embedding_accelerator_requested: report.embedding_accelerator_requested,
                embedding_accelerator_request_provider: report
                    .embedding_accelerator_request_provider,
                embedding_accelerator_request_device: report.embedding_accelerator_request_device,
                embedding_cpu_allowed: report.embedding_cpu_allowed,
                manifest_generation,
                manifest_input_hash,
            }
        }
        Err(error) => ReportSidecarStatus {
            retrieval_mode: "unavailable".to_string(),
            degraded_reason: Some(format!("sidecar_status_error: {error}")),
            embedding_device_policy: "accelerator_required".to_string(),
            embedding_device_state: "unknown".to_string(),
            embedding_device_observation_source: "sidecar_unobserved".to_string(),
            embedding_detected_provider: None,
            embedding_detected_gpu: None,
            embedding_accelerator_requested: false,
            embedding_accelerator_request_provider: None,
            embedding_accelerator_request_device: None,
            embedding_cpu_allowed: false,
            manifest_generation: None,
            manifest_input_hash: None,
        },
    }
}

fn attach_report_handoff(
    output: &mut RepoReport,
    summary: &codestory_contracts::api::ProjectSummary,
    sidecar: &ReportSidecarStatus,
) {
    let readiness = crate::readiness::build_readiness_verdicts(crate::readiness::ReadinessInputs {
        project: &summary.root,
        stats: &summary.stats,
        freshness: summary.freshness.as_ref(),
        setup: None,
        sidecar: Some(crate::readiness::ReadinessSidecarInput {
            profile: Some("local"),
            run_id: None,
            retrieval_mode: &sidecar.retrieval_mode,
            degraded_reason: sidecar.degraded_reason.as_deref(),
            embedding_device_policy: Some(&sidecar.embedding_device_policy),
            embedding_device_state: Some(&sidecar.embedding_device_state),
            embedding_device_observation_source: Some(&sidecar.embedding_device_observation_source),
            embedding_detected_provider: sidecar.embedding_detected_provider.as_deref(),
            embedding_detected_gpu: sidecar.embedding_detected_gpu.as_deref(),
            embedding_accelerator_requested: sidecar.embedding_accelerator_requested,
            embedding_accelerator_request_provider: sidecar
                .embedding_accelerator_request_provider
                .as_deref(),
            embedding_accelerator_request_device: sidecar
                .embedding_accelerator_request_device
                .as_deref(),
            embedding_cpu_allowed: sidecar.embedding_cpu_allowed,
            manifest_generation: sidecar.manifest_generation.as_deref(),
            manifest_input_hash: sidecar.manifest_input_hash.as_deref(),
        }),
    });
    let next_command = crate::readiness::primary_non_ready(&readiness)
        .and_then(|verdict| verdict.minimum_next.first().cloned())
        .or_else(|| {
            output
                .follow_up_queries
                .first()
                .map(|query| query.command.clone())
        })
        .or_else(|| {
            crate::readiness::combined_minimum_next(&readiness)
                .into_iter()
                .next()
        });
    let trust_caveat = if crate::readiness::primary_non_ready(&readiness).is_some() {
        "Readiness is not fully green; run the next command before trusting agent packet/search output.".to_string()
    } else {
        "Generated from the current local store; treat it as a handoff snapshot, not source-of-truth state.".to_string()
    };
    output.metadata.handoff = Some(RepoReportHandoff {
        readiness,
        freshness: summary.freshness.clone(),
        sidecar_retrieval_mode: Some(sidecar.retrieval_mode.clone()),
        degraded_reason: sidecar.degraded_reason.clone(),
        trust_caveat,
        top_entry_point: output.entry_points.first().map(report_node_label),
        top_risk: output.hotspots.first().map(report_node_label),
        next_command,
    });
}

fn append_handoff_header(markdown: &mut String, output: &RepoReport) {
    let _ = writeln!(markdown, "## Read This First / Agent Handoff");
    let Some(handoff) = output.metadata.handoff.as_ref() else {
        let _ = writeln!(markdown, "- readiness: not attached");
        let _ = writeln!(markdown);
        return;
    };
    for verdict in &handoff.readiness {
        let _ = writeln!(
            markdown,
            "- readiness {}: `{}` - {}",
            crate::readiness::goal_label(verdict.goal),
            crate::readiness::status_label(verdict.status),
            verdict.summary
        );
    }
    if let Some(freshness) = handoff.freshness.as_ref() {
        let stale_count = freshness
            .changed_file_count
            .saturating_add(freshness.new_file_count)
            .saturating_add(freshness.removed_file_count);
        let _ = writeln!(
            markdown,
            "- freshness: `{:?}` stale_files={} checked={} indexed={}",
            freshness.status,
            stale_count,
            freshness.checked_file_count,
            freshness.indexed_file_count
        );
    } else {
        let _ = writeln!(markdown, "- freshness: not checked");
    }
    let _ = writeln!(
        markdown,
        "- sidecar: mode={} degraded_reason={}",
        handoff
            .sidecar_retrieval_mode
            .as_deref()
            .unwrap_or("unknown"),
        handoff.degraded_reason.as_deref().unwrap_or("none")
    );
    let _ = writeln!(markdown, "- trust_caveat: {}", handoff.trust_caveat);
    let _ = writeln!(
        markdown,
        "- top_entry_point: {}",
        handoff.top_entry_point.as_deref().unwrap_or("n/a")
    );
    let _ = writeln!(
        markdown,
        "- top_risk: {}",
        handoff.top_risk.as_deref().unwrap_or("n/a")
    );
    if let Some(command) = handoff.next_command.as_deref() {
        let _ = writeln!(markdown, "- next_command: `{}`", markdown_escape(command));
    }
    let _ = writeln!(markdown);
}

fn append_summary(markdown: &mut String, output: &codestory_runtime::graph_analysis::RepoReport) {
    let summary = &output.summary;
    let _ = writeln!(markdown, "## Repo Summary");
    let _ = writeln!(
        markdown,
        "- stats: nodes={} edges={} files={} errors={}",
        summary.node_count, summary.edge_count, summary.file_count, summary.error_count
    );
    let _ = writeln!(
        markdown,
        "- export: nodes={} edges={}",
        summary.exported_node_count, summary.exported_edge_count
    );
    append_count_map(markdown, "node_kinds", &summary.node_kinds);
    append_count_map(markdown, "edge_kinds", &summary.edge_kinds);
    let _ = writeln!(markdown);
}

fn append_count_map(
    markdown: &mut String,
    label: &str,
    counts: &std::collections::BTreeMap<String, usize>,
) {
    if counts.is_empty() {
        let _ = writeln!(markdown, "- {label}: none");
        return;
    }
    let rendered = counts
        .iter()
        .map(|(kind, count)| format!("{kind}={count}"))
        .collect::<Vec<_>>()
        .join(", ");
    let _ = writeln!(markdown, "- {label}: {rendered}");
}

fn append_node_section(
    markdown: &mut String,
    title: &str,
    nodes: &[codestory_runtime::graph_analysis::ReportNodeSummary],
) {
    let _ = writeln!(markdown, "## {title}");
    if nodes.is_empty() {
        let _ = writeln!(markdown, "No indexed graph nodes matched this section.");
        let _ = writeln!(markdown);
        return;
    }
    let _ = writeln!(markdown, "| Node | Kind | In | Out | Total | Source |");
    let _ = writeln!(markdown, "| --- | --- | ---: | ---: | ---: | --- |");
    for node in nodes {
        let _ = writeln!(
            markdown,
            "| `{}` | `{}` | {} | {} | {} | {} |",
            markdown_escape(&node.name),
            node.kind,
            node.incoming_edges,
            node.outgoing_edges,
            node.total_edges,
            render_source_location(node.source_location.as_ref())
        );
    }
    let _ = writeln!(markdown);
}

fn append_follow_ups(
    markdown: &mut String,
    output: &codestory_runtime::graph_analysis::RepoReport,
) {
    let _ = writeln!(markdown, "## Suggested Follow-up Queries");
    if output.follow_up_queries.is_empty() {
        let _ = writeln!(
            markdown,
            "No follow-up queries were generated because the current store has no visible graph relationships."
        );
        let _ = writeln!(markdown);
        return;
    }
    for query in &output.follow_up_queries {
        let _ = writeln!(
            markdown,
            "- `{}`: {}. Next: `{}`",
            markdown_escape(&query.query),
            query.reason,
            markdown_escape(&query.command)
        );
    }
    let _ = writeln!(markdown);
}

fn render_source_location(
    location: Option<&codestory_runtime::graph_analysis::SourceLocation>,
) -> String {
    let Some(location) = location else {
        return "n/a".to_string();
    };
    let Some(file) = location.file.as_deref() else {
        return "n/a".to_string();
    };
    let mut rendered = format!("`{}`", markdown_escape(&clean_path_string(file)));
    if let Some(line) = location.start_line {
        rendered.push(':');
        rendered.push_str(&line.to_string());
    }
    rendered
}

fn report_node_label(node: &ReportNodeSummary) -> String {
    let source = render_source_location(node.source_location.as_ref());
    format!(
        "`{}` ({}, edges={}) at {}",
        markdown_escape(&node.name),
        node.kind,
        node.total_edges,
        source
    )
}

fn markdown_escape(value: &str) -> String {
    value.replace('|', "\\|").replace('\n', " ")
}
