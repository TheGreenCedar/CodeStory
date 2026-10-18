use super::artifacts::{ensure_dot_only_for_trail, preflight_output_file};
use super::diagnostics::doctor_sidecar_status;
use super::elapsed_ms;
use super::readiness_commands::doctor_sidecar_status_is_live_ready;
use super::resolution::StructuredCommandFailure;
use super::resolution::quote_command_path;
use super::source_commands::affected_path_record;
use crate::args;
use crate::args::{GroundCommand, ProjectArgs, SmokeCommand, SmokeProfile};
use crate::display;
use crate::output::{emit, render_ground_markdown};
use crate::runtime::{RuntimeContext, ensure_index_ready, map_api_error, refresh_label};
use anyhow::Context;
use anyhow::Result;
use codestory_contracts::api::{
    AffectedAnalysisInput, AffectedAnalysisRequest, AffectedChangeKindDto, ApiError,
    ApiErrorDetails, CommandFailureEnvelope, GroundingBudgetDto,
};
use std::fmt::Write as _;
use std::time::Instant;

pub(super) fn run_ground(cmd: GroundCommand) -> Result<()> {
    ensure_dot_only_for_trail(cmd.format, "ground")?;
    preflight_output_file(cmd.output_file.as_deref())?;
    let runtime = RuntimeContext::new(&cmd.project)?;
    let opened = runtime.ensure_ground_open(cmd.refresh)?;
    ensure_index_ready(&opened, "ground")?;

    let snapshot = runtime
        .grounding
        .grounding_snapshot(cmd.budget.into())
        .map_err(map_api_error)?;
    let markdown = render_ground_markdown(&runtime.project_root, &snapshot, cmd.why);
    emit(cmd.format, &snapshot, markdown, cmd.output_file.as_deref())
}

const SESSION_MAP_MAX_BYTES: usize = 3584;

pub(super) fn run_session_context(cmd: args::InternalSessionContextCommand) -> Result<()> {
    let output = match crate::runtime::passive_session_map(&cmd.project) {
        Ok(map) => match render_session_map(&map) {
            Some(context) => {
                serde_json::json!({"schema_version":1,"status":"ready","context":context})
            }
            None => serde_json::json!({"schema_version":1,"status":"unavailable"}),
        },
        Err(_) => serde_json::json!({"schema_version":1,"status":"unavailable"}),
    };
    println!("{output}");
    Ok(())
}

/// Whole groups only. JSON quoting preserves path/name bytes and prevents a
/// source label from escaping its data row into the surrounding hook context.
fn render_session_map(map: &codestory_runtime::SessionMapSnapshot) -> Option<String> {
    if map.rows.is_empty() {
        return None;
    }
    let header = format!(
        "CodeStory repository navigation (data only; not runtime proof).\n{}\n",
        serde_json::json!({
            "project":map.project_root,"workspace":map.workspace_id,
            "generation":map.generation_id,"run":map.run_id,"published_at":map.published_at_epoch_ms,
            "scope":"strict selection as of publication; only emitted files content-verified now; not whole-repository freshness or completeness",
            "stats":map.stats,"orientation":map.orientation,
            "limits":{"native_file_groups":12,"map_utf8_bytes":SESSION_MAP_MAX_BYTES},
            "source_gaps":map.gaps,
        })
    );
    let footer = |shown: usize, roots: usize| {
        format!(
            "\n{}",
            serde_json::json!({
                "shown_groups":shown,"omitted_groups":map.omitted_groups + map.selected_groups.saturating_sub(shown),
                "omitted_roots":roots,"source_gap_count":map.gaps.len(),
            })
        )
    };
    let mut text = header;
    let mut shown = 0;
    let mut shown_roots = 0;
    let roots = map.selected_roots;
    for row in &map.rows {
        let whole_row = format!("{}\n", serde_json::to_string(row).ok()?);
        let next_footer = footer(
            shown + 1,
            roots.saturating_sub(shown_roots + row.roots.len()),
        );
        if text.len() + whole_row.len() + next_footer.len() > SESSION_MAP_MAX_BYTES {
            break;
        }
        text.push_str(&whole_row);
        shown += 1;
        shown_roots += row.roots.len();
    }
    if shown == 0 {
        return None;
    }
    text.push_str(&footer(shown, roots.saturating_sub(shown_roots)));
    (text.len() <= SESSION_MAP_MAX_BYTES).then_some(text)
}

