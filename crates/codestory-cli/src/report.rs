use anyhow::{Result, bail};
use std::fmt::Write as _;

use crate::args::{OutputFormat, ReportCommand};
use crate::display::clean_path_string;
use crate::output::{emit, validate_output_file_parent};
use crate::runtime::{RuntimeContext, ensure_index_ready};

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
    let output = codestory_runtime::graph_analysis::build_report_export(
        &runtime.project_root,
        &runtime.storage_path,
        cmd.limit,
    )?;
    let markdown = render_report_markdown(&output);
    emit(cmd.format, &output, markdown, cmd.output_file.as_deref())
}

fn render_report_markdown(output: &codestory_runtime::graph_analysis::RepoReportExport) -> String {
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
        "JSON graph export: rerun with `--format json` to get nodes, edges, confidence/certainty, source locations, and generation metadata."
    );
    markdown
}

fn append_summary(
    markdown: &mut String,
    output: &codestory_runtime::graph_analysis::RepoReportExport,
) {
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
    let _ = writeln!(
        markdown,
        "| Node | Kind | In | Out | Total | Source |"
    );
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
    output: &codestory_runtime::graph_analysis::RepoReportExport,
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

fn markdown_escape(value: &str) -> String {
    value.replace('|', "\\|").replace('\n', " ")
}
