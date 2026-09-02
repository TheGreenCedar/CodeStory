use super::artifacts::{ensure_dot_only_for_trail, preflight_output_file};
use super::lifecycle::{OpenedAgentSurface, open_search_surface};
use super::to_api_repo_text_mode;
use crate::args::SearchCommand;
use crate::output::{RenderedPublicOutput, emit_public_operation};
use crate::runtime::map_api_error;
use anyhow::Result;
use codestory_contracts::api::SearchRequest;

pub(super) fn run_search(cmd: SearchCommand) -> Result<()> {
    ensure_dot_only_for_trail(cmd.format, "search")?;
    preflight_output_file(cmd.output_file.as_deref())?;
    let repo_text = to_api_repo_text_mode(cmd.repo_text);
    let OpenedAgentSurface { runtime, .. } = open_search_surface(
        &cmd.project,
        cmd.profile,
        cmd.run_id.as_deref(),
        cmd.refresh,
        repo_text,
    )?;
    let operation = runtime.run_public_operation(
        codestory_runtime::search_operation_name(repo_text),
        || {
            let search_results = runtime
                .browser
                .search_results(search_request_from_command(&cmd))
                .map_err(map_api_error)?;
            let projection = codestory_runtime::project_search_v3(
                &runtime.public_operation,
                "codestory-cli",
                &search_results,
            )
            .map_err(map_api_error)?;
            let markdown = render_search_projection_markdown(&projection);
            RenderedPublicOutput::structured(&projection, markdown)
        },
    )?;
    emit_public_operation(cmd.format, operation, cmd.output_file.as_deref())
}

fn render_search_projection_markdown(
    search: &codestory_contracts::packet_projection_v3::SearchProjectionV3Dto,
) -> String {
    use std::fmt::Write as _;
    let mut markdown = String::from("# Search evidence\n\n");
    let _ = writeln!(
        markdown,
        "packet_id: `{}`",
        search.identity.packet_id.as_str()
    );
    let _ = writeln!(markdown, "status: `{:?}`", search.status);
    for row in search.evidence.as_slice() {
        let line = row
            .start_line
            .map_or(String::new(), |line| format!(":{line}"));
        let _ = writeln!(markdown, "- {}{line}", row.path.as_str());
    }
    if !search.gaps.as_slice().is_empty() {
        let _ = writeln!(markdown, "\n## Gaps");
        for gap in search.gaps.as_slice() {
            let _ = writeln!(markdown, "- {:?}", gap.kind);
        }
    }
    markdown
}

fn search_request_from_command(cmd: &SearchCommand) -> SearchRequest {
    SearchRequest {
        query: cmd.query.clone(),
        repo_text: to_api_repo_text_mode(cmd.repo_text),
        limit_per_source: cmd.limit.clamp(1, 50),
        expand_search_plan: cmd.why && cmd.plan_details,
        hybrid_weights: None,
        hybrid_limits: None,
    }
}
