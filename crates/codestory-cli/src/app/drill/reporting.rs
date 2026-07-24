use super::summary_decision::{drill_suite_retrieval_label, ensure_trailing_newline};
use crate::args;
use crate::args::{
    DrillOutput, DrillSuiteOutput, DrillSuiteRepoOutput, DrillSuiteRetrievalBlockerOutput,
    DrillSummaryBridgesOutput, DrillSummarySourceTruthOutput,
};
use crate::display;
use crate::runtime;
use anyhow::{Context, Result, bail};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;

pub(super) fn write_drill_suite_outputs(
    format: args::OutputFormat,
    output_dir: &std::path::Path,
    output: &DrillSuiteOutput,
) -> Result<()> {
    let markdown = render_drill_suite_markdown(output);
    let json = ensure_trailing_newline(
        serde_json::to_string_pretty(output).context("Failed to serialize drill suite JSON")?,
    );
    write_drill_report_file(&output_dir.join("suite-report.md"), &markdown)?;
    write_drill_report_file(&output_dir.join("suite-report.json"), &json)?;
    let selected = match format {
        args::OutputFormat::Markdown => ensure_trailing_newline(markdown),
        args::OutputFormat::Json => json,
        args::OutputFormat::Dot => unreachable!("dot was rejected above"),
    };
    let report_ext = drill_artifact_extension(format);
    write_drill_report_file(
        &output_dir.join(format!("drill-suite-report.{report_ext}")),
        &selected,
    )
}

pub(super) fn drill_artifact_extension(format: args::OutputFormat) -> &'static str {
    match format {
        args::OutputFormat::Markdown => "md",
        args::OutputFormat::Json => "json",
        args::OutputFormat::Dot => unreachable!("dot was rejected above"),
    }
}

pub(super) fn render_drill_suite_markdown(output: &DrillSuiteOutput) -> String {
    let mut markdown = String::new();
    render_drill_suite_header(&mut markdown, output);
    render_drill_suite_retrieval_blockers(&mut markdown, &output.retrieval_blockers);
    render_drill_suite_repo_table(&mut markdown, &output.repos);
    render_drill_suite_repo_artifacts(&mut markdown, &output.repos);
    render_drill_suite_next_actions(&mut markdown, &output.next_actions);
    ensure_trailing_newline(markdown)
}

pub(super) fn render_drill_suite_header(markdown: &mut String, output: &DrillSuiteOutput) {
    let _ = writeln!(markdown, "# CodeStory Real-Repo Agent Drill Suite");
    let _ = writeln!(markdown);
    let _ = writeln!(markdown, "- suite: `{}`", output.suite);
    let _ = writeln!(markdown, "- project: `{}`", output.project);
    let _ = writeln!(markdown, "- case_file: `{}`", output.case_file);
    let _ = writeln!(markdown, "- output_dir: `{}`", output.output_dir);
    let _ = writeln!(
        markdown,
        "- repos: {} total, {} ready, {} degraded, {} blocked",
        output.repo_count, output.ready_count, output.degraded_count, output.blocked_count
    );
}

pub(super) fn render_drill_suite_retrieval_blockers(
    markdown: &mut String,
    blockers: &[DrillSuiteRetrievalBlockerOutput],
) {
    if blockers.is_empty() {
        return;
    }

    let _ = writeln!(markdown);
    let _ = writeln!(markdown, "## Retrieval Blockers");
    for blocker in blockers {
        let _ = writeln!(
            markdown,
            "- `{}` repos={} [{}]: {}",
            blocker.status,
            blocker.repo_count,
            blocker.repos.join(", "),
            blocker.next_action
        );
    }
}

pub(super) fn render_drill_suite_repo_table(markdown: &mut String, repos: &[DrillSuiteRepoOutput]) {
    let _ = writeln!(markdown);
    let _ = writeln!(
        markdown,
        "| repo | verdict | freshness | retrieval | anchors | bridges | source truth | reports | next action |"
    );
    let _ = writeln!(markdown, "|---|---|---|---|---:|---:|---|---|---|");
    for repo in repos {
        let reports = drill_suite_repo_report_label(repo);
        let _ = writeln!(
            markdown,
            "| `{}` | {} | {} | {} | {}/{} | {} | {} | {} | {} |",
            repo.slug,
            repo.summary.verdict.status,
            repo.summary
                .mechanical
                .freshness_status
                .as_deref()
                .unwrap_or("unknown"),
            drill_suite_retrieval_label(repo.summary.mechanical.retrieval_status.as_deref()),
            repo.summary.anchors.resolved,
            repo.summary.anchors.requested,
            drill_suite_bridge_label(&repo.summary.bridges),
            drill_suite_source_truth_label(&repo.summary.source_truth),
            reports,
            repo.summary.verdict.next_action.replace('|', "\\|")
        );
    }
}

pub(super) fn render_drill_suite_repo_artifacts(
    markdown: &mut String,
    repos: &[DrillSuiteRepoOutput],
) {
    if repos.is_empty() {
        return;
    }

    let _ = writeln!(markdown);
    let _ = writeln!(markdown, "## Repo Artifacts");
    for repo in repos {
        if repo.summary.full_report_markdown.is_empty() && repo.summary.full_report_json.is_empty()
        {
            let _ = writeln!(
                markdown,
                "- `{}`: no per-repo artifacts were written because the case blocked before evidence collection",
                repo.slug
            );
            continue;
        }
        let markdown_report =
            drill_suite_join_artifact_path(&repo.output_dir, &repo.summary.full_report_markdown);
        let json_report =
            drill_suite_join_artifact_path(&repo.output_dir, &repo.summary.full_report_json);
        let bridge_artifacts = drill_suite_join_artifact_path(
            &repo.output_dir,
            &format!("*-bridge.{}", repo.artifact_extension),
        );
        let _ = writeln!(
            markdown,
            "- `{}`: report `{}`; json `{}`; bridge artifacts `{}`",
            repo.slug, markdown_report, json_report, bridge_artifacts
        );
    }
}

