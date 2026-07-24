//! Command-line integration entry point for CodeStory.
//!
//! This module is the thin command dispatch and shared lifecycle facade.
//! Command owners live in focused sibling modules; they parse types from
//! `args`, open project state through `RuntimeContext`, and emit through
//! `output` helpers so markdown, JSON, DOT, stdout, and `--output-file`
//! behavior stays consistent.
//!
//! User-facing command usage belongs in CLI help and external docs. Rustdoc in
//! this crate documents the code contracts that command handlers, output DTOs,
//! and local integration transports rely on.

use anyhow::{Context, Result};
use clap::Parser;
use codestory_contracts::api::{CommandFailureEnvelope, SearchRepoTextMode};
use std::{
    fs,
    process::ExitCode,
    time::{Duration, Instant},
};

use crate::{embedding_qualification, embedding_server_transport, explore, report, retrieval};

const AGENT_PREFLIGHT_LOCAL_REFRESH_FOREGROUND_BUDGET: Duration = Duration::from_secs(5);

use crate::args::{self, Cli, Command, RepoTextMode};
use crate::runtime;
#[cfg(test)]
use crate::stdio_catalog::{
    prompts_list_json as stdio_prompts_list_json,
    resource_templates_list_json as stdio_resource_templates_list_json,
    resources_list_json as stdio_resources_list_json, tools_list_json as stdio_tools_list_json,
};
pub(crate) use artifacts::preflight_output_file;
use resolution::{
    StructuredCommandFailure, command_failure_envelope, command_failure_message,
    emit_command_failure, generic_command_failure, json_output_requested, requested_output_file,
};

const MAX_DRILL_JOBS: usize = 8;

fn to_api_repo_text_mode(mode: RepoTextMode) -> SearchRepoTextMode {
    match mode {
        RepoTextMode::Auto => SearchRepoTextMode::Auto,
        RepoTextMode::On => SearchRepoTextMode::On,
        RepoTextMode::Off => SearchRepoTextMode::Off,
    }
}

fn from_api_repo_text_mode(mode: SearchRepoTextMode) -> RepoTextMode {
    match mode {
        SearchRepoTextMode::Auto => RepoTextMode::Auto,
        SearchRepoTextMode::On => RepoTextMode::On,
        SearchRepoTextMode::Off => RepoTextMode::Off,
    }
}

fn drill_read_only_jobs(requested: usize, refresh: args::RefreshMode) -> usize {
    if refresh == args::RefreshMode::None {
        normalize_drill_jobs(requested)
    } else {
        1
    }
}

fn normalize_drill_jobs(requested: usize) -> usize {
    let available = std::thread::available_parallelism()
        .map(|parallelism| parallelism.get())
        .unwrap_or(1);
    normalize_drill_jobs_with_limit(requested, available)
}

fn normalize_drill_jobs_with_limit(requested: usize, available: usize) -> usize {
    requested.clamp(1, MAX_DRILL_JOBS).min(available.max(1))
}

fn elapsed_ms(start: Instant) -> u64 {
    start.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
}

mod agent_context;
pub(crate) mod artifacts;
mod bookmarks;
pub(crate) mod diagnostics;
mod drill;
mod ground_smoke;
mod index_command;
mod lifecycle;
mod readiness_commands;
pub(crate) mod rendering;
pub(crate) mod resolution;
mod search_command;
mod server;
mod source_commands;

pub(crate) use agent_context::packet_sufficiency_label;
#[cfg(test)]
use agent_context::{
    build_task_brief_output, packet_budget_mode_label, packet_task_class_label,
    render_packet_markdown, render_task_brief_markdown,
};
#[cfg(test)]
use index_command::validate_index_watch_output_file;
#[cfg(test)]
use lifecycle::{
    embedding_client_transport_mode, map_embedding_preflight_error, open_agent_surface,
};
pub(crate) use readiness_commands::{
    attach_complete_publication, local_refresh_output_from_summary,
};
#[cfg(test)]
use readiness_commands::{
    build_agent_preflight_output, classify_local_refresh_failure_state,
    local_freshness_needs_refresh,
};
#[cfg(test)]
use runtime::map_api_error;

