use super::super::artifacts::ensure_dot_only_for_trail;
use super::super::drill_read_only_jobs;
use super::execution::{execute_drill, write_drill_outputs};
use super::reporting::{
    drill_artifact_extension, drill_suite_retrieval_blockers, drill_suite_text_key,
    render_drill_suite_markdown, validate_drill_output_dir, write_drill_suite_outputs,
};
use super::summary_decision::{
    dedupe_and_rank_drill_files, drill_summary_stats, ensure_trailing_newline, output_slug,
};
use super::summary_evidence::drill_summary;
use crate::args;
use crate::args::{
    DrillCommand, DrillRuntimeTimingsOutput, DrillSuiteCommand, DrillSuiteExpectationOutput,
    DrillSuiteOutput, DrillSuiteRepoOutput, DrillSummaryAnchorStatusOutput,
    DrillSummaryAnchorsOutput, DrillSummaryBridgesOutput, DrillSummaryMechanicalOutput,
    DrillSummaryOpenGapsOutput, DrillSummaryOutput, DrillSummarySourceTruthOutput,
    DrillSummaryVerdictOutput, ProjectArgs,
};
use crate::runtime::refresh_label;
use crate::{display, drill_targeting};
use anyhow::{Context, Result, bail};
use codestory_contracts::api::ClaimReadinessDto;
use serde::Deserialize;
use std::collections::HashSet;
use std::fs;

pub(in crate::app) fn run_drill_suite(cmd: DrillSuiteCommand) -> Result<()> {
    ensure_dot_only_for_trail(cmd.format, "drill-suite")?;
    validate_drill_output_dir(&cmd.output_dir)?;
    let suite_output = execute_codestory_real_repo_drill_suite(&cmd)?;
    emit_drill_suite_progress(format!(
        "writing suite reports output_dir={}",
        display::clean_path_string(&cmd.output_dir.to_string_lossy())
    ));
    write_drill_suite_outputs(cmd.format, &cmd.output_dir, &suite_output)?;
    emit_drill_suite_progress(format!(
        "done repos={} ready={} degraded={} blocked={} output_dir={}",
        suite_output.repo_count,
        suite_output.ready_count,
        suite_output.degraded_count,
        suite_output.blocked_count,
        suite_output.output_dir
    ));
    let markdown = render_drill_suite_markdown(&suite_output);
    let selected = match cmd.format {
        args::OutputFormat::Markdown => ensure_trailing_newline(markdown),
        args::OutputFormat::Json => ensure_trailing_newline(
            serde_json::to_string_pretty(&suite_output)
                .context("Failed to serialize drill suite JSON")?,
        ),
        args::OutputFormat::Dot => unreachable!("dot was rejected above"),
    };
    print!("{selected}");
    Ok(())
}

#[derive(Debug, Deserialize)]
pub(super) struct DrillSuiteCaseManifest {
    #[serde(default)]
    suite: Option<String>,
    cases: Vec<DrillSuiteCaseConfig>,
}

