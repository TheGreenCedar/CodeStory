use super::super::artifacts::{ensure_dot_only_for_trail, preflight_output_file};
use super::super::rendering::{
    build_query_resolution_output_from_occurrences, build_query_resolution_output_with_runtime,
};
use super::super::resolution::{
    resolve_target_or_emit_ambiguity, structured_ambiguous_target_failure,
};
use crate::args;
use crate::args::{QueryResolutionOutput, SymbolCommand, SymbolJsonOutput, SymbolWorkflowCommand};
use crate::output::{
    RenderedPublicOutput, emit_public_operation, render_symbol_markdown, render_symbol_mermaid,
};
use crate::runtime;
use crate::runtime::{AmbiguousTargetError, RuntimeContext, ensure_index_ready, map_api_error};
use anyhow::{Result, bail};
use codestory_contracts::api::TrailContextDto;
use std::fmt::Write as _;

pub(in crate::app) fn run_symbol(cmd: SymbolCommand) -> Result<()> {
    ensure_dot_only_for_trail(cmd.format, "symbol")?;
    preflight_output_file(cmd.output_file.as_deref())?;
    let runtime = RuntimeContext::new(&cmd.project)?;
    let opened = runtime.ensure_open(cmd.refresh)?;
    ensure_index_ready(&opened, "symbol")?;

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
        let context = runtime
            .browser
            .symbol_context(target.selected.node_id.clone())
            .map_err(map_api_error)?;
        let resolution = build_query_resolution_output_with_runtime(&runtime, &target);
        if cmd.mermaid {
            return Ok(RenderedPublicOutput::text(render_symbol_mermaid(&context)));
        }
        let verification_targets = resolution.resolved.verification_targets.clone();
        let markdown = render_symbol_markdown(
            &runtime.project_root,
            &target,
            &context,
            &verification_targets,
        );
        let output = SymbolJsonOutput {
            resolution,
            symbol: &context,
            verification_targets,
        };
        RenderedPublicOutput::structured(&output, markdown)
    })?;
    emit_public_operation(cmd.format, operation, cmd.output_file.as_deref())
}

#[derive(serde::Serialize)]
pub(super) struct SymbolWorkflowOutput<'a> {
    workflow: &'static str,
    project_root: &'a str,
    resolution: QueryResolutionOutput,
    symbol: &'a codestory_contracts::api::SymbolContextDto,
    direct_callers: &'a [codestory_runtime::SymbolWorkflowNode],
    transitive_callers: &'a [codestory_runtime::SymbolWorkflowNode],
    impacted_files: &'a [String],
    impacted_routes: &'a [codestory_runtime::SymbolWorkflowRoute],
    likely_tests: &'a [codestory_runtime::SymbolWorkflowTest],
    caps: &'a codestory_runtime::SymbolWorkflowCaps,
    unknowns: &'a [String],
    next_commands: &'a [String],
    #[serde(default, skip_serializing_if = "Option::is_none")]
    affected: Option<&'a codestory_contracts::api::AffectedAnalysisDto>,
    trail: &'a TrailContextDto,
}

pub(in crate::app) fn run_symbol_workflow(
    mode: codestory_runtime::SymbolWorkflowMode,
    cmd: SymbolWorkflowCommand,
) -> Result<()> {
    ensure_dot_only_for_trail(cmd.format, mode.label())?;
    preflight_output_file(cmd.output_file.as_deref())?;
    let runtime = RuntimeContext::new(&cmd.project)?;
    let opened = runtime.ensure_open(cmd.refresh)?;
    ensure_index_ready(&opened, mode.label())?;

    let file_filter = cmd.target.file_filter();
    let operation_name = if cmd.target.query.is_some() {
        "graph_assisted"
    } else {
        "graph"
    };
    let operation = runtime.run_public_operation(operation_name, || {
        let target = match cmd.target.selection()? {
            args::TargetSelection::Id(id) => codestory_runtime::TargetSelection::Id(id),
            args::TargetSelection::Query { query, choose } => {
                codestory_runtime::TargetSelection::Query { query, choose }
            }
        };
        let response = match runtime
            .browser
            .symbol_workflow(codestory_runtime::SymbolWorkflowRequest {
                mode,
                target,
                file_filter: file_filter.clone(),
                depth: cmd.depth,
                max_nodes: cmd.max_nodes,
                include_tests: cmd.include_tests,
            })
            .map_err(map_api_error)?
        {
            codestory_runtime::SymbolWorkflowOutcome::Complete(response) => *response,
            codestory_runtime::SymbolWorkflowOutcome::Ambiguous(ambiguous) => {
                return structured_ambiguous_target_failure(
                    &runtime,
                    AmbiguousTargetError {
                        query: ambiguous.query,
                        file_filter: ambiguous.file_filter,
                        alternatives: ambiguous.alternatives,
                        message: ambiguous.message,
                    },
                    cmd.format,
                    cmd.output_file.as_deref(),
                );
            }
            codestory_runtime::SymbolWorkflowOutcome::Rejected(message) => bail!(message),
        };
        let resolution_target =
            runtime::ResolvedTarget::from_runtime(response.resolution.target.clone());
        let resolution = build_query_resolution_output_from_occurrences(
            &runtime.project_root,
            &resolution_target,
            &response.resolution.occurrences,
        );
        let output = SymbolWorkflowOutput {
            workflow: response.workflow,
            project_root: &response.project_root,
            resolution,
            symbol: &response.symbol,
            direct_callers: &response.direct_callers,
            transitive_callers: &response.transitive_callers,
            impacted_files: &response.impacted_files,
            impacted_routes: &response.impacted_routes,
            likely_tests: &response.likely_tests,
            caps: &response.caps,
            unknowns: &response.unknowns,
            next_commands: &response.next_commands,
            affected: response.affected.as_ref(),
            trail: &response.trail,
        };
        let markdown = render_symbol_workflow_markdown(mode, &output);
        RenderedPublicOutput::structured(&output, markdown)
    })?;
    emit_public_operation(cmd.format, operation, cmd.output_file.as_deref())
}

