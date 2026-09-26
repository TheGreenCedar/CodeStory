use super::super::artifacts::preflight_output_file;
use super::super::rendering::build_query_resolution_output_with_runtime;
use super::super::resolution::resolve_target_or_emit_ambiguity;
use crate::args;
use crate::args::{
    CliDirection, CliTrailMode, TrailCommand, TrailJsonOutput, build_trail_request,
    trail_caller_scope_wire_label,
};
use crate::output::{
    RenderedPublicOutput, emit_public_operation, render_trail_dot, render_trail_markdown,
    render_trail_mermaid, render_trail_story_markdown,
};
use crate::runtime::{RuntimeContext, ensure_index_ready, map_api_error};
use anyhow::{Result, bail};
use codestory_contracts::api::TrailCallerScope;
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
        let caller_scope = request.caller_scope;
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
        let notes = trail_guidance_notes(&context, caller_scope);
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
            caller_scope: trail_caller_scope_wire_label(caller_scope),
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
    caller_scope: TrailCallerScope,
) -> Vec<String> {
    if !context.trail.edges.is_empty() || context.trail.nodes.len() > 1 {
        return Vec::new();
    }
    if context.focus.file_path.is_none() {
        return Vec::new();
    }
    let (scope_label, control) = match caller_scope {
        TrailCallerScope::ProductionOnly => (
            "production-only caller scope (tests and benches excluded)",
            "Pass --include-tests to include test and benchmark callers",
        ),
        TrailCallerScope::IncludeTestsAndBenches => (
            "include-tests-and-benches caller scope",
            "This view already includes test and benchmark callers",
        ),
    };
    vec![format!(
        "Returned {scope_label} for `{}`. {control}. This filtered view is not a claim that no graph edges were indexed. truncated={} omitted_edge_count={}.",
        context.focus.display_name, context.trail.truncated, context.trail.omitted_edge_count
    )]
}

#[cfg(test)]
mod tests {
    use super::*;
    use codestory_contracts::api::{
        GraphNodeDto, GraphResponse, NodeDetailsDto, NodeId, NodeKind, TrailCallerScope,
        TrailContextDto,
    };

    fn empty_focus_trail(truncated: bool, omitted_edge_count: u32) -> TrailContextDto {
        let id = NodeId("9196608968427629473".to_string());
        TrailContextDto {
            focus: NodeDetailsDto {
                id: id.clone(),
                kind: NodeKind::FUNCTION,
                display_name: "test_entry".to_string(),
                serialized_name: "test_entry".to_string(),
                qualified_name: Some("test_entry".to_string()),
                canonical_id: None,
                file_path: Some("tests/test_flow.py".to_string()),
                start_line: Some(4),
                start_col: Some(1),
                end_line: Some(5),
                end_col: Some(18),
                evidence_tier: None,
                evidence_producer: None,
                resolution_status: None,
                member_access: None,
                route_endpoint: None,
            },
            trail: GraphResponse {
                center_id: id.clone(),
                nodes: vec![GraphNodeDto {
                    id,
                    label: "test_entry".to_string(),
                    kind: NodeKind::FUNCTION,
                    depth: 0,
                    label_policy: None,
                    badge_visible_members: None,
                    badge_total_members: None,
                    merged_symbol_examples: Vec::new(),
                    file_path: Some("tests/test_flow.py".to_string()),
                    qualified_name: Some("test_entry".to_string()),
                    member_access: None,
                }],
                edges: Vec::new(),
                truncated,
                omitted_edge_count,
                canonical_layout: None,
            },
            story: None,
        }
    }

    #[test]
    fn production_only_empty_view_does_not_claim_edges_were_unindexed() {
        let notes = trail_guidance_notes(
            &empty_focus_trail(false, 0),
            TrailCallerScope::ProductionOnly,
        );
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(
            notes[0].contains("production-only") && notes[0].contains("--include-tests"),
            "{notes:?}"
        );
        assert!(
            !notes[0].contains("No graph edges were indexed"),
            "{notes:?}"
        );
        assert!(
            notes[0].contains("truncated=false") && notes[0].contains("omitted_edge_count=0"),
            "filtering must stay distinct from truncation: {notes:?}"
        );
    }

    #[test]
    fn include_tests_empty_view_does_not_invent_omitted_counts() {
        let notes = trail_guidance_notes(
            &empty_focus_trail(true, 4),
            TrailCallerScope::IncludeTestsAndBenches,
        );
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(
            notes[0].contains("include-tests") || notes[0].contains("tests and benches"),
            "{notes:?}"
        );
        assert!(
            notes[0].contains("truncated=true") && notes[0].contains("omitted_edge_count=4"),
            "{notes:?}"
        );
        assert!(
            !notes[0].contains("No graph edges were indexed"),
            "{notes:?}"
        );
    }
}