#[derive(Debug, Deserialize)]
pub(super) struct DrillSuiteCaseConfig {
    slug: String,
    project: std::path::PathBuf,
    question: String,
    anchors: Vec<String>,
    #[serde(default)]
    expect: DrillSuiteCaseExpectConfig,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub(super) struct DrillSuiteCaseExpectConfig {
    #[serde(default)]
    source_truth_files: Vec<String>,
    #[serde(default)]
    false_claims: Vec<String>,
    #[serde(default)]
    min_anchor_resolution: Option<usize>,
    #[serde(default)]
    allow_partial_bridges: Option<bool>,
}

#[derive(Debug)]
pub(super) struct DrillSuiteCase {
    slug: String,
    project_root: std::path::PathBuf,
    question: String,
    anchors: Vec<String>,
    expectations: DrillSuiteExpectationOutput,
}

pub(super) fn emit_drill_suite_progress(message: impl AsRef<str>) {
    eprintln!("[drill-suite] {}", message.as_ref());
}

pub(super) fn drill_suite_repo_progress_start_message(
    index: usize,
    total: usize,
    case: &DrillSuiteCase,
    repo_output_dir: &std::path::Path,
) -> String {
    format!(
        "[{index}/{total}] start {} project={} output_dir={}",
        case.slug,
        display::clean_path_string(&case.project_root.to_string_lossy()),
        display::clean_path_string(&repo_output_dir.to_string_lossy())
    )
}

pub(super) fn drill_suite_repo_progress_done_message(
    index: usize,
    total: usize,
    slug: &str,
    summary: &DrillSummaryOutput,
) -> String {
    format!(
        "[{index}/{total}] done {slug} verdict={} anchors={}/{} bridges=graph:{} partial:{} unresolved:{} output_dir={}",
        summary.verdict.status,
        summary.anchors.resolved,
        summary.anchors.requested,
        summary.bridges.graph_path,
        summary.bridges.partial,
        summary.bridges.unresolved_or_error,
        summary.output_dir
    )
}

pub(super) fn execute_codestory_real_repo_drill_suite(
    cmd: &DrillSuiteCommand,
) -> Result<DrillSuiteOutput> {
    let owner_root = cmd
        .project
        .project
        .canonicalize()
        .with_context(|| format!("Failed to resolve {}", cmd.project.project.display()))?;
    let (suite_name, cases) = drill_suite_cases_from_manifest(&cmd.case_file, &owner_root)?;
    let total_cases = cases.len();
    emit_drill_suite_progress(format!(
        "start cases={} refresh={} output_dir={}",
        total_cases,
        format!("{:?}", cmd.refresh).to_ascii_lowercase(),
        display::clean_path_string(&cmd.output_dir.to_string_lossy())
    ));
    let suite_jobs = drill_suite_case_jobs(cmd.jobs, cmd.refresh, total_cases);
    let drill_jobs = if suite_jobs > 1 {
        1
    } else {
        drill_read_only_jobs(cmd.jobs, cmd.refresh)
    };
    let repos = run_drill_suite_cases(cmd, cases, suite_jobs, drill_jobs);

    let degraded_count = drill_suite_verdict_count(&repos, "degraded");
    let blocked_count = drill_suite_verdict_count(&repos, "blocked");
    let ready_count = drill_suite_verdict_count(&repos, "ready");
    let next_actions = repos
        .iter()
        .map(|repo| format!("{}: {}", repo.slug, repo.summary.verdict.next_action))
        .collect::<Vec<_>>();
    let retrieval_blockers = drill_suite_retrieval_blockers(&repos);

    Ok(DrillSuiteOutput {
        suite: suite_name,
        project: display::clean_path_string(&owner_root.to_string_lossy()),
        case_file: display::clean_path_string(&cmd.case_file.to_string_lossy()),
        output_dir: display::clean_path_string(&cmd.output_dir.to_string_lossy()),
        repo_count: repos.len(),
        degraded_count,
        blocked_count,
        ready_count,
        repos,
        retrieval_blockers,
        next_actions,
    })
}

pub(super) fn drill_suite_case_jobs(
    requested: usize,
    refresh: args::RefreshMode,
    total_cases: usize,
) -> usize {
    if total_cases <= 1 {
        1
    } else {
        drill_read_only_jobs(requested, refresh).min(total_cases)
    }
}

pub(super) fn run_drill_suite_cases(
    cmd: &DrillSuiteCommand,
    cases: Vec<DrillSuiteCase>,
    jobs: usize,
    drill_jobs: usize,
) -> Vec<DrillSuiteRepoOutput> {
    let total_cases = cases.len();
    if jobs <= 1 || total_cases <= 1 {
        return cases
            .iter()
            .enumerate()
            .map(|(case_index, case)| {
                run_drill_suite_case(cmd, case_index, total_cases, case, drill_jobs)
            })
            .collect();
    }

    let indexed_cases = cases.into_iter().enumerate().collect::<Vec<_>>();
    let chunk_size = indexed_cases.len().div_ceil(jobs);
    let mut repos_by_case = vec![None; total_cases];
    std::thread::scope(|scope| {
        let mut handles = Vec::new();
        for chunk in indexed_cases.chunks(chunk_size) {
            handles.push(scope.spawn(move || {
                chunk
                    .iter()
                    .map(|(case_index, case)| {
                        let repo = run_drill_suite_case(cmd, *case_index, total_cases, case, 1);
                        (*case_index, repo)
                    })
                    .collect::<Vec<_>>()
            }));
        }

        for handle in handles {
            for (case_index, repo) in handle.join().expect("drill-suite worker panicked") {
                repos_by_case[case_index] = Some(repo);
            }
        }
    });

    repos_by_case
        .into_iter()
        .map(|repo| repo.expect("drill-suite worker should fill every case"))
        .collect()
}

pub(super) fn run_drill_suite_case(
    cmd: &DrillSuiteCommand,
    case_index: usize,
    total_cases: usize,
    case: &DrillSuiteCase,
    drill_jobs: usize,
) -> DrillSuiteRepoOutput {
    let progress_index = case_index + 1;
    let repo_output_dir = cmd.output_dir.join(format!("{}-drill", case.slug));
    emit_drill_suite_progress(drill_suite_repo_progress_start_message(
        progress_index,
        total_cases,
        case,
        &repo_output_dir,
    ));
    let drill_cmd = DrillCommand {
        project: ProjectArgs {
            project: case.project_root.clone(),
            cache_dir: drill_suite_case_cache_dir(cmd.project.cache_dir.as_deref(), &case.slug),
        },
        anchors: case
            .anchors
            .iter()
            .map(|anchor| anchor.to_string())
            .collect(),
        label: Some(case.slug.clone()),
        question: Some(case.question.clone()),
        output_dir: repo_output_dir.clone(),
        refresh: cmd.refresh,
        profile: None,
        run_id: None,
        format: cmd.format,
        jobs: drill_jobs,
    };
    match execute_drill(&drill_cmd).and_then(|operation| {
        write_drill_outputs(cmd.format, &repo_output_dir, &operation)?;
        Ok(drill_summary(&operation.value))
    }) {
        Ok(summary) => {
            emit_drill_suite_progress(drill_suite_repo_progress_done_message(
                progress_index,
                total_cases,
                &case.slug,
                &summary,
            ));
            DrillSuiteRepoOutput {
                slug: case.slug.clone(),
                project: display::clean_path_string(&case.project_root.to_string_lossy()),
                question: case.question.clone(),
                anchors: case.anchors.clone(),
                output_dir: display::clean_path_string(&repo_output_dir.to_string_lossy()),
                artifact_extension: drill_artifact_extension(cmd.format).to_string(),
                summary,
                expectations: case.expectations.clone(),
            }
        }
        Err(error) => {
            emit_drill_suite_progress(format!(
                "[{progress_index}/{total_cases}] blocked {} error={}",
                case.slug, error
            ));
            blocked_drill_suite_repo_output(
                case,
                &repo_output_dir,
                cmd.refresh,
                cmd.format,
                &error.to_string(),
            )
        }
    }
}

pub(super) fn drill_suite_verdict_count(repos: &[DrillSuiteRepoOutput], status: &str) -> usize {
    repos
        .iter()
        .filter(|repo| repo.summary.verdict.status == status)
        .count()
}

pub(super) fn drill_suite_case_cache_dir(
    suite_cache_dir: Option<&std::path::Path>,
    slug: &str,
) -> Option<std::path::PathBuf> {
    suite_cache_dir.map(|cache_dir| cache_dir.join(output_slug(slug)))
}

pub(super) fn drill_suite_cases_from_manifest(
    case_file: &std::path::Path,
    owner_root: &std::path::Path,
) -> Result<(String, Vec<DrillSuiteCase>)> {
    let case_file = absolute_existing_path(case_file).with_context(|| {
        format!(
            "Failed to resolve drill-suite case file {}",
            display::clean_path_string(&case_file.to_string_lossy())
        )
    })?;
    let manifest_text = fs::read_to_string(&case_file).with_context(|| {
        format!(
            "Failed to read drill-suite case file {}",
            display::clean_path_string(&case_file.to_string_lossy())
        )
    })?;
    let manifest: DrillSuiteCaseManifest =
        serde_json::from_str(&manifest_text).with_context(|| {
            format!(
                "Failed to parse drill-suite case file {} as JSON",
                display::clean_path_string(&case_file.to_string_lossy())
            )
        })?;
    if manifest.cases.is_empty() {
        bail!("drill-suite case file must contain at least one case");
    }
    let manifest_dir = case_file
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(owner_root);
    let mut cases = Vec::with_capacity(manifest.cases.len());
    let mut seen_slugs = HashSet::new();
    for case in manifest.cases {
        let slug = output_slug(&case.slug);
        if slug.is_empty() {
            bail!("drill-suite case slug cannot be empty");
        }
        if !seen_slugs.insert(slug.clone()) {
            bail!("drill-suite case slug `{slug}` is duplicated");
        }
        if case.question.trim().is_empty() {
            bail!("drill-suite case `{slug}` question cannot be empty");
        }
        let anchors = drill_targeting::validated_drill_anchors(
            &case.anchors,
            &format!("drill-suite case `{slug}`"),
        )?;
        let project_root = if case.project.is_absolute() {
            case.project
        } else {
            manifest_dir.join(case.project)
        };
        cases.push(DrillSuiteCase {
            slug,
            project_root,
            question: case.question,
            anchors,
            expectations: drill_suite_expectations_from_config(case.expect),
        });
    }
    Ok((
        manifest
            .suite
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| "codestory-agent-drill-suite".to_string()),
        cases,
    ))
}