pub(super) fn render_symbol_workflow_markdown(
    mode: codestory_runtime::SymbolWorkflowMode,
    output: &SymbolWorkflowOutput<'_>,
) -> String {
    let mut markdown = String::new();
    let _ = writeln!(markdown, "# {}", mode.title());
    let _ = writeln!(
        markdown,
        "symbol: {} [{}]",
        output.symbol.node.display_name, output.symbol.node.id.0
    );
    if let Some(path) = output.symbol.node.file_path.as_deref() {
        let line = output
            .symbol
            .node
            .start_line
            .map(|line| format!(":{line}"))
            .unwrap_or_default();
        let _ = writeln!(markdown, "source: {path}{line}");
    }
    let _ = writeln!(
        markdown,
        "caps: caller_depth={} caller_max_nodes={} affected_depth={} impacted_symbols<=200 impacted_routes<=100",
        output.caps.caller_depth, output.caps.caller_max_nodes, output.caps.affected_depth
    );

    append_symbol_workflow_nodes(&mut markdown, "direct_callers", output.direct_callers);
    append_symbol_workflow_nodes(
        &mut markdown,
        "transitive_callers",
        output.transitive_callers,
    );
    append_symbol_workflow_strings(&mut markdown, "impacted_files", output.impacted_files);

    let _ = writeln!(markdown, "impacted_routes:");
    if output.impacted_routes.is_empty() {
        let _ = writeln!(markdown, "- none");
    } else {
        for route in output.impacted_routes {
            let location = route
                .file_path
                .as_deref()
                .map(|path| {
                    route
                        .line
                        .map(|line| format!(" {path}:{line}"))
                        .unwrap_or_else(|| format!(" {path}"))
                })
                .unwrap_or_default();
            let _ = writeln!(
                markdown,
                "- {} {} -> {} [{}]{}",
                route.method, route.path, route.display_name, route.confidence, location
            );
            let _ = writeln!(markdown, "  reason: {}", route.reason);
        }
    }

    let _ = writeln!(markdown, "likely_tests:");
    if output.likely_tests.is_empty() {
        let _ = writeln!(markdown, "- none");
    } else {
        for test in output.likely_tests {
            let _ = writeln!(
                markdown,
                "- {} confidence={} graph_depth={} impacted_symbols={}",
                test.path, test.confidence, test.graph_depth, test.impacted_symbol_count
            );
            let _ = writeln!(markdown, "  reason: {}", test.reason);
        }
    }

    append_symbol_workflow_strings(&mut markdown, "unknowns", output.unknowns);
    append_symbol_workflow_strings(&mut markdown, "next_commands", output.next_commands);
    markdown
}

pub(in crate::app) fn append_symbol_workflow_nodes(
    markdown: &mut String,
    label: &str,
    nodes: &[codestory_runtime::SymbolWorkflowNode],
) {
    let _ = writeln!(markdown, "{label}:");
    if nodes.is_empty() {
        let _ = writeln!(markdown, "- none");
        return;
    }
    for node in nodes {
        let location = node
            .file_path
            .as_deref()
            .map(|path| format!(" {path}"))
            .unwrap_or_default();
        let _ = writeln!(
            markdown,
            "- [{}] {} ({}) depth={}{}",
            node.node_id.0, node.display_name, node.kind, node.depth, location
        );
    }
}

pub(super) fn append_symbol_workflow_strings(markdown: &mut String, label: &str, items: &[String]) {
    let _ = writeln!(markdown, "{label}:");
    if items.is_empty() {
        let _ = writeln!(markdown, "- none");
        return;
    }
    for item in items {
        let _ = writeln!(markdown, "- {item}");
    }
}