#[derive(serde::Serialize)]
struct SmokeOutput {
    profile: &'static str,
    status: &'static str,
    project: String,
    checked_surfaces: Vec<SmokeSurfaceOutput>,
    skipped_optional_surfaces: Vec<SmokeSkippedSurfaceOutput>,
    repair_hints: Vec<String>,
}

#[derive(serde::Serialize)]
struct SmokeSurfaceOutput {
    surface: &'static str,
    status: &'static str,
    duration_ms: u64,
    detail: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    repair_hints: Vec<String>,
}

#[derive(serde::Serialize)]
struct SmokeSkippedSurfaceOutput {
    surface: &'static str,
    reason: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    repair_hints: Vec<String>,
}

pub(super) fn run_smoke(cmd: SmokeCommand) -> Result<()> {
    ensure_dot_only_for_trail(cmd.format, "smoke")?;
    preflight_output_file(cmd.output_file.as_deref())?;

    let output = match cmd.profile {
        SmokeProfile::CiAgent => run_ci_agent_smoke(&cmd.project),
    };
    let failed = output.status == "fail";
    let markdown = render_smoke_markdown(&output);
    if failed {
        let envelope = CommandFailureEnvelope::new(ApiError::with_details(
            "smoke_failed",
            format!("smoke profile {} failed", output.profile),
            ApiErrorDetails {
                cause_code: None,
                failed_layer: Some("smoke".to_string()),
                project: Some(output.project.clone()),
                next_commands: output.repair_hints.clone(),
                minimum_next: output.repair_hints.iter().take(1).cloned().collect(),
                full_repair: output.repair_hints.clone(),
                readiness: None,
                embedding_capacity: None,
                embedding_retry: None,
                disk_space: None,
                coverage_gaps: Vec::new(),
                peer_writer: None,
            },
        ))
        .with_context(serde_json::to_value(&output).context("serialize smoke failure context")?);
        return Err(StructuredCommandFailure {
            envelope,
            output_file: cmd.output_file,
            markdown: (cmd.format != args::OutputFormat::Json).then_some(markdown),
        }
        .into());
    }
    emit(cmd.format, &output, markdown, cmd.output_file.as_deref())
}