pub(super) fn drill_suite_expectations_from_config(
    config: DrillSuiteCaseExpectConfig,
) -> DrillSuiteExpectationOutput {
    let mut source_truth_files = config
        .source_truth_files
        .into_iter()
        .map(|path| path.trim().to_string())
        .filter(|path| !path.is_empty())
        .collect::<Vec<_>>();
    dedupe_and_rank_drill_files(&mut source_truth_files);
    let mut false_claims = config
        .false_claims
        .into_iter()
        .map(|claim| claim.trim().to_string())
        .filter(|claim| !claim.is_empty())
        .collect::<Vec<_>>();
    false_claims.sort_by_key(|claim| drill_suite_text_key(claim));
    false_claims.dedup_by(|left, right| drill_suite_text_key(left) == drill_suite_text_key(right));
    DrillSuiteExpectationOutput {
        source_truth_files,
        false_claims,
        min_anchor_resolution: config.min_anchor_resolution,
        allow_partial_bridges: config.allow_partial_bridges,
    }
}

pub(super) fn absolute_existing_path(path: &std::path::Path) -> Result<std::path::PathBuf> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .context("Failed to resolve current working directory")?
            .join(path)
    };
    fs::metadata(&path).with_context(|| {
        format!(
            "Failed to access path {}",
            display::clean_path_string(&path.to_string_lossy())
        )
    })?;
    Ok(path)
}