#[tokio::main]
pub async fn run() -> ExitCode {
    let raw_args = std::env::args_os().collect::<Vec<_>>();
    let json = json_output_requested(&raw_args);
    let cli = match Cli::try_parse_from(&raw_args) {
        Ok(cli) => cli,
        Err(error) => {
            if matches!(
                error.kind(),
                clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
            ) {
                let _ = error.print();
                return ExitCode::SUCCESS;
            }
            if json {
                let envelope = command_failure_envelope(
                    "invalid_arguments",
                    "cli_arguments",
                    error.to_string(),
                    serde_json::json!({"kind": format!("{:?}", error.kind())}),
                );
                emit_command_failure(&envelope, requested_output_file(&raw_args));
            } else {
                let _ = error.print();
            }
            return ExitCode::from(2);
        }
    };

    match run_cli(cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let structured = error.downcast_ref::<StructuredCommandFailure>();
            if json {
                let envelope = structured
                    .map(|failure| failure.envelope.clone())
                    .or_else(|| {
                        runtime::api_error_in_chain(&error)
                            .cloned()
                            .map(CommandFailureEnvelope::new)
                    })
                    .unwrap_or_else(|| generic_command_failure(&error));
                let output_file = structured
                    .and_then(|failure| failure.output_file.as_deref())
                    .or_else(|| requested_output_file(&raw_args));
                emit_command_failure(&envelope, output_file);
            } else {
                if let Some(failure) = structured
                    && let (Some(path), Some(markdown)) =
                        (failure.output_file.as_deref(), failure.markdown.as_deref())
                    && let Err(write_error) = fs::write(path, markdown)
                {
                    eprintln!("Error: failed to write {}: {write_error}", path.display());
                    return ExitCode::FAILURE;
                }
                eprintln!("Error: {}", command_failure_message(&error));
            }
            ExitCode::FAILURE
        }
    }
}

async fn run_cli(cli: Cli) -> Result<()> {
    if let Some(mode) = lifecycle::embedding_client_transport_mode(&cli.command) {
        embedding_server_transport::install_client_transport(mode)
            .context("install native embedding server transport")?;
    }
    match cli.command {
        Command::Index(cmd) => index_command::run_index(cmd),
        Command::Ground(cmd) => ground_smoke::run_ground(cmd),
        Command::Report(cmd) => report::run_report(cmd),
        Command::Context(cmd) => agent_context::run_context(cmd),
        Command::Packet(cmd) => agent_context::run_packet(cmd),
        Command::Task(cmd) => agent_context::run_task(cmd),
        Command::Doctor(cmd) => readiness_commands::run_doctor(cmd),
        Command::Ready(cmd) => readiness_commands::run_ready(cmd),
        Command::Smoke(cmd) => ground_smoke::run_smoke(cmd),
        Command::Agent(cmd) => readiness_commands::run_agent(cmd),
        Command::Cache(cmd) => lifecycle::run_cache(cmd),
        Command::Search(cmd) => search_command::run_search(cmd),
        Command::Drill(cmd) => drill::run_drill(cmd),
        Command::DrillSuite(cmd) => drill::run_drill_suite(cmd),
        Command::Symbol(cmd) => source_commands::run_symbol(cmd),
        Command::Impact(cmd) => {
            source_commands::run_symbol_workflow(codestory_runtime::SymbolWorkflowMode::Impact, cmd)
        }
        Command::TestMap(cmd) => source_commands::run_symbol_workflow(
            codestory_runtime::SymbolWorkflowMode::TestMap,
            cmd,
        ),
        Command::Trail(cmd) => source_commands::run_trail(cmd),
        Command::Callers(cmd) => source_commands::run_callers(cmd),
        Command::Callees(cmd) => source_commands::run_callees(cmd),
        Command::Trace(cmd) => source_commands::run_trace(cmd),
        Command::Snippet(cmd) => source_commands::run_snippet(cmd),
        Command::Query(cmd) => source_commands::run_query(cmd),
        Command::Explore(cmd) => explore::run_explore(cmd),
        Command::Files(cmd) => source_commands::run_files(cmd),
        Command::Affected(cmd) => source_commands::run_affected(cmd),
        Command::Bookmark(cmd) => bookmarks::run_bookmark(cmd),
        Command::Serve(cmd) => server::run_serve(cmd).await,
        Command::GenerateCompletions(cmd) => server::run_generate_completions(cmd),
        Command::Retrieval(cmd) => retrieval::run_retrieval(cmd),
        Command::InternalOwnedDelete(cmd) => lifecycle::run_internal_owned_delete(cmd),
        Command::InternalEmbeddingServer => {
            embedding_server_transport::run_internal_embedding_server()
        }
        Command::InternalEmbeddingQualificationWorker(cmd) => {
            embedding_qualification::run_internal_embedding_qualification_worker(cmd)
                .map_err(|error| anyhow::anyhow!("{error:#}"))
        }
    }
}

#[cfg(test)]
mod tests;