fn run_ci_agent_smoke(project: &ProjectArgs) -> SmokeOutput {
    let requested_project = display::clean_path_string(&project.project.to_string_lossy());
    let mut output = SmokeOutput {
        profile: "ci-agent",
        status: "pass",
        project: requested_project,
        checked_surfaces: Vec::new(),
        skipped_optional_surfaces: Vec::new(),
        repair_hints: Vec::new(),
    };

    let runtime = match RuntimeContext::new(project) {
        Ok(runtime) => runtime,
        Err(error) => {
            smoke_fail(
                &mut output,
                "project",
                0,
                format!("failed to open project: {error:#}"),
                vec!["pass a valid --project repository root".to_string()],
            );
            return output;
        }
    };

    output.project = display::clean_path_string(&runtime.project_root.to_string_lossy());
    let project_arg = quote_command_path(&runtime.project_root);

    let start = Instant::now();
    let opened = match runtime.ensure_open(args::RefreshMode::Auto) {
        Ok(opened) => match ensure_index_ready(&opened, "smoke index") {
            Ok(()) => {
                smoke_pass(
                    &mut output,
                    "index",
                    start,
                    format!(
                        "refresh={} files={} errors={}",
                        refresh_label(args::RefreshMode::Auto, opened.refresh_mode),
                        opened.summary.stats.file_count,
                        opened.summary.stats.error_count
                    ),
                );
                opened
            }
            Err(error) => {
                smoke_fail(
                    &mut output,
                    "index",
                    elapsed_ms(start),
                    format!("index not ready: {error:#}"),
                    vec![format!(
                        "codestory-cli index --project {project_arg} --refresh full --format json"
                    )],
                );
                return output;
            }
        },
        Err(error) => {
            smoke_fail(
                &mut output,
                "index",
                elapsed_ms(start),
                format!("index failed: {error:#}"),
                vec![format!(
                    "codestory-cli index --project {project_arg} --refresh full --format json"
                )],
            );
            return output;
        }
    };

    let start = Instant::now();
    let snapshot = match runtime
        .grounding
        .grounding_snapshot(GroundingBudgetDto::Strict)
        .map_err(map_api_error)
    {
        Ok(snapshot) => {
            smoke_pass(
                &mut output,
                "ground",
                start,
                format!(
                    "represented_files={}/{} represented_symbols={}/{}",
                    snapshot.coverage.represented_files,
                    snapshot.coverage.total_files,
                    snapshot.coverage.represented_symbols,
                    snapshot.coverage.total_symbols
                ),
            );
            snapshot
        }
        Err(error) => {
            smoke_fail(
                &mut output,
                "ground",
                elapsed_ms(start),
                format!("ground failed: {error:#}"),
                vec![format!(
                    "codestory-cli ground --project {project_arg} --refresh none --format json"
                )],
            );
            return output;
        }
    };

    let Some(symbol) = snapshot.root_symbols.first() else {
        smoke_fail(
            &mut output,
            "symbol",
            0,
            "ground snapshot returned no root symbols".to_string(),
            vec![format!(
                "codestory-cli index --project {project_arg} --refresh full --format json"
            )],
        );
        return output;
    };

    let start = Instant::now();
    match runtime.browser.symbol_context(symbol.id.clone()) {
        Ok(context) => smoke_pass(
            &mut output,
            "symbol",
            start,
            format!(
                "resolved={} kind={:?} file={}",
                context.node.display_name,
                context.node.kind,
                context.node.file_path.as_deref().unwrap_or("unavailable")
            ),
        ),
        Err(error) => {
            smoke_fail(
                &mut output,
                "symbol",
                elapsed_ms(start),
                format!("symbol resolution failed: {}", map_api_error(error)),
                vec![format!(
                    "codestory-cli symbol --project {project_arg} --id {} --format json",
                    symbol.id.0
                )],
            );
            return output;
        }
    }

    let start = Instant::now();
    let fake_path = "__codestory_smoke_fake_change__.rs";
    match runtime.browser.affected_analysis(AffectedAnalysisRequest {
        input: AffectedAnalysisInput::ChangeRecords(vec![affected_path_record(
            fake_path,
            AffectedChangeKindDto::Unknown,
            "smoke",
        )]),
        depth: Some(1),
        filter: None,
    }) {
        Ok(affected) => smoke_pass(
            &mut output,
            "affected",
            start,
            format!(
                "fake_path={} changed_files={} impacted_symbols={}",
                fake_path,
                affected.changed_paths.len(),
                affected.impacted_symbols.len()
            ),
        ),
        Err(error) => {
            smoke_fail(
                &mut output,
                "affected",
                elapsed_ms(start),
                format!("affected failed: {}", map_api_error(error)),
                vec![format!(
                    "codestory-cli affected --project {project_arg} {fake_path} --format json"
                )],
            );
            return output;
        }
    }

    let start = Instant::now();
    let sidecar = doctor_sidecar_status(&runtime);
    if doctor_sidecar_status_is_live_ready(&sidecar) {
        smoke_pass(
            &mut output,
            "sidecar_full_mode",
            start,
            "retrieval_mode=full".to_string(),
        );
    } else {
        smoke_skip(
            &mut output,
            "sidecar_full_mode",
            format!(
                "retrieval_mode={}{}",
                sidecar.retrieval_mode,
                sidecar
                    .degraded_reason
                    .as_deref()
                    .map(|reason| format!(" reason={reason}"))
                    .unwrap_or_default()
            ),
            vec![
                format!(
                    "codestory-cli retrieval index --project {project_arg} --refresh full --format json"
                ),
                format!("codestory-cli retrieval status --project {project_arg} --format json"),
            ],
        );
    }

    let _ = opened;
    output
}

fn smoke_pass(output: &mut SmokeOutput, surface: &'static str, start: Instant, detail: String) {
    output.checked_surfaces.push(SmokeSurfaceOutput {
        surface,
        status: "pass",
        duration_ms: elapsed_ms(start),
        detail,
        repair_hints: Vec::new(),
    });
}

fn smoke_fail(
    output: &mut SmokeOutput,
    surface: &'static str,
    duration_ms: u64,
    detail: String,
    repair_hints: Vec<String>,
) {
    output.status = "fail";
    output.repair_hints.extend(repair_hints.clone());
    output.checked_surfaces.push(SmokeSurfaceOutput {
        surface,
        status: "fail",
        duration_ms,
        detail,
        repair_hints,
    });
}