pub(super) fn blocked_drill_suite_repo_output(
    case: &DrillSuiteCase,
    repo_output_dir: &std::path::Path,
    refresh: args::RefreshMode,
    format: args::OutputFormat,
    error: &str,
) -> DrillSuiteRepoOutput {
    let project = display::clean_path_string(&case.project_root.to_string_lossy());
    let output_dir = display::clean_path_string(&repo_output_dir.to_string_lossy());
    let next_action = format!(
        "Fix or skip this case, then rerun `drill-suite`; blocked before evidence artifacts were written: {}",
        error.replace('|', "\\|")
    );

    DrillSuiteRepoOutput {
        slug: case.slug.clone(),
        project: project.clone(),
        question: case.question.clone(),
        anchors: case.anchors.clone(),
        output_dir: output_dir.clone(),
        artifact_extension: drill_artifact_extension(format).to_string(),
        summary: blocked_drill_summary(case, project, output_dir, refresh, error, next_action),
        expectations: case.expectations.clone(),
    }
}

fn blocked_drill_anchor_statuses(case: &DrillSuiteCase) -> Vec<DrillSummaryAnchorStatusOutput> {
    case.anchors
        .iter()
        .map(|anchor| DrillSummaryAnchorStatusOutput {
            anchor: anchor.clone(),
            status: "not_run".to_string(),
            typed_hit_count: 0,
            selected: None,
            selected_node_id: None,
            selected_node_ref: None,
            selected_kind: None,
            selected_file_path: None,
            selected_line: None,
            caller_count: 0,
            consumer_count: 0,
            text_hint_count: 0,
            command_count: 0,
            failed_command_count: 0,
            command_duration_ms: 0,
            total_duration_ms: 0,
            resolution_duration_ms: 0,
            consumer_summary_duration_ms: 0,
            slowest_command: None,
            slowest_command_ms: 0,
            source_truth_target_count: 0,
        })
        .collect()
}