pub(super) fn render_drill_suite_next_actions(markdown: &mut String, next_actions: &[String]) {
    if next_actions.is_empty() {
        return;
    }

    let _ = writeln!(markdown);
    let _ = writeln!(markdown, "## Next Actions");
    for action in next_actions {
        let _ = writeln!(markdown, "- {action}");
    }
}

pub(super) fn drill_suite_repo_report_label(repo: &DrillSuiteRepoOutput) -> String {
    if repo.summary.full_report_markdown.is_empty() && repo.summary.full_report_json.is_empty() {
        return "not written (blocked before evidence)".to_string();
    }
    let markdown_report =
        drill_suite_join_artifact_path(&repo.output_dir, &repo.summary.full_report_markdown);
    let json_report =
        drill_suite_join_artifact_path(&repo.output_dir, &repo.summary.full_report_json);
    format!("`{markdown_report}` / `{json_report}`").replace('|', "\\|")
}

pub(super) fn drill_suite_join_artifact_path(output_dir: &str, artifact: &str) -> String {
    if artifact.contains(':')
        || artifact.starts_with('/')
        || artifact.starts_with('\\')
        || artifact.contains('/')
        || artifact.contains('\\')
    {
        return artifact.to_string();
    }
    format!(
        "{}/{}",
        output_dir.trim_end_matches(['/', '\\']),
        artifact.trim_start_matches(['/', '\\'])
    )
}

pub(super) fn drill_suite_bridge_label(bridges: &DrillSummaryBridgesOutput) -> String {
    format!(
        "{} graph / {} partial / {} unresolved-error",
        bridges.graph_path, bridges.partial, bridges.unresolved_or_error
    )
}

pub(super) fn drill_suite_source_truth_label(
    source_truth: &DrillSummarySourceTruthOutput,
) -> String {
    if source_truth.required
        || source_truth.pending_check_count > 0
        || source_truth.verified_check_count > 0
    {
        return format!(
            "{} targets / {} verified / {} pending",
            source_truth.target_file_count,
            source_truth.verified_check_count,
            source_truth.pending_check_count
        );
    }
    format!(
        "{} targets / {} checks",
        source_truth.target_file_count, source_truth.check_count
    )
}

pub(super) fn drill_suite_retrieval_blockers(
    repos: &[DrillSuiteRepoOutput],
) -> Vec<DrillSuiteRetrievalBlockerOutput> {
    let mut grouped = BTreeMap::<String, Vec<String>>::new();
    for repo in repos {
        let Some(status) = repo.summary.mechanical.retrieval_status.as_ref() else {
            continue;
        };
        if drill_suite_retrieval_label(Some(status)) == "full" {
            continue;
        }
        grouped
            .entry(status.clone())
            .or_default()
            .push(repo.slug.clone());
    }
    grouped
        .into_iter()
        .map(|(status, repos)| {
            let next_action = if status.contains("MissingEmbeddingRuntime") {
                "rebuild with `codestory-cli retrieval index --project <repo> --refresh full`; the embedded engine initializes automatically".to_string()
            } else if status.contains("MissingSemanticDocs") {
                "rerun `codestory-cli retrieval index --project <repo> --refresh full` before trusting packet/search evidence".to_string()
            } else {
                "inspect doctor/retrieval status and repair to retrieval_mode=full before treating broad search quality as repo-specific".to_string()
            };
            DrillSuiteRetrievalBlockerOutput {
                status,
                repo_count: repos.len(),
                repos,
                next_action,
            }
        })
        .collect()
}

pub(super) fn drill_suite_text_key(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

pub(super) fn validate_drill_output_dir(output_dir: &std::path::Path) -> Result<()> {
    fs::create_dir_all(output_dir).with_context(|| {
        format!(
            "Failed to create drill output directory {}",
            display::clean_path_string(&output_dir.to_string_lossy())
        )
    })
}

pub(in crate::app) struct DrillReportContents {
    pub(super) selected: String,
    pub(super) markdown: String,
    pub(super) json: String,
}

pub(super) fn render_drill_contents(
    format: args::OutputFormat,
    operation: &codestory_runtime::PublicOperation<DrillOutput>,
    markdown: &str,
) -> Result<DrillReportContents> {
    let markdown = ensure_trailing_newline(markdown.to_string());
    let output = runtime::public_operation_json_value(operation, &operation.value)?;
    let json = ensure_trailing_newline(
        serde_json::to_string_pretty(&output).context("Failed to serialize drill JSON")?,
    );
    let selected = match format {
        args::OutputFormat::Markdown => markdown.clone(),
        args::OutputFormat::Json => json.clone(),
        args::OutputFormat::Dot => bail!("--format dot is only supported by `trail`"),
    };
    Ok(DrillReportContents {
        selected,
        markdown,
        json,
    })
}

pub(super) fn write_drill_report_file(path: &std::path::Path, content: &str) -> Result<()> {
    fs::write(path, content).with_context(|| {
        format!(
            "Failed to write drill report {}",
            display::clean_path_string(&path.to_string_lossy())
        )
    })
}