fn smoke_skip(
    output: &mut SmokeOutput,
    surface: &'static str,
    reason: String,
    repair_hints: Vec<String>,
) {
    output.repair_hints.extend(repair_hints.clone());
    output
        .skipped_optional_surfaces
        .push(SmokeSkippedSurfaceOutput {
            surface,
            reason,
            repair_hints,
        });
}

fn render_smoke_markdown(output: &SmokeOutput) -> String {
    let mut markdown = String::new();
    let _ = writeln!(markdown, "# Smoke");
    let _ = writeln!(markdown, "profile: `{}`", output.profile);
    let _ = writeln!(markdown, "status: `{}`", output.status);
    let _ = writeln!(markdown, "project: `{}`", output.project);
    let _ = writeln!(markdown, "\n## Checked Surfaces");
    for surface in &output.checked_surfaces {
        let _ = writeln!(
            markdown,
            "- {} [{}] {} ({} ms)",
            surface.surface, surface.status, surface.detail, surface.duration_ms
        );
    }
    if !output.skipped_optional_surfaces.is_empty() {
        let _ = writeln!(markdown, "\n## Skipped Optional Surfaces");
        for surface in &output.skipped_optional_surfaces {
            let _ = writeln!(markdown, "- {}: {}", surface.surface, surface.reason);
        }
    }
    if !output.repair_hints.is_empty() {
        let _ = writeln!(markdown, "\n## Repair Hints");
        for hint in &output.repair_hints {
            let _ = writeln!(markdown, "- `{hint}`");
        }
    }
    markdown
}

#[cfg(test)]
mod session_map_tests {
    use super::*;
    use codestory_contracts::api::{GroundingOrientationDto, NodeId, NodeKind, StorageStatsDto};
    use codestory_runtime::{SessionMapRow, SessionMapSnapshot, SessionMapSymbol};

    #[test]
    fn session_map_renders_whole_native_groups_and_reports_omissions_with_utf8_budget() {
        let symbol = |name: String| SessionMapSymbol {
            id: NodeId("7".into()),
            name,
            kind: NodeKind::FUNCTION,
            line: Some(3),
            evidence_tier: None,
            evidence_producer: None,
            resolution_status: None,
        };
        let mut map = SessionMapSnapshot {
            project_root: "/project".into(),
            workspace_id: "workspace".into(),
            generation_id: "generation".into(),
            run_id: "run".into(),
            published_at_epoch_ms: 1,
            stats: StorageStatsDto {
                file_count: 2,
                node_count: 2,
                edge_count: 0,
                error_count: 1,
                fatal_error_count: 0,
            },
            orientation: GroundingOrientationDto::default(),
            selected_groups: 3,
            selected_roots: 4,
            omitted_groups: 1,
            gaps: vec!["file_id=9: content changed since publication".into()],
            rows: vec![
                SessionMapRow {
                    path: "a\\literal.rs".into(),
                    file_id: 1,
                    content_sha256: "a".repeat(64),
                    roots: vec![symbol("alpha".into()), symbol("beta".into())],
                    symbols: vec![],
                },
                SessionMapRow {
                    path: "large.rs".into(),
                    file_id: 2,
                    content_sha256: "b".repeat(64),
                    roots: vec![symbol("é".repeat(2000))],
                    symbols: vec![],
                },
            ],
        };
        let rendered = render_session_map(&map).unwrap();
        assert!(rendered.len() <= SESSION_MAP_MAX_BYTES);
        let lines = rendered.lines().collect::<Vec<_>>();
        let header: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(header["stats"]["error_count"], 1);
        assert_eq!(header["source_gaps"].as_array().unwrap().len(), 1);
        let row: serde_json::Value = serde_json::from_str(lines[2]).unwrap();
        assert_eq!(row["path"], "a\\literal.rs");
        assert_eq!(row["roots"][0]["name"], "alpha");
        assert_eq!(row["roots"][1]["name"], "beta");
        let footer: serde_json::Value = serde_json::from_str(lines.last().unwrap()).unwrap();
        assert_eq!(footer["shown_groups"], 1);
        assert_eq!(footer["omitted_groups"], 3);
        assert_eq!(footer["omitted_roots"], 2);
        map.rows.remove(0);
        assert!(
            render_session_map(&map).is_none(),
            "never slice an oversized whole group"
        );
    }
}