fn blocked_drill_summary(
    case: &DrillSuiteCase,
    project: String,
    output_dir: String,
    refresh: args::RefreshMode,
    error: &str,
    next_action: String,
) -> DrillSummaryOutput {
    DrillSummaryOutput {
        summary_version: 1,
        project,
        label: Some(case.slug.clone()),
        question: Some(case.question.clone()),
        output_dir: output_dir.clone(),
        full_report_json: String::new(),
        full_report_markdown: String::new(),
        mechanical: DrillSummaryMechanicalOutput {
            refresh: refresh_label(refresh, None),
            before: Some(drill_summary_stats(0, 0, 0, 0)),
            before_unavailable_reason: None,
            after: drill_summary_stats(0, 0, 0, 1),
            index_ready: false,
            error_delta: Some(1),
            retrieval_status: None,
            freshness_status: Some("unknown".to_string()),
            stale_file_count: 0,
            freshness_samples: Vec::new(),
            phase_timing_available: false,
            drill_timings: DrillRuntimeTimingsOutput::default(),
        },
        anchors: DrillSummaryAnchorsOutput {
            requested: case.anchors.len(),
            resolved: 0,
            unresolved: case.anchors.len(),
            failed_command_count: 1,
            statuses: blocked_drill_anchor_statuses(case),
        },
        bridges: DrillSummaryBridgesOutput {
            total: 0,
            graph_path: 0,
            partial: 0,
            unresolved_or_error: 0,
            statuses: Vec::new(),
        },
        source_truth: DrillSummarySourceTruthOutput {
            required: false,
            check_count: 0,
            pending_check_count: 0,
            verified_check_count: 0,
            target_file_count: 0,
            target_files: Vec::new(),
            target_file_details: Vec::new(),
            checklist_item_count: 0,
            claim_count: 0,
            pending_claim_count: 0,
            verified_claim_count: 0,
        },
        open_gaps: DrillSummaryOpenGapsOutput {
            overall_status: ClaimReadinessDto::NeedsSourceRead,
            answer_quality_status: "blocked_before_evidence".to_string(),
            safe_to_say_count: 0,
            inferred_claim_count: 0,
            needs_verification_count: 1,
            needs_verification_claim_count: 0,
            pending_claim_count: 0,
            pending_source_truth_check_count: 0,
            next_command_count: 1,
            open_gap_friendly: true,
            status: "blocked".to_string(),
        },
        verdict: DrillSummaryVerdictOutput {
            status: "blocked".to_string(),
            reason: format!("drill failed before evidence collection: {error}"),
            next_action,
        },
    }
}
