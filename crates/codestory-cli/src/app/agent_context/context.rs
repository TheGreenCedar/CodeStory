use super::super::artifacts::{
    ensure_dot_only_for_trail, preflight_output_file, write_context_bundle,
};
use super::super::bookmarks::load_bookmark_focus_by_id;
use super::super::lifecycle::{OpenedAgentSurface, open_agent_surface};
use super::super::rendering::build_query_resolution_output;
use super::super::resolution::resolve_target_or_emit_ambiguity;
use crate::args;
use crate::args::{ContextCommand, QueryResolutionOutput, QuerySelectorOutput};
use crate::display;
use crate::output::{
    RenderedPublicOutput, context_packet_json, emit_public_operation, render_context_markdown,
};
use crate::runtime;
use crate::runtime::{RuntimeContext, map_api_error};
use anyhow::{Result, bail};
use codestory_contracts::api::{
    AgentAnswerDto, AgentAskRequest, AgentResponseModeDto, AgentRetrievalPresetDto,
    AgentRetrievalProfileSelectionDto, BookmarkDto, NodeId,
};

#[derive(serde::Serialize)]
struct ContextTargetOutput {
    selector: QuerySelectorOutput,
    requested: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    bookmark_id: Option<String>,
}

#[derive(serde::Serialize)]
struct ContextJsonOutput {
    target: ContextTargetOutput,
    resolution: QueryResolutionOutput,
    context: serde_json::Value,
}

struct ResolvedContextTarget {
    target: runtime::ResolvedTarget,
    requested: String,
    selector: QuerySelectorOutput,
    bookmark: Option<BookmarkDto>,
}

pub(in crate::app) fn run_context(cmd: ContextCommand) -> Result<()> {
    ensure_dot_only_for_trail(cmd.format, "context")?;
    preflight_output_file(cmd.output_file.as_deref())?;
    let OpenedAgentSurface { runtime, .. } =
        open_agent_surface(&cmd.project, None, None, cmd.refresh, "context")?;

    let operation = runtime.run_public_operation("context", || {
        let resolved =
            resolve_context_target(&runtime, &cmd, cmd.format, cmd.output_file.as_deref())?;
        let target_prompt = context_target_prompt(&resolved);
        let request = AgentAskRequest {
            prompt: target_prompt,
            retrieval_profile: AgentRetrievalProfileSelectionDto::Preset {
                preset: AgentRetrievalPresetDto::Investigate,
            },
            focus_node_id: Some(resolved.target.selected.node_id.clone()),
            max_results: Some(cmd.max_results.clamp(1, 25)),
            response_mode: AgentResponseModeDto::Markdown,
            latency_budget_ms: None,
            include_evidence: !cmd.no_evidence,
            hybrid_weights: None,
        };

        let mut answer = runtime.browser.ask(request).map_err(map_api_error)?;
        answer
            .retrieval_trace
            .annotations
            .push("mode=db_first".to_string());
        annotate_answer_with_context_target(&mut answer, &resolved);
        let markdown = render_context_markdown(&runtime.project_root, &answer);
        let output = ContextJsonOutput {
            target: ContextTargetOutput {
                selector: resolved.selector,
                requested: resolved.requested,
                bookmark_id: resolved
                    .bookmark
                    .as_ref()
                    .map(|bookmark| bookmark.id.clone()),
            },
            resolution: build_query_resolution_output(&runtime.project_root, &resolved.target),
            context: context_packet_json(&answer),
        };
        let rendered = RenderedPublicOutput::structured(&output, markdown)?;
        Ok((answer, rendered))
    })?;
    if let Some(bundle_dir) = cmd.bundle.as_deref() {
        let (json, markdown) = operation
            .value
            .1
            .structured_parts()
            .expect("context always renders structured output");
        let json = runtime::public_operation_json_value(&operation, json)?;
        write_context_bundle(bundle_dir, &json, &operation.value.0.graphs, markdown)?;
    }
    let operation = runtime::map_public_operation(operation, |(_, rendered)| rendered);
    emit_public_operation(cmd.format, operation, cmd.output_file.as_deref())
}

fn resolve_context_target(
    runtime: &RuntimeContext,
    cmd: &ContextCommand,
    format: args::OutputFormat,
    output_file: Option<&std::path::Path>,
) -> Result<ResolvedContextTarget> {
    let bookmark = cmd
        .bookmark
        .as_deref()
        .map(|id| load_bookmark_focus_by_id(runtime, id))
        .transpose()?;
    let (selection, requested, selector) = if let Some(bookmark) = bookmark.as_ref() {
        (
            args::TargetSelection::Id(bookmark.node_id.clone()),
            bookmark.node_label.clone(),
            QuerySelectorOutput::Id,
        )
    } else if let Some(id) = cmd.id.as_deref() {
        let trimmed = id.trim();
        if trimmed.is_empty() {
            bail!("--id cannot be empty.");
        }
        (
            args::TargetSelection::Id(NodeId(trimmed.to_string())),
            trimmed.to_string(),
            QuerySelectorOutput::Id,
        )
    } else if let Some(query) = cmd.query.as_deref() {
        let trimmed = query.trim();
        if trimmed.is_empty() {
            bail!("--query cannot be empty.");
        }
        (
            args::TargetSelection::Query {
                query: trimmed.to_string(),
                choose: None,
            },
            trimmed.to_string(),
            QuerySelectorOutput::Query,
        )
    } else {
        bail!("Pass exactly one of --id, --query, or --bookmark.");
    };
    let target = resolve_target_or_emit_ambiguity(runtime, selection, None, format, output_file)?;
    Ok(ResolvedContextTarget {
        target,
        requested,
        selector,
        bookmark,
    })
}

fn context_target_prompt(resolved: &ResolvedContextTarget) -> String {
    let selected = resolved.target.selected.display_name.trim();
    if selected.is_empty() {
        resolved.requested.clone()
    } else {
        selected.to_string()
    }
}

fn annotate_answer_with_context_target(
    answer: &mut AgentAnswerDto,
    resolved: &ResolvedContextTarget,
) {
    let selected = &resolved.target.selected;
    answer.retrieval_trace.annotations.push(format!(
        "context_target selector={:?} requested=`{}` node={} label=`{}` kind={:?}",
        resolved.selector,
        resolved.requested.replace('`', "'"),
        selected.node_id.0,
        selected.display_name.replace('`', "'"),
        selected.kind
    ));
    if let Some(file_path) = selected.file_path.as_deref() {
        answer.retrieval_trace.annotations.push(format!(
            "context_target_location path=`{}` line={}",
            display::clean_path_string(file_path),
            selected
                .line
                .map(|line| line.to_string())
                .unwrap_or_else(|| "unknown".to_string())
        ));
    }
    if let Some(bookmark) = resolved.bookmark.as_ref() {
        annotate_context_with_bookmark_focus(answer, bookmark);
    }
}

fn annotate_context_with_bookmark_focus(answer: &mut AgentAnswerDto, bookmark: &BookmarkDto) {
    let mut annotation = format!(
        "bookmark_focus id={} category_id={} node={} label=`{}` kind={:?}",
        bookmark.id,
        bookmark.category_id,
        bookmark.node_id.0,
        bookmark.node_label,
        bookmark.node_kind
    );
    if let Some(file_path) = bookmark.file_path.as_deref() {
        annotation.push_str(&format!(
            " path=`{}`",
            display::clean_path_string(file_path)
        ));
    }
    if let Some(comment) = bookmark.comment.as_deref() {
        annotation.push_str(&format!(" comment=`{}`", comment.replace('`', "'")));
    }
    answer.retrieval_trace.annotations.push(annotation);
}
