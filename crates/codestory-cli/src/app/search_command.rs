use super::artifacts::{ensure_dot_only_for_trail, preflight_output_file};
use super::drill::search_output_from_results;
use super::lifecycle::{OpenedAgentSurface, open_agent_surface};
use super::to_api_repo_text_mode;
use crate::args::SearchCommand;
use crate::output::{RenderedPublicOutput, emit_public_operation, render_search_markdown};
use crate::runtime::map_api_error;
use anyhow::Result;
use codestory_contracts::api::SearchRequest;

pub(super) fn run_search(cmd: SearchCommand) -> Result<()> {
    ensure_dot_only_for_trail(cmd.format, "search")?;
    preflight_output_file(cmd.output_file.as_deref())?;
    let OpenedAgentSurface { runtime, .. } = open_agent_surface(
        &cmd.project,
        cmd.profile,
        cmd.run_id.as_deref(),
        cmd.refresh,
        "search",
    )?;
    let operation = runtime.run_public_operation("search", || {
        let search_results = runtime
            .browser
            .search_results(search_request_from_command(&cmd))
            .map_err(map_api_error)?;
        let output = search_output_from_results(&runtime, &search_results, cmd.why);
        let markdown = render_search_markdown(&runtime.project_root, &output);
        RenderedPublicOutput::structured(&output, markdown)
    })?;
    emit_public_operation(cmd.format, operation, cmd.output_file.as_deref())
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
