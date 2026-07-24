use super::super::artifacts::preflight_output_file;
use super::super::rendering::build_query_resolution_output_with_runtime;
use super::super::resolution::resolve_target_or_emit_ambiguity;
use crate::args;
use crate::args::{CliDirection, CliTrailMode, TrailCommand, TrailJsonOutput, build_trail_request};
use crate::output::{
    RenderedPublicOutput, emit_public_operation, render_trail_dot, render_trail_markdown,
    render_trail_mermaid, render_trail_story_markdown,
};
use crate::runtime::{RuntimeContext, ensure_index_ready, map_api_error};
use anyhow::{Result, bail};
use std::fmt::Write as _;

pub(in crate::app) fn run_trail(cmd: TrailCommand) -> Result<()> {
    preflight_output_file(cmd.output_file.as_deref())?;
    if cmd.story && cmd.mermaid {
        bail!("--story cannot be combined with --mermaid; use markdown or json output");
    }
    if cmd.story && cmd.format == args::OutputFormat::Dot {
        bail!("--story cannot be combined with --format dot; use markdown or json output");
    }
    let runtime = RuntimeContext::new(&cmd.project)?;
    let opened = runtime.ensure_open(cmd.refresh)?;
    ensure_index_ready(&opened, "trail")?;

    let file_filter = cmd.target.file_filter();
    let operation = if cmd.target.query.is_some() {
        "graph_assisted"
    } else {
        "graph"
    };
    let operation = runtime.run_public_operation(operation, || {
        let target = resolve_target_or_emit_ambiguity(
            &runtime,
            cmd.target.selection()?,
            file_filter.as_deref(),
            cmd.format,
            cmd.output_file.as_deref(),
        )?;
        let request = build_trail_request(&target.selected.node_id, &cmd);
        let context = runtime
            .browser
            .trail_context(request)
            .map_err(map_api_error)?;
        let resolution = build_query_resolution_output_with_runtime(&runtime, &target);
        if cmd.mermaid {
            return Ok(RenderedPublicOutput::text(render_trail_mermaid(&context)));
        }
        if cmd.format == args::OutputFormat::Dot {
            return Ok(RenderedPublicOutput::text(render_trail_dot(
                &runtime.project_root,
                &context,
            )));
        }
        let notes = trail_guidance_notes(&context);
        let mut markdown = if let Some(story) = context.story.as_ref() {
            render_trail_story_markdown(&runtime.project_root, &target, &context, &cmd, story)
        } else {
            render_trail_markdown(&runtime.project_root, &target, &context, &cmd)
        };
        if !notes.is_empty() {
            let _ = writeln!(markdown, "notes:");
            for note in &notes {
                let _ = writeln!(markdown, "- {note}");
            }
        }
        let output = TrailJsonOutput {
            resolution,
            trail: &context,
            notes,
        };
        RenderedPublicOutput::structured(&output, markdown)
    })?;
    emit_public_operation(cmd.format, operation, cmd.output_file.as_deref())
}

pub(in crate::app) fn run_callers(mut cmd: TrailCommand) -> Result<()> {
    cmd.mode = CliTrailMode::Referencing;
    cmd.direction = Some(CliDirection::Incoming);
    run_trail(cmd)
}

pub(in crate::app) fn run_callees(mut cmd: TrailCommand) -> Result<()> {
    cmd.mode = CliTrailMode::Referenced;
    cmd.direction = Some(CliDirection::Outgoing);
    run_trail(cmd)
}

pub(in crate::app) fn run_trace(mut cmd: TrailCommand) -> Result<()> {
    if !cmd.mermaid && cmd.format != args::OutputFormat::Dot {
        cmd.story = true;
    }
    run_trail(cmd)
}

pub(super) fn trail_guidance_notes(
    context: &codestory_contracts::api::TrailContextDto,
) -> Vec<String> {
    if !context.trail.edges.is_empty() || context.trail.nodes.len() > 1 {
        return Vec::new();
    }
    if context.focus.file_path.is_none() {
        return Vec::new();
    }
    vec![format!(
        "No graph edges were indexed for `{}`. For object/config exports, use `snippet --id {}` or `explore --id {}` to inspect fields, hooks, access rules, and imports.",
        context.focus.display_name, context.focus.id.0, context.focus.id.0
    )]
}
