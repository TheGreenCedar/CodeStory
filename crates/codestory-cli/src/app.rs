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

use anyhow::{Context, Result, bail};
use clap::Parser;
use codestory_contracts::api::{
    AffectedAnalysisInput, AffectedAnalysisRequest, AffectedChangeKindDto, AgentAnswerDto,
    AgentAskRequest, AgentPacketDto, AgentPacketRequestDto, AgentResponseModeDto,
    AgentRetrievalPresetDto, AgentRetrievalProfileSelectionDto, ApiError, ApiErrorDetails,
    AppEventPayload, BookmarkCategoryDto, BookmarkDto, CommandFailureEnvelope,
    CreateBookmarkCategoryRequest, CreateBookmarkRequest, GroundingBudgetDto,
    IndexFreshnessStatusDto, IndexMode, NodeId, NodeKind, PacketBudgetModeDto,
    PacketSufficiencyStatusDto, PacketTaskClassDto, ProjectSummary, ReadinessGoalDto,
    ReadinessStatusDto, SearchRepoTextMode, SearchRequest,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
    fs,
    process::ExitCode,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use crate::{
    display, embedding_qualification, embedding_server_transport, explore, local_refresh_status,
    readiness, report, retrieval,
};

const AGENT_PREFLIGHT_LOCAL_REFRESH_FOREGROUND_BUDGET: Duration = Duration::from_secs(5);

use crate::args::{
    self, BookmarkAction, BookmarkAddCommand, BookmarkAddOutput, BookmarkCommand,
    BookmarkListCommand, BookmarkListOutput, BookmarkOutput, BookmarkRemoveCommand,
    BookmarkRemoveOutput, CacheAction, CacheCommand, Cli, Command, ContextCommand, DoctorCommand,
    GroundCommand, IndexCommand, IndexDryRunOutput, IndexOutput, InternalOwnedDeleteCommand,
    PacketCommand, ProjectArgs, QueryResolutionOutput, QuerySelectorOutput, ReadinessLaneOutput,
    ReadyCommand, ReadyOutput, RepoTextMode, RetrievalStatusOutput, SearchCommand, SmokeCommand,
    SmokeProfile, TaskAction, TaskBriefCommand, TaskCommand,
};
use crate::output::{
    REPO_CONTENT_BOUNDARY_LINE, RenderedPublicOutput, context_packet_json, emit,
    emit_public_operation, render_agent_citation, render_context_markdown, render_doctor_markdown,
    render_ground_markdown, render_index_dry_run_markdown, render_index_markdown,
    render_ready_markdown, render_search_markdown,
};
use crate::runtime::{
    self, RuntimeContext, annotate_refresh_error, ensure_index_ready, index_mode_name,
    map_api_error, map_api_error_for_project, refresh_label, refresh_mode_name,
};
#[cfg(test)]
use crate::stdio_catalog::{
    prompts_list_json as stdio_prompts_list_json,
    resource_templates_list_json as stdio_resource_templates_list_json,
    resources_list_json as stdio_resources_list_json, tools_list_json as stdio_tools_list_json,
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
    if let Some(mode) = embedding_client_transport_mode(&cli.command) {
        embedding_server_transport::install_client_transport(mode)
            .context("install native embedding server transport")?;
    }
    match cli.command {
        Command::Index(cmd) => run_index(cmd),
        Command::Ground(cmd) => run_ground(cmd),
        Command::Report(cmd) => report::run_report(cmd),
        Command::Context(cmd) => run_context(cmd),
        Command::Packet(cmd) => run_packet(cmd),
        Command::Task(cmd) => run_task(cmd),
        Command::Doctor(cmd) => run_doctor(cmd),
        Command::Ready(cmd) => run_ready(cmd),
        Command::Smoke(cmd) => run_smoke(cmd),
        Command::Agent(cmd) => run_agent(cmd),
        Command::Cache(cmd) => run_cache(cmd),
        Command::Search(cmd) => run_search(cmd),
        Command::Drill(cmd) => run_drill(cmd),
        Command::DrillSuite(cmd) => run_drill_suite(cmd),
        Command::Symbol(cmd) => run_symbol(cmd),
        Command::Impact(cmd) => {
            run_symbol_workflow(codestory_runtime::SymbolWorkflowMode::Impact, cmd)
        }
        Command::TestMap(cmd) => {
            run_symbol_workflow(codestory_runtime::SymbolWorkflowMode::TestMap, cmd)
        }
        Command::Trail(cmd) => run_trail(cmd),
        Command::Callers(cmd) => run_callers(cmd),
        Command::Callees(cmd) => run_callees(cmd),
        Command::Trace(cmd) => run_trace(cmd),
        Command::Snippet(cmd) => run_snippet(cmd),
        Command::Query(cmd) => run_query(cmd),
        Command::Explore(cmd) => explore::run_explore(cmd),
        Command::Files(cmd) => run_files(cmd),
        Command::Affected(cmd) => run_affected(cmd),
        Command::Bookmark(cmd) => run_bookmark(cmd),
        Command::Serve(cmd) => run_serve(cmd).await,
        Command::GenerateCompletions(cmd) => run_generate_completions(cmd),
        Command::Retrieval(cmd) => retrieval::run_retrieval(cmd),
        Command::InternalOwnedDelete(cmd) => run_internal_owned_delete(cmd),
        Command::InternalEmbeddingServer => {
            embedding_server_transport::run_internal_embedding_server()
        }
        Command::InternalEmbeddingQualification(cmd) => {
            embedding_qualification::run_internal_embedding_qualification(cmd)
                .map_err(|error| anyhow::anyhow!("{error:#}"))
        }
        Command::InternalEmbeddingQualificationWorker(cmd) => {
            embedding_qualification::run_internal_embedding_qualification_worker(cmd)
                .map_err(|error| anyhow::anyhow!("{error:#}"))
        }
    }
}

fn embedding_client_transport_mode(
    command: &Command,
) -> Option<embedding_server_transport::ClientTransportMode> {
    match command {
        Command::Ground(_) => Some(embedding_server_transport::ClientTransportMode::ObserveOnly),
        Command::Retrieval(args::RetrievalCommand {
            action: args::RetrievalAction::Status(_),
        }) => Some(embedding_server_transport::ClientTransportMode::ObserveOnly),
        Command::InternalEmbeddingServer => None,
        // This is deliberately an allowlist for attested observe-only capture. New commands retain
        // fresh exact executable identity unless their transport behavior is reviewed explicitly.
        _ => Some(embedding_server_transport::ClientTransportMode::SpawnCapable),
    }
}

fn run_internal_owned_delete(cmd: InternalOwnedDeleteCommand) -> Result<()> {
    let deletion = codestory_workspace::owned_deletion::OwnedDeletionRoot::open(&cmd.root)
        .with_context(|| format!("open owned deletion root {}", cmd.root.display()))?;
    deletion.remove(&cmd.relative).with_context(|| {
        format!(
            "remove owned relative path {} below {}",
            cmd.relative.display(),
            cmd.root.display()
        )
    })?;
    Ok(())
}

fn new_agent_surface_runtime(
    project: &ProjectArgs,
    profile: Option<args::CliSidecarProfile>,
    run_id: Option<&str>,
) -> Result<RuntimeContext> {
    RuntimeContext::new_agent_sidecar_with_selection(project, profile, run_id)
}

struct OpenedAgentSurface {
    runtime: RuntimeContext,
    before: Option<ProjectSummary>,
    opened: runtime::OpenedProject,
}

fn open_agent_surface(
    project: &ProjectArgs,
    profile: Option<args::CliSidecarProfile>,
    run_id: Option<&str>,
    refresh: args::RefreshMode,
    surface: &'static str,
) -> Result<OpenedAgentSurface> {
    let runtime = new_agent_surface_runtime(project, profile, run_id)?;
    let (before, opened) = runtime.ensure_open_with_before(refresh)?;
    ensure_index_ready(&opened, surface)?;
    codestory_retrieval::ensure_product_embedding_backend_for_runtime(&runtime.sidecar)
        .map_err(map_embedding_preflight_error)
        .with_context(|| format!("initialize retrieval for {surface}"))?;
    Ok(OpenedAgentSurface {
        runtime,
        before,
        opened,
    })
}

fn map_embedding_preflight_error(error: anyhow::Error) -> anyhow::Error {
    codestory_runtime::embedding_api_error(&error).map_or(error, map_api_error)
}

fn run_cache(cmd: CacheCommand) -> Result<()> {
    match cmd.action {
        CacheAction::Identity(cmd) => run_cache_identity(cmd),
        CacheAction::Rehydrate(cmd) => run_cache_rehydrate(cmd),
    }
}

fn run_cache_identity(cmd: args::CacheIdentityCommand) -> Result<()> {
    ensure_dot_only_for_trail(cmd.format, "cache identity")?;
    preflight_output_file(cmd.output_file.as_deref())?;
    let runtime = RuntimeContext::new_inspect_only(&cmd.project)?;
    let output = codestory_runtime::inspect_repository_identity(&runtime.project_root);
    let markdown = render_cache_identity_markdown(&output);
    emit(cmd.format, &output, markdown, cmd.output_file.as_deref())
}

fn render_cache_identity_markdown(output: &codestory_runtime::RepositoryIdentityReport) -> String {
    let mut markdown = String::new();
    let _ = writeln!(markdown, "# Cache Identity");
    let _ = writeln!(markdown, "project: `{}`", output.project);
    let _ = writeln!(
        markdown,
        "project_identity_schema_version: `{}`",
        output.project_identity_schema_version
    );
    let _ = writeln!(markdown, "project_id: `{}`", output.project_id);
    let _ = writeln!(markdown, "workspace_id: `{}`", output.workspace_id);
    let _ = writeln!(
        markdown,
        "artifact_scope_id: `{}`",
        output.artifact_scope_id
    );
    let _ = writeln!(
        markdown,
        "root_derived_project_id: `{}`",
        output.root_derived_project_id
    );
    let _ = writeln!(
        markdown,
        "canonical_repository_id: `{}`",
        output
            .canonical_repository_id
            .as_deref()
            .unwrap_or("unavailable")
    );
    let _ = writeln!(
        markdown,
        "repository_identity_schema_version: `{}`",
        output.repository_identity_schema_version
    );
    let _ = writeln!(
        markdown,
        "normalized_repository_identity: `{}`",
        output
            .normalized_repository_identity
            .as_deref()
            .unwrap_or("unavailable")
    );
    let _ = writeln!(
        markdown,
        "legacy_alias_disposition: `{}`",
        output.legacy_alias_disposition
    );
    let _ = writeln!(
        markdown,
        "legacy_project_id: `{}`",
        output.legacy_project_id.as_deref().unwrap_or("unavailable")
    );
    let _ = writeln!(
        markdown,
        "git_remote: `{}`",
        output.git_remote.as_deref().unwrap_or("unavailable")
    );
    let _ = writeln!(
        markdown,
        "git_tree: `{}`",
        output.git_tree.as_deref().unwrap_or("unavailable")
    );
    let _ = writeln!(
        markdown,
        "cache_schema_version: `{}`",
        output.cache_schema_version
    );
    let _ = writeln!(
        markdown,
        "portable_reuse_eligible: `{}`",
        output.portable_reuse_eligible
    );
    let _ = writeln!(
        markdown,
        "portable_reuse_reason: `{}`",
        output.portable_reuse_reason
    );
    let _ = writeln!(markdown, "freshness_inputs:");
    for input in &output.freshness_inputs {
        let _ = writeln!(markdown, "- `{input}`");
    }
    markdown
}

fn run_cache_rehydrate(cmd: args::CacheRehydrateCommand) -> Result<()> {
    ensure_dot_only_for_trail(cmd.format, "cache rehydrate")?;
    preflight_output_file(cmd.output_file.as_deref())?;
    let source_args = ProjectArgs {
        project: cmd.from_project,
        cache_dir: cmd.from_cache_dir,
    };
    let source = RuntimeContext::new_inspect_only(&source_args)?;
    let target = RuntimeContext::new_inspect_only(&cmd.project)?;
    let output = codestory_runtime::rehydrate_cache(codestory_runtime::CacheRehydrateRequest {
        source_project: &source.project_root,
        source_cache_dir: &source.cache_root,
        target_project: &target.project_root,
        target_cache_dir: &target.cache_root,
        dry_run: cmd.dry_run,
    })?;
    let markdown = render_cache_rehydrate_markdown(&output);
    emit(cmd.format, &output, markdown, cmd.output_file.as_deref())
}

fn render_cache_rehydrate_markdown(output: &codestory_runtime::CacheRehydrateOutput) -> String {
    let mut markdown = String::new();
    let _ = writeln!(markdown, "# Cache Rehydrate");
    let _ = writeln!(markdown, "status: `{}`", output.status);
    if let Some(reason) = output.reason.as_deref() {
        let _ = writeln!(markdown, "reason: {reason}");
    }
    let _ = writeln!(markdown, "source_project: `{}`", output.source_project);
    let _ = writeln!(markdown, "target_project: `{}`", output.target_project);
    let _ = writeln!(markdown, "source_cache: `{}`", output.source_cache_dir);
    let _ = writeln!(markdown, "target_cache: `{}`", output.target_cache_dir);
    if let Some(schema_version) = output.schema_version {
        let _ = writeln!(markdown, "schema_version: `{schema_version}`");
    }
    if let Some(source_file_count) = output.source_file_count {
        let _ = writeln!(markdown, "source_files: `{source_file_count}`");
    }
    let _ = writeln!(markdown, "copied: `{}`", output.copied);
    let _ = writeln!(markdown, "preserved_scope: `{}`", output.preserved_scope);
    let _ = writeln!(
        markdown,
        "invalidated_retrieval_manifests: `{}`",
        output.invalidated_retrieval_manifests
    );
    let _ = writeln!(
        markdown,
        "invalidated_index_artifact_rows: `{}`",
        output.invalidated_index_artifact_rows
    );
    let _ = writeln!(
        markdown,
        "rebased_path_bound_rows: `{}`",
        output.rebased_path_bound_rows
    );
    let _ = writeln!(markdown, "retrieval: {}", output.retrieval);
    let _ = writeln!(markdown, "retrieval_status: `{}`", output.retrieval_status);
    let _ = writeln!(markdown, "retrieval_reason: {}", output.retrieval_reason);
    if let Some(command) = output.retrieval_next_command.as_deref() {
        let _ = writeln!(markdown, "retrieval_next_command: `{command}`");
    }
    if !output.next_commands.is_empty() {
        let _ = writeln!(markdown, "next_commands:");
        for command in &output.next_commands {
            let _ = writeln!(markdown, "- `{command}`");
        }
    }
    markdown
}

fn run_index(cmd: IndexCommand) -> Result<()> {
    ensure_dot_only_for_trail(cmd.format, "index")?;
    preflight_output_file(cmd.output_file.as_deref())?;
    validate_index_watch_output_file(&cmd)?;
    run_index_once(&cmd)?;
    if cmd.watch {
        run_index_watch(cmd)?;
    }
    Ok(())
}

fn validate_index_watch_output_file(cmd: &IndexCommand) -> Result<()> {
    if !cmd.watch {
        return Ok(());
    }
    let Some(output_file) = cmd.output_file.as_deref() else {
        return Ok(());
    };

    let project_root = fs::canonicalize(&cmd.project.project).with_context(|| {
        format!(
            "Failed to resolve project root {}",
            display::clean_path_string(&cmd.project.project.to_string_lossy())
        )
    })?;
    let output_path = if output_file.is_absolute() {
        output_file.to_path_buf()
    } else {
        std::env::current_dir()
            .context("Failed to resolve current directory")?
            .join(output_file)
    };
    let Some(output_parent) = output_path.parent() else {
        return Ok(());
    };
    if !output_parent.exists() {
        return Ok(());
    }
    let resolved_parent = fs::canonicalize(output_parent).with_context(|| {
        format!(
            "Failed to resolve output parent {}",
            display::clean_path_string(&output_parent.to_string_lossy())
        )
    })?;
    let resolved_output = output_path
        .file_name()
        .map(|file_name| resolved_parent.join(file_name))
        .unwrap_or(resolved_parent);

    if resolved_output.starts_with(&project_root) {
        bail!(
            "--watch cannot write --output-file inside the watched project tree: {}",
            display::clean_path_string(&resolved_output.to_string_lossy())
        );
    }

    Ok(())
}

fn run_index_once(cmd: &IndexCommand) -> Result<()> {
    let runtime = if cmd.dry_run {
        RuntimeContext::new_inspect_only(&cmd.project)?
    } else {
        RuntimeContext::new(&cmd.project)?
    };
    if cmd.dry_run {
        let decision = runtime.resolve_refresh_decision_with_preflight(cmd.refresh)?;
        let refresh_mode = decision.effective_mode.unwrap_or(IndexMode::Incremental);
        let dry_run = runtime.index.dry_run_index(refresh_mode).map_err(|error| {
            map_api_error_for_project(
                annotate_refresh_error(error, cmd.refresh, refresh_mode),
                &runtime.project_root,
            )
        })?;
        let output = IndexDryRunOutput {
            requested_refresh: refresh_mode_name(cmd.refresh),
            effective_refresh: index_mode_name(refresh_mode),
            compatibility_reason: decision.reason.as_deref(),
            dry_run: &dry_run,
        };
        let markdown = render_index_dry_run_markdown(&output);
        return emit(cmd.format, &output, markdown, cmd.output_file.as_deref());
    }

    let progress = if cmd.progress {
        Some(spawn_progress_printer(runtime.events.clone()))
    } else {
        None
    };
    let opened = runtime.ensure_open(cmd.refresh)?;
    if let Some(progress) = progress {
        progress.finish();
    }
    let summary_generation = if cmd.summarize {
        Some(
            runtime
                .index
                .summarize_symbols_blocking()
                .map_err(map_api_error)?,
        )
    } else {
        None
    };
    let retrieval = opened
        .summary
        .retrieval
        .as_ref()
        .context("Open project summary did not include retrieval state")?;
    let refresh_label = refresh_label(cmd.refresh, opened.refresh_mode);
    let storage_path = runtime.storage_path.to_string_lossy().to_string();
    let sidecar_retrieval = doctor_sidecar_status(&runtime);
    let readiness = build_summary_readiness(
        &opened.summary.root,
        &opened.summary.stats,
        opened.summary.freshness.as_ref(),
        &sidecar_retrieval,
    );
    let next_commands = readiness::compatibility_next_commands(&readiness);
    let output = IndexOutput {
        project: &opened.summary.root,
        storage_path: &storage_path,
        refresh: &refresh_label,
        refresh_reason: opened.refresh_reason.as_deref(),
        summary: &opened.summary,
        retrieval,
        phase_timings: opened.phase_timings.as_ref(),
        summary_generation: summary_generation.as_ref(),
        readiness,
        next_commands,
    };

    let markdown = render_index_markdown(&output);
    emit(cmd.format, &output, markdown, cmd.output_file.as_deref())
}

struct ProgressPrinter {
    done: Arc<AtomicBool>,
    handle: std::thread::JoinHandle<()>,
}

impl ProgressPrinter {
    fn finish(self) {
        self.done.store(true, Ordering::SeqCst);
        let _ = self.handle.join();
    }
}

fn spawn_progress_printer(rx: crossbeam_channel::Receiver<AppEventPayload>) -> ProgressPrinter {
    let done = Arc::new(AtomicBool::new(false));
    let worker_done = Arc::clone(&done);
    let handle = std::thread::spawn(move || {
        while !worker_done.load(Ordering::SeqCst) {
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(event) => print_progress_event(event),
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
            }
        }
    });
    ProgressPrinter { done, handle }
}

fn print_progress_event(event: AppEventPayload) {
    match event {
        AppEventPayload::IndexingProgress { current, total } => {
            eprintln!(
                "[{current}/{total}] {} indexing",
                format_progress_bar(current, total)
            );
        }
        AppEventPayload::IndexingStarted { file_count } => {
            eprintln!(
                "[0/{file_count}] {} indexing started",
                format_progress_bar(0, file_count)
            );
        }
        _ => {}
    }
}

fn format_progress_bar(current: u32, total: u32) -> String {
    const WIDTH: u32 = 18;
    let filled = if total == 0 {
        0
    } else {
        current.saturating_mul(WIDTH) / total.max(1)
    }
    .min(WIDTH);
    format!(
        "[{}{}]",
        "#".repeat(filled as usize),
        "-".repeat(WIDTH.saturating_sub(filled) as usize)
    )
}

fn run_index_watch(mut cmd: IndexCommand) -> Result<()> {
    use notify::{RecursiveMode, Watcher};

    cmd.dry_run = false;
    cmd.refresh = args::RefreshMode::Incremental;
    let (tx, rx) = std::sync::mpsc::channel();
    let mut watcher = notify::recommended_watcher(move |event| {
        let _ = tx.send(event);
    })?;
    watcher.watch(&cmd.project.project, RecursiveMode::Recursive)?;
    eprintln!(
        "watching {} for changes; press Ctrl+C to stop",
        cmd.project.project.display()
    );
    loop {
        match rx.recv() {
            Ok(Ok(_event)) => {
                std::thread::sleep(Duration::from_millis(250));
                while rx.try_recv().is_ok() {}
                eprintln!("change detected; running incremental index");
                run_index_once(&cmd)?;
            }
            Ok(Err(error)) => eprintln!("watch error: {error}"),
            Err(error) => anyhow::bail!("watch channel closed: {error}"),
        }
    }
}

fn run_ground(cmd: GroundCommand) -> Result<()> {
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

fn run_smoke(cmd: SmokeCommand) -> Result<()> {
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
                coverage_gaps: Vec::new(),
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

fn run_context(cmd: ContextCommand) -> Result<()> {
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

fn run_packet(cmd: PacketCommand) -> Result<()> {
    ensure_dot_only_for_trail(cmd.format, "packet")?;
    preflight_output_file(cmd.output_file.as_deref())?;
    args::validate_packet_probe_arguments(&cmd.probes, &cmd.extra_probes)
        .map_err(anyhow::Error::msg)?;
    let OpenedAgentSurface { runtime, .. } = open_agent_surface(
        &cmd.project,
        cmd.profile,
        cmd.run_id.as_deref(),
        cmd.refresh,
        "packet",
    )?;

    let operation = runtime.run_public_operation("packet", || {
        let packet = runtime
            .browser
            .packet(AgentPacketRequestDto {
                question: cmd.question.clone(),
                budget: cmd.budget.into(),
                task_class: cmd.task_class.map(Into::into),
                probes: cmd.probes.clone(),
                extra_probes: cmd.extra_probes.clone(),
                include_evidence: !cmd.no_evidence,
                latency_budget_ms: cmd.latency_budget_ms,
            })
            .map_err(map_api_error)?;
        let step_trace = if cmd.step_trace_out.is_some() {
            let trace = codestory_runtime::packet_step_trace_json(&packet.answer);
            Some(serde_json::to_string_pretty(&trace)?)
        } else {
            None
        };
        let markdown = render_packet_markdown(&runtime.project_root, &packet);
        Ok((
            RenderedPublicOutput::structured(&packet, markdown)?,
            step_trace,
        ))
    })?;
    if let (Some(path), Some(trace)) = (&cmd.step_trace_out, &operation.value.1) {
        std::fs::write(path, trace)?;
    }
    let operation = runtime::map_public_operation(operation, |(rendered, _)| rendered);
    emit_public_operation(cmd.format, operation, cmd.output_file.as_deref())
}

fn run_task(cmd: TaskCommand) -> Result<()> {
    match cmd.action {
        TaskAction::Brief(cmd) => run_task_brief(cmd),
    }
}

fn run_task_brief(cmd: TaskBriefCommand) -> Result<()> {
    ensure_dot_only_for_trail(cmd.format, "task brief")?;
    preflight_output_file(cmd.output_file.as_deref())?;
    args::validate_packet_probe_arguments(&cmd.probes, &cmd.extra_probes)
        .map_err(anyhow::Error::msg)?;
    let OpenedAgentSurface { runtime, .. } =
        open_agent_surface(&cmd.project, None, None, cmd.refresh, "task brief")?;

    let operation = runtime.run_public_operation("packet", || {
        let packet = runtime
            .browser
            .packet(AgentPacketRequestDto {
                question: cmd.prompt.clone(),
                budget: cmd.budget.into(),
                task_class: Some(PacketTaskClassDto::EditPlanning),
                probes: cmd.probes.clone(),
                extra_probes: cmd.extra_probes.clone(),
                include_evidence: !cmd.no_evidence,
                latency_budget_ms: cmd.latency_budget_ms,
            })
            .map_err(map_api_error)?;
        let brief = build_task_brief_output(&runtime.project_root, &packet);
        let markdown = render_task_brief_markdown(&brief);
        RenderedPublicOutput::structured(&brief, markdown)
    })?;
    emit_public_operation(cmd.format, operation, cmd.output_file.as_deref())
}

#[derive(Debug, serde::Serialize)]
struct TaskBriefOutput {
    task_brief_version: u32,
    prompt: String,
    status: String,
    source_packet_id: String,
    source_packet_sufficiency: String,
    first_files: Vec<TaskBriefFileOutput>,
    relevant_symbols: Vec<TaskBriefSymbolOutput>,
    likely_tests: Vec<TaskBriefFileOutput>,
    impacted_surfaces: Vec<String>,
    risks_unknowns: Vec<String>,
    follow_up_codestory_commands: Vec<String>,
    future_sections: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
struct TaskBriefFileOutput {
    path: String,
    line: Option<u32>,
    reason: String,
}

#[derive(Debug, Clone, serde::Serialize)]
struct TaskBriefSymbolOutput {
    name: String,
    kind: String,
    path: Option<String>,
    line: Option<u32>,
    reason: String,
}

fn build_task_brief_output(
    project_root: &std::path::Path,
    packet: &AgentPacketDto,
) -> TaskBriefOutput {
    let citations = packet_task_brief_citations(packet);
    let first_files = task_brief_first_files(&citations);
    let relevant_symbols = task_brief_relevant_symbols(&citations);
    let likely_tests = task_brief_likely_tests(&citations);
    let impacted_surfaces = task_brief_impacted_surfaces(&first_files, &relevant_symbols);
    let risks_unknowns = task_brief_risks_unknowns(packet, &likely_tests);
    let follow_up_codestory_commands =
        task_brief_follow_up_commands(project_root, packet, &first_files, &relevant_symbols);

    TaskBriefOutput {
        task_brief_version: 1,
        prompt: packet.question.clone(),
        status: packet_operator_status(packet.sufficiency.status).to_string(),
        source_packet_id: packet.packet_id.clone(),
        source_packet_sufficiency: packet_sufficiency_label(packet.sufficiency.status).to_string(),
        first_files,
        relevant_symbols,
        likely_tests,
        impacted_surfaces,
        risks_unknowns,
        follow_up_codestory_commands,
        future_sections: vec![
            "scout".to_string(),
            "where".to_string(),
            "onboard".to_string(),
        ],
    }
}

fn packet_task_brief_citations(
    packet: &AgentPacketDto,
) -> Vec<&codestory_contracts::api::AgentCitationDto> {
    let mut citations = Vec::new();
    for claim in &packet.sufficiency.covered_claims {
        citations.extend(claim.citations.iter());
    }
    citations.extend(packet.answer.citations.iter());
    citations
}

fn task_brief_first_files(
    citations: &[&codestory_contracts::api::AgentCitationDto],
) -> Vec<TaskBriefFileOutput> {
    let mut seen = BTreeSet::new();
    let mut files = Vec::new();
    for citation in citations {
        let Some(path) = citation.file_path.as_deref() else {
            continue;
        };
        if seen.insert(path.to_string()) {
            files.push(TaskBriefFileOutput {
                path: path.to_string(),
                line: citation.line,
                reason: "cited by source packet".to_string(),
            });
        }
        if files.len() >= 8 {
            break;
        }
    }
    files
}

fn task_brief_relevant_symbols(
    citations: &[&codestory_contracts::api::AgentCitationDto],
) -> Vec<TaskBriefSymbolOutput> {
    let mut seen = BTreeSet::new();
    let mut symbols = Vec::new();
    for citation in citations {
        let key = format!(
            "{}:{}:{}",
            citation.display_name,
            citation.file_path.as_deref().unwrap_or(""),
            citation.line.unwrap_or(0)
        );
        if seen.insert(key) {
            symbols.push(TaskBriefSymbolOutput {
                name: citation.display_name.clone(),
                kind: display::format_kind(citation.kind),
                path: citation.file_path.clone(),
                line: citation.line,
                reason: "cited by source packet".to_string(),
            });
        }
        if symbols.len() >= 12 {
            break;
        }
    }
    symbols
}

fn task_brief_likely_tests(
    citations: &[&codestory_contracts::api::AgentCitationDto],
) -> Vec<TaskBriefFileOutput> {
    let mut seen = BTreeSet::new();
    let mut tests = Vec::new();
    for citation in citations {
        let Some(path) = citation.file_path.as_deref() else {
            continue;
        };
        if task_brief_path_is_test(path) && seen.insert(path.to_string()) {
            tests.push(TaskBriefFileOutput {
                path: path.to_string(),
                line: citation.line,
                reason: "test-like cited file".to_string(),
            });
        }
        if tests.len() >= 6 {
            break;
        }
    }
    tests
}

fn task_brief_path_is_test(path: &str) -> bool {
    let normalized = path.replace('\\', "/").to_ascii_lowercase();
    normalized.contains("/tests/")
        || normalized.ends_with("_test.rs")
        || normalized.ends_with("_tests.rs")
        || normalized.ends_with(".test.ts")
        || normalized.ends_with(".spec.ts")
        || normalized.ends_with(".test.js")
        || normalized.ends_with(".spec.js")
}

fn task_brief_impacted_surfaces(
    first_files: &[TaskBriefFileOutput],
    symbols: &[TaskBriefSymbolOutput],
) -> Vec<String> {
    let mut surfaces = BTreeSet::new();
    for path in first_files
        .iter()
        .map(|file| file.path.as_str())
        .chain(symbols.iter().filter_map(|symbol| symbol.path.as_deref()))
    {
        surfaces.insert(task_brief_surface_for_path(path));
    }
    surfaces.into_iter().take(8).collect()
}

fn task_brief_surface_for_path(path: &str) -> String {
    let normalized = path.replace('\\', "/");
    let mut parts = normalized.split('/');
    match (parts.next(), parts.next()) {
        (Some("crates"), Some(crate_name)) => format!("crates/{crate_name}"),
        (Some(first), Some(second)) if first == "plugins" => format!("{first}/{second}"),
        (Some(first), _) if !first.is_empty() => first.to_string(),
        _ => "unknown".to_string(),
    }
}

fn task_brief_risks_unknowns(
    packet: &AgentPacketDto,
    likely_tests: &[TaskBriefFileOutput],
) -> Vec<String> {
    let mut risks = packet.sufficiency.gaps.clone();
    if packet.budget.truncated {
        risks.push(format!(
            "source packet was budget-truncated; omitted sections: {}",
            packet_budget_omitted_sections(packet)
        ));
    }
    if likely_tests.is_empty() {
        risks.push("no test files were cited by the source packet".to_string());
    }
    if risks.is_empty() {
        risks.push("none from packet sufficiency; verify cited files before editing".to_string());
    }
    risks
}

fn task_brief_follow_up_commands(
    project_root: &std::path::Path,
    packet: &AgentPacketDto,
    first_files: &[TaskBriefFileOutput],
    symbols: &[TaskBriefSymbolOutput],
) -> Vec<String> {
    let project = quote_command_path(project_root);
    let prompt = quote_command_value(&packet.question);
    let mut commands = Vec::new();
    commands.push(format!(
        "codestory-cli packet --project {project} --question {prompt} --task-class edit-planning --budget {}",
        packet_budget_mode_label(packet.budget.requested)
    ));
    if let Some(file) = first_files.first() {
        commands.push(format!(
            "codestory-cli snippet --project {project} --query {}",
            quote_command_value(&file.path)
        ));
    }
    if let Some(symbol) = symbols.first() {
        commands.push(format!(
            "codestory-cli trail --project {project} --query {} --story --hide-speculative",
            quote_command_value(&symbol.name)
        ));
    }
    commands.push(format!("codestory-cli affected --project {project} <path>"));
    commands.extend(packet.sufficiency.follow_up_commands.iter().cloned());
    commands
}

fn render_task_brief_markdown(brief: &TaskBriefOutput) -> String {
    let mut markdown = String::new();
    let _ = writeln!(markdown, "# Task Brief");
    let _ = writeln!(
        markdown,
        "status: {}",
        task_brief_inline_code(&brief.status)
    );
    let _ = writeln!(markdown, "task_brief_version: {}", brief.task_brief_version);
    let _ = writeln!(
        markdown,
        "source_packet_id: {}",
        task_brief_inline_code(&brief.source_packet_id)
    );
    let _ = writeln!(
        markdown,
        "source_packet_sufficiency: {}",
        task_brief_inline_code(&brief.source_packet_sufficiency)
    );
    let _ = writeln!(
        markdown,
        "prompt: {}",
        task_brief_inline_code(&brief.prompt)
    );
    append_task_brief_files(&mut markdown, "First Files", &brief.first_files);
    append_task_brief_symbols(&mut markdown, "Relevant Symbols", &brief.relevant_symbols);
    append_task_brief_files(&mut markdown, "Likely Tests", &brief.likely_tests);
    append_task_brief_strings(&mut markdown, "Impacted Surfaces", &brief.impacted_surfaces);
    append_task_brief_strings(&mut markdown, "Risks And Unknowns", &brief.risks_unknowns);
    append_task_brief_commands(
        &mut markdown,
        "Follow Up CodeStory Commands",
        &brief.follow_up_codestory_commands,
    );
    append_task_brief_strings(&mut markdown, "Future Sections", &brief.future_sections);
    markdown
}

fn task_brief_inline_code(value: &str) -> String {
    format!("`{}`", task_brief_markdown_text(value))
}

fn task_brief_markdown_text(value: &str) -> String {
    value.replace('`', "'").replace(['\r', '\n'], " ")
}

fn append_task_brief_files(markdown: &mut String, title: &str, files: &[TaskBriefFileOutput]) {
    let _ = writeln!(markdown, "\n## {title}");
    if files.is_empty() {
        let _ = writeln!(markdown, "- none from source packet");
        return;
    }
    for file in files {
        let line = file.line.map(|line| format!(":{line}")).unwrap_or_default();
        let _ = writeln!(
            markdown,
            "- {}{} - {}",
            task_brief_inline_code(&file.path),
            line,
            task_brief_markdown_text(&file.reason)
        );
    }
}

fn append_task_brief_symbols(
    markdown: &mut String,
    title: &str,
    symbols: &[TaskBriefSymbolOutput],
) {
    let _ = writeln!(markdown, "\n## {title}");
    if symbols.is_empty() {
        let _ = writeln!(markdown, "- none from source packet");
        return;
    }
    for symbol in symbols {
        let location = symbol
            .path
            .as_ref()
            .map(|path| {
                let line = symbol
                    .line
                    .map(|line| format!(":{line}"))
                    .unwrap_or_default();
                format!(" {}{line}", task_brief_inline_code(path))
            })
            .unwrap_or_default();
        let _ = writeln!(
            markdown,
            "- {} ({}){} - {}",
            task_brief_inline_code(&symbol.name),
            task_brief_markdown_text(&symbol.kind),
            location,
            task_brief_markdown_text(&symbol.reason)
        );
    }
}

fn append_task_brief_strings(markdown: &mut String, title: &str, values: &[String]) {
    let _ = writeln!(markdown, "\n## {title}");
    if values.is_empty() {
        let _ = writeln!(markdown, "- none");
        return;
    }
    for value in values {
        let _ = writeln!(markdown, "- {}", task_brief_markdown_text(value));
    }
}

fn append_task_brief_commands(markdown: &mut String, title: &str, values: &[String]) {
    let _ = writeln!(markdown, "\n## {title}");
    for value in values {
        let _ = writeln!(markdown, "- command:");
        let _ = writeln!(markdown, "    {}", value.replace(['\r', '\n'], " "));
    }
}

fn render_packet_markdown(project_root: &std::path::Path, packet: &AgentPacketDto) -> String {
    let mut markdown = String::new();
    let _ = writeln!(markdown, "# Packet");
    append_packet_operator_header(&mut markdown, packet);
    let _ = writeln!(
        markdown,
        "question: `{}`",
        packet.question.replace('\n', " ")
    );
    let _ = writeln!(
        markdown,
        "budget: `{}`",
        packet_budget_mode_label(packet.budget.requested)
    );
    let _ = writeln!(
        markdown,
        "task_class: `{}`",
        packet_task_class_label(packet.plan.task_class)
    );
    let _ = writeln!(
        markdown,
        "sufficiency: `{}`",
        packet_sufficiency_label(packet.sufficiency.status)
    );
    if packet.budget.truncated {
        let _ = writeln!(
            markdown,
            "truncated: `{}` ({})",
            packet.budget.truncated,
            packet.budget.omitted_sections.join(", ")
        );
    }

    if !packet.plan.queries.is_empty() {
        let _ = writeln!(markdown, "\n## Plan");
        for query in &packet.plan.queries {
            let _ = writeln!(markdown, "- `{}` - {}", query.query, query.purpose);
        }
    }

    if !packet.sufficiency.covered_claims.is_empty() {
        let _ = writeln!(markdown, "\n## Covered Claims");
        let _ = writeln!(markdown, "{REPO_CONTENT_BOUNDARY_LINE}");
        for claim in &packet.sufficiency.covered_claims {
            let _ = writeln!(markdown, "- {}", claim.claim);
            for citation in claim.citations.iter().take(3) {
                let _ = writeln!(
                    markdown,
                    "  - {}",
                    render_agent_citation(project_root, citation, true)
                );
            }
        }
    }

    if !packet.sufficiency.gaps.is_empty() {
        let _ = writeln!(markdown, "\n## Gaps");
        for gap in &packet.sufficiency.gaps {
            let _ = writeln!(markdown, "- {gap}");
        }
    }

    if !packet.sufficiency.follow_up_commands.is_empty() {
        let _ = writeln!(markdown, "\n## Follow Up");
        for command in &packet.sufficiency.follow_up_commands {
            let _ = writeln!(markdown, "- `{command}`");
        }
    }

    if !packet.sufficiency.avoid_opening.is_empty() {
        let _ = writeln!(markdown, "\n## Avoid Opening");
        for item in &packet.sufficiency.avoid_opening {
            let _ = writeln!(markdown, "- {item}");
        }
    }

    markdown.push('\n');
    markdown.push_str(&render_context_markdown(project_root, &packet.answer));
    markdown
}

fn append_packet_operator_header(markdown: &mut String, packet: &AgentPacketDto) {
    let _ = writeln!(markdown, "## Status");
    let _ = writeln!(
        markdown,
        "status: {}",
        packet_operator_status(packet.sufficiency.status)
    );
    let _ = writeln!(markdown, "## Trust");
    let _ = writeln!(
        markdown,
        "trust: sufficiency={} budget_truncated={} omitted_sections={}",
        packet_sufficiency_label(packet.sufficiency.status),
        packet.budget.truncated,
        packet_budget_omitted_sections(packet)
    );
    let _ = writeln!(markdown, "## Next Action");
    let _ = writeln!(
        markdown,
        "next_action: {}",
        packet_operator_next_action(packet)
    );
    let _ = writeln!(markdown, "## Proof Tier");
    let _ = writeln!(markdown, "proof_tier: packet_evidence");
}

fn packet_operator_status(status: PacketSufficiencyStatusDto) -> &'static str {
    match status {
        PacketSufficiencyStatusDto::Sufficient => "ready",
        PacketSufficiencyStatusDto::Partial => "needs_attention",
        PacketSufficiencyStatusDto::Insufficient => "blocked",
    }
}

fn packet_budget_omitted_sections(packet: &AgentPacketDto) -> String {
    if packet.budget.omitted_sections.is_empty() {
        "none".to_string()
    } else {
        packet.budget.omitted_sections.join(",")
    }
}

fn packet_operator_next_action(packet: &AgentPacketDto) -> &str {
    packet
        .sufficiency
        .follow_up_commands
        .first()
        .map(String::as_str)
        .unwrap_or("Inspect cited source before relying on claims not covered by packet citations.")
}

fn packet_budget_mode_label(mode: PacketBudgetModeDto) -> &'static str {
    match mode {
        PacketBudgetModeDto::Tiny => "tiny",
        PacketBudgetModeDto::Compact => "compact",
        PacketBudgetModeDto::Standard => "standard",
        PacketBudgetModeDto::Deep => "deep",
    }
}

fn packet_task_class_label(task_class: PacketTaskClassDto) -> &'static str {
    match task_class {
        PacketTaskClassDto::ArchitectureExplanation => "architecture_explanation",
        PacketTaskClassDto::BugLocalization => "bug_localization",
        PacketTaskClassDto::ChangeImpact => "change_impact",
        PacketTaskClassDto::RouteTracing => "route_tracing",
        PacketTaskClassDto::SymbolOwnership => "symbol_ownership",
        PacketTaskClassDto::DataFlow => "data_flow",
        PacketTaskClassDto::EditPlanning => "edit_planning",
    }
}

pub(crate) fn packet_sufficiency_label(status: PacketSufficiencyStatusDto) -> &'static str {
    match status {
        PacketSufficiencyStatusDto::Sufficient => "sufficient",
        PacketSufficiencyStatusDto::Partial => "partial",
        PacketSufficiencyStatusDto::Insufficient => "blocked",
    }
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

fn run_bookmark(cmd: BookmarkCommand) -> Result<()> {
    match cmd.action {
        BookmarkAction::Add(cmd) => run_bookmark_add(cmd),
        BookmarkAction::List(cmd) => run_bookmark_list(cmd),
        BookmarkAction::Remove(cmd) => run_bookmark_remove(cmd),
    }
}

fn run_bookmark_add(cmd: BookmarkAddCommand) -> Result<()> {
    ensure_dot_only_for_trail(cmd.format, "bookmark add")?;
    preflight_output_file(cmd.output_file.as_deref())?;
    let runtime = RuntimeContext::new(&cmd.project)?;
    let opened = runtime.ensure_open(cmd.refresh)?;
    ensure_index_ready(&opened, "bookmark add")?;
    let file_filter = cmd.target.file_filter();
    let target = resolve_target_or_emit_ambiguity(
        &runtime,
        cmd.target.selection()?,
        file_filter.as_deref(),
        cmd.format,
        cmd.output_file.as_deref(),
    )?;
    let category = ensure_bookmark_category(&runtime, &cmd.category)?;
    let bookmark = runtime
        .bookmarks
        .create_bookmark(CreateBookmarkRequest {
            category_id: category.id.clone(),
            node_id: target.selected.node_id.clone(),
            comment: cmd.comment.clone(),
        })
        .map_err(map_api_error)?;
    let output = BookmarkAddOutput {
        category,
        bookmark: bookmark_output(bookmark),
    };
    emit(
        cmd.format,
        &output,
        render_bookmark_add_markdown(&output),
        cmd.output_file.as_deref(),
    )
}

fn run_bookmark_list(cmd: BookmarkListCommand) -> Result<()> {
    ensure_dot_only_for_trail(cmd.format, "bookmark list")?;
    preflight_output_file(cmd.output_file.as_deref())?;
    let runtime = RuntimeContext::new(&cmd.project)?;
    let _summary = runtime.open_project_summary()?;
    let categories = runtime.bookmarks.list_categories().map_err(map_api_error)?;
    let category_id = cmd
        .category
        .as_deref()
        .map(|category| resolve_bookmark_category_id(&categories, category))
        .transpose()?;
    let bookmarks = runtime
        .bookmarks
        .list_bookmarks(category_id)
        .map_err(map_api_error)?
        .into_iter()
        .map(bookmark_output)
        .collect::<Vec<_>>();
    let output = BookmarkListOutput {
        categories,
        bookmarks,
    };
    emit(
        cmd.format,
        &output,
        render_bookmark_list_markdown(&output),
        cmd.output_file.as_deref(),
    )
}

fn run_bookmark_remove(cmd: BookmarkRemoveCommand) -> Result<()> {
    ensure_dot_only_for_trail(cmd.format, "bookmark remove")?;
    preflight_output_file(cmd.output_file.as_deref())?;
    let runtime = RuntimeContext::new(&cmd.project)?;
    let _summary = runtime.open_project_summary()?;
    let bookmark_id = parse_bookmark_db_id(&cmd.id, "bookmark_id")?;
    find_bookmark_by_id(&runtime, &cmd.id)?;
    runtime
        .bookmarks
        .delete_bookmark(bookmark_id)
        .map_err(map_api_error)?;
    let output = BookmarkRemoveOutput {
        removed_id: bookmark_id.to_string(),
    };
    emit(
        cmd.format,
        &output,
        render_bookmark_remove_markdown(&output),
        cmd.output_file.as_deref(),
    )
}

fn bookmark_output(bookmark: BookmarkDto) -> BookmarkOutput {
    let stale = bookmark.node_kind == NodeKind::UNKNOWN;
    BookmarkOutput { bookmark, stale }
}

fn parse_bookmark_db_id(raw: &str, field: &str) -> Result<i64> {
    let trimmed = raw.trim();
    trimmed
        .parse::<i64>()
        .with_context(|| format!("Invalid {field}: `{trimmed}`"))
}

fn resolve_bookmark_category_id(categories: &[BookmarkCategoryDto], raw: &str) -> Result<i64> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        bail!("Bookmark category cannot be empty.");
    }
    if let Ok(id) = trimmed.parse::<i64>()
        && categories
            .iter()
            .any(|category| category.id == id.to_string())
    {
        return Ok(id);
    }
    categories
        .iter()
        .find(|category| category.name.eq_ignore_ascii_case(trimmed))
        .map(|category| parse_bookmark_db_id(&category.id, "category_id"))
        .unwrap_or_else(|| bail!("Bookmark category not found: `{trimmed}`"))
}

fn ensure_bookmark_category(runtime: &RuntimeContext, raw: &str) -> Result<BookmarkCategoryDto> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        bail!("Bookmark category cannot be empty.");
    }
    let categories = runtime.bookmarks.list_categories().map_err(map_api_error)?;
    if let Ok(id) = trimmed.parse::<i64>()
        && let Some(category) = categories
            .iter()
            .find(|category| category.id == id.to_string())
    {
        return Ok(category.clone());
    }
    if let Some(category) = categories
        .iter()
        .find(|category| category.name.eq_ignore_ascii_case(trimmed))
    {
        return Ok(category.clone());
    }
    runtime
        .bookmarks
        .create_category(CreateBookmarkCategoryRequest {
            name: trimmed.to_string(),
        })
        .map_err(map_api_error)
}

fn find_bookmark_by_id(runtime: &RuntimeContext, raw_id: &str) -> Result<BookmarkDto> {
    let bookmark_id = parse_bookmark_db_id(raw_id, "bookmark_id")?;
    runtime
        .bookmarks
        .list_bookmarks(None)
        .map_err(map_api_error)?
        .into_iter()
        .find(|bookmark| bookmark.id == bookmark_id.to_string())
        .with_context(|| format!("Bookmark not found: {bookmark_id}"))
}

fn load_bookmark_focus_by_id(runtime: &RuntimeContext, raw_id: &str) -> Result<BookmarkDto> {
    let bookmark_id = parse_bookmark_db_id(raw_id, "bookmark_id")?;
    let bookmark = find_bookmark_by_id(runtime, raw_id)?;
    if bookmark.node_kind == NodeKind::UNKNOWN {
        bail!(
            "Bookmark {bookmark_id} is stale: node {} is no longer present after reindex.",
            bookmark.node_id.0
        );
    }
    Ok(bookmark)
}

fn render_bookmark_add_markdown(output: &BookmarkAddOutput) -> String {
    let mut markdown = String::new();
    markdown.push_str("# Bookmark Added\n");
    markdown.push_str(&format!("- category: {}\n", output.category.name));
    markdown.push_str(&render_bookmark_row(&output.bookmark));
    markdown
}

fn render_bookmark_list_markdown(output: &BookmarkListOutput) -> String {
    let mut markdown = String::new();
    markdown.push_str("# Bookmarks\n");
    markdown.push_str("categories:\n");
    for category in &output.categories {
        markdown.push_str(&format!("- {}: {}\n", category.id, category.name));
    }
    markdown.push_str("bookmarks:\n");
    if output.bookmarks.is_empty() {
        markdown.push_str("- none\n");
    }
    for bookmark in &output.bookmarks {
        markdown.push_str(&render_bookmark_row(bookmark));
    }
    markdown
}

fn render_bookmark_remove_markdown(output: &BookmarkRemoveOutput) -> String {
    format!("# Bookmark Removed\n- removed_id: {}\n", output.removed_id)
}

fn render_bookmark_row(output: &BookmarkOutput) -> String {
    let bookmark = &output.bookmark;
    let stale = if output.stale { " stale=true" } else { "" };
    let file = bookmark
        .file_path
        .as_deref()
        .map(|path| format!(" path=`{}`", display::clean_path_string(path)))
        .unwrap_or_default();
    let comment = bookmark
        .comment
        .as_deref()
        .map(|comment| format!(" comment=`{}`", comment.replace('`', "'")))
        .unwrap_or_default();
    format!(
        "- id={} node={} label=`{}` kind={:?}{file}{comment}{stale}\n",
        bookmark.id, bookmark.node_id.0, bookmark.node_label, bookmark.node_kind
    )
}

fn run_doctor(cmd: DoctorCommand) -> Result<()> {
    ensure_dot_only_for_trail(cmd.format, "doctor")?;
    preflight_output_file(cmd.output_file.as_deref())?;
    let runtime = RuntimeContext::new_inspect_only(&cmd.project)?;
    let summary = runtime.open_project_summary()?;
    let output = build_doctor_output(&runtime, &summary);
    let markdown = render_doctor_markdown(&output);
    emit(cmd.format, &output, markdown, cmd.output_file.as_deref())
}

fn run_ready(cmd: ReadyCommand) -> Result<()> {
    ensure_dot_only_for_trail(cmd.format, "ready")?;
    preflight_output_file(cmd.output_file.as_deref())?;
    let output = build_ready_output(&cmd)?;
    let markdown = render_ready_markdown(&output);
    emit(cmd.format, &output, markdown, cmd.output_file.as_deref())
}

fn build_ready_output(cmd: &ReadyCommand) -> Result<ReadyOutput> {
    let runtime = RuntimeContext::new_inspect_only(&cmd.project)?;
    let agent_run_id = cmd.run_id.as_deref();
    let (summary, local_refresh) = if cmd.wait_fresh {
        wait_for_local_freshness(&cmd.project, &runtime)?
    } else {
        (runtime.open_project_summary()?, None)
    };
    let readiness_sidecar = if matches!(cmd.goal, None | Some(args::ReadyGoal::Agent)) {
        agent_readiness_status(&runtime, agent_run_id)
    } else {
        doctor_sidecar_status(&runtime)
    };
    let selected_agent_run_id = readiness_sidecar
        .run_id
        .as_deref()
        .or(agent_run_id)
        .map(str::to_string);
    let mut verdicts = build_summary_readiness(
        &summary.root,
        &summary.stats,
        summary.freshness.as_ref(),
        &readiness_sidecar,
    );
    let readiness_lanes = build_readiness_lanes_for_runtime(
        &runtime,
        &verdicts,
        selected_agent_run_id.as_deref(),
        Some(&readiness_sidecar),
    );
    if let Some(goal) = cmd.goal {
        let goal = goal.as_dto();
        verdicts.retain(|verdict| verdict.goal == goal);
    }
    let output = ReadyOutput {
        verdicts,
        local_refresh,
        readiness_lanes,
    };
    Ok(output)
}

fn doctor_sidecar_status_is_live_ready(status: &RetrievalStatusOutput) -> bool {
    status.retrieval_mode == "full" && status.degraded_reason.is_none()
}

pub(crate) fn wait_for_local_freshness(
    project: &ProjectArgs,
    inspect_runtime: &RuntimeContext,
) -> Result<(ProjectSummary, Option<readiness::LocalRefreshOutput>)> {
    let summary = inspect_runtime.open_project_summary()?;
    if !local_freshness_needs_refresh(&summary) {
        let mut output = local_refresh_output_from_summary(&summary);
        if output.state == readiness::LocalRefreshState::Refreshed {
            output.reason = Some("already_fresh".to_string());
        }
        return Ok((summary, Some(output)));
    }

    let lock = match local_refresh_status::try_acquire_local_refresh_lock(
        &inspect_runtime.cache_root,
        &inspect_runtime.project_root,
    )? {
        local_refresh_status::LocalRefreshLockAttempt::Acquired(lock) => lock,
        local_refresh_status::LocalRefreshLockAttempt::Busy(busy) => {
            let mut output = local_refresh_output_from_summary(&summary);
            output.state = readiness::LocalRefreshState::Refreshing;
            output.blocks_local_surfaces = true;
            output.readiness_status = ReadinessStatusDto::RepairIndex;
            output.reason = Some(if busy.status.is_some() {
                "refreshing".to_string()
            } else {
                "refresh_lock_held".to_string()
            });
            if let Some(status) = busy.status {
                output.phase = Some(status.phase);
                output.pid = Some(status.pid);
                output.started_at_epoch_ms = Some(status.started_at_epoch_ms);
                output.updated_at_epoch_ms = Some(status.updated_at_epoch_ms);
                output.last_failure_reason = status.last_failure_reason;
            } else {
                output.pid = busy.pid;
                output.started_at_epoch_ms = busy.started_at_epoch_ms;
                output.phase = Some("starting".to_string());
            }
            output.lock_path = Some(display::clean_path_string(
                &busy.lock_path.to_string_lossy(),
            ));
            attach_complete_publication(&mut output, &summary);
            return Ok((summary, Some(output)));
        }
    };
    let summary = inspect_runtime.open_project_summary()?;
    if !local_freshness_needs_refresh(&summary) {
        let mut output = local_refresh_output_from_summary(&summary);
        output.reason = Some("coalesced_refresh_completed".to_string());
        return Ok((summary, Some(output)));
    }
    let refresh_started_at_epoch_ms = lock.started_at_epoch_ms();
    let refresh_pid = lock.pid();
    let refresh_phase = "incremental_index";
    if !lock.write_status(
        &inspect_runtime.project_root,
        "refreshing",
        refresh_phase,
        None,
    )? {
        anyhow::bail!("local refresh ownership changed before indexing");
    }
    let heartbeat = local_refresh_status::LocalRefreshHeartbeat::start(
        &lock,
        &inspect_runtime.project_root,
        refresh_phase,
    );

    let index_runtime = RuntimeContext::new(project)?;
    let refresh_result = index_runtime.ensure_open(args::RefreshMode::Incremental);
    heartbeat.stop();
    match refresh_result {
        Ok(opened) => {
            let _ = lock.write_status(
                &inspect_runtime.project_root,
                "refreshed",
                refresh_phase,
                None,
            );
            let mut output = local_refresh_output_from_summary(&opened.summary);
            output.phase = Some(refresh_phase.to_string());
            output.pid = Some(refresh_pid);
            output.started_at_epoch_ms = Some(refresh_started_at_epoch_ms);
            output.updated_at_epoch_ms = Some(local_refresh_status::now_epoch_ms());
            if output.state == readiness::LocalRefreshState::Refreshed {
                output.reason = Some("refreshed".to_string());
            } else {
                output.state = readiness::LocalRefreshState::Failed;
                output.blocks_local_surfaces = true;
                output.reason = Some("refresh_did_not_reach_fresh".to_string());
            }
            attach_complete_publication(&mut output, &opened.summary);
            Ok((opened.summary, Some(output)))
        }
        Err(error) => {
            let error_text = error.to_string();
            let _ = lock.write_status(
                &inspect_runtime.project_root,
                "failed",
                refresh_phase,
                Some(error_text.clone()),
            );
            let mut output = local_refresh_output_from_summary(&summary);
            output.state = classify_local_refresh_failure_state(&error);
            output.blocks_local_surfaces = true;
            output.readiness_status = ReadinessStatusDto::RepairIndex;
            output.reason = Some(error_text.clone());
            output.phase = Some(refresh_phase.to_string());
            output.pid = Some(refresh_pid);
            output.started_at_epoch_ms = Some(refresh_started_at_epoch_ms);
            output.updated_at_epoch_ms = Some(local_refresh_status::now_epoch_ms());
            output.last_failure_reason = Some(error_text);
            attach_complete_publication(&mut output, &summary);
            Ok((summary, Some(output)))
        }
    }
}

pub(crate) fn attach_complete_publication(
    output: &mut readiness::LocalRefreshOutput,
    summary: &ProjectSummary,
) {
    output.serving_publication = summary
        .publication
        .as_ref()
        .and_then(|publication| serde_json::to_value(publication).ok());
    if output.serving_publication.is_some()
        && output.state == readiness::LocalRefreshState::Refreshing
    {
        output.blocks_local_surfaces = false;
        output.readiness_status = ReadinessStatusDto::Ready;
    }
}

pub(crate) fn local_freshness_needs_refresh(summary: &ProjectSummary) -> bool {
    summary.freshness.as_ref().is_some_and(|freshness| {
        matches!(
            freshness.status,
            IndexFreshnessStatusDto::Stale | IndexFreshnessStatusDto::NotChecked
        )
    })
}

pub(crate) fn local_refresh_output_from_summary(
    summary: &ProjectSummary,
) -> readiness::LocalRefreshOutput {
    let verdict = readiness::build_readiness_verdict(
        ReadinessGoalDto::LocalNavigation,
        readiness::ReadinessInputs {
            project: &summary.root,
            stats: &summary.stats,
            freshness: summary.freshness.as_ref(),
            sidecar: None,
        },
    );
    readiness::local_refresh_output(&verdict)
}

fn classify_local_refresh_failure_state(error: &anyhow::Error) -> readiness::LocalRefreshState {
    let message = format!("{error:#}").to_ascii_lowercase();
    if message.contains("cache_busy")
        || message.contains("database is locked")
        || message.contains("database table is locked")
        || message.contains("cache is busy")
    {
        readiness::LocalRefreshState::Skipped
    } else {
        readiness::LocalRefreshState::Failed
    }
}

fn run_agent(cmd: args::AgentCommand) -> Result<()> {
    match cmd.action {
        args::AgentAction::Preflight(cmd) => run_agent_preflight(cmd),
    }
}

fn run_agent_preflight(cmd: args::AgentPreflightCommand) -> Result<()> {
    ensure_dot_only_for_trail(cmd.format, "agent preflight")?;
    preflight_output_file(cmd.output_file.as_deref())?;
    let runtime = RuntimeContext::new_inspect_only(&cmd.project)?;
    let summary = runtime.open_project_summary()?;
    let (summary, local_refresh) = if local_freshness_needs_refresh(&summary) {
        wait_for_agent_preflight_local_freshness(&cmd.project, &summary)?
    } else {
        (summary, None)
    };
    let readiness_sidecar = agent_readiness_status(&runtime, None);
    let readiness = build_summary_readiness(
        &summary.root,
        &summary.stats,
        summary.freshness.as_ref(),
        &readiness_sidecar,
    );
    let readiness_lanes =
        build_readiness_lanes_for_runtime(&runtime, &readiness, None, Some(&readiness_sidecar));
    let output = build_agent_preflight_output(&readiness, readiness_lanes, local_refresh);
    let markdown = render_agent_preflight_markdown(&output);
    emit(cmd.format, &output, markdown, cmd.output_file.as_deref())
}

fn wait_for_agent_preflight_local_freshness(
    project: &ProjectArgs,
    summary: &ProjectSummary,
) -> Result<(ProjectSummary, Option<readiness::LocalRefreshOutput>)> {
    let (tx, rx) = mpsc::channel();
    let project = project.clone();
    thread::spawn(move || {
        let result = RuntimeContext::new_inspect_only(&project)
            .and_then(|runtime| wait_for_local_freshness(&project, &runtime));
        let _ = tx.send(result);
    });

    let budget = agent_preflight_local_refresh_foreground_budget();
    if budget.is_zero() {
        return Ok((
            summary.clone(),
            Some(agent_preflight_local_refresh_timeout_output(summary)),
        ));
    }

    match rx.recv_timeout(budget) {
        Ok(result) => result,
        Err(mpsc::RecvTimeoutError::Timeout) => Ok((
            summary.clone(),
            Some(agent_preflight_local_refresh_timeout_output(summary)),
        )),
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            let mut output = local_refresh_output_from_summary(summary);
            output.state = readiness::LocalRefreshState::Failed;
            output.blocks_local_surfaces = true;
            output.readiness_status = ReadinessStatusDto::RepairIndex;
            output.reason = Some("refresh_worker_disconnected".to_string());
            output.updated_at_epoch_ms = Some(local_refresh_status::now_epoch_ms());
            Ok((summary.clone(), Some(output)))
        }
    }
}

fn agent_preflight_local_refresh_timeout_output(
    summary: &ProjectSummary,
) -> readiness::LocalRefreshOutput {
    let mut output = local_refresh_output_from_summary(summary);
    output.state = readiness::LocalRefreshState::Refreshing;
    output.blocks_local_surfaces = true;
    output.readiness_status = ReadinessStatusDto::RepairIndex;
    output.reason = Some("refresh_timeout".to_string());
    output.phase = Some("incremental_index".to_string());
    output.updated_at_epoch_ms = Some(local_refresh_status::now_epoch_ms());
    output
}

fn agent_preflight_local_refresh_foreground_budget() -> Duration {
    std::env::var("CODESTORY_AGENT_PREFLIGHT_LOCAL_REFRESH_TIMEOUT_MS")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map(Duration::from_millis)
        .unwrap_or(AGENT_PREFLIGHT_LOCAL_REFRESH_FOREGROUND_BUDGET)
}

const LOCAL_GRAPH_AGENT_SURFACES: &[&str] = &[
    "ground", "files", "symbol", "callers", "callees", "trail", "trace", "snippet", "affected",
];
const FULL_RETRIEVAL_AGENT_SURFACES: &[&str] = &["packet_full", "search_full", "context_full"];

fn build_agent_preflight_output(
    readiness: &[codestory_contracts::api::ReadinessVerdictDto],
    readiness_lanes: BTreeMap<String, ReadinessLaneOutput>,
    local_refresh: Option<readiness::LocalRefreshOutput>,
) -> args::AgentPreflightOutput {
    let local = readiness
        .iter()
        .find(|verdict| verdict.goal == ReadinessGoalDto::LocalNavigation)
        .expect("local_navigation readiness verdict");
    let agent = readiness
        .iter()
        .find(|verdict| verdict.goal == ReadinessGoalDto::AgentPacketSearch)
        .expect("agent_packet_search readiness verdict");
    let local_ready = local.status == ReadinessStatusDto::Ready;
    let full_ready = agent.status == ReadinessStatusDto::Ready;
    let mut safe_surfaces = Vec::new();
    let mut blocked_surfaces = Vec::new();

    if local_ready {
        safe_surfaces.extend(surface_strings(LOCAL_GRAPH_AGENT_SURFACES));
    } else {
        blocked_surfaces.extend(surface_strings(LOCAL_GRAPH_AGENT_SURFACES));
    }
    if full_ready {
        safe_surfaces.extend(surface_strings(FULL_RETRIEVAL_AGENT_SURFACES));
    } else {
        blocked_surfaces.extend(surface_strings(FULL_RETRIEVAL_AGENT_SURFACES));
    }

    let mode = if full_ready {
        "full_retrieval"
    } else if local_ready {
        "local_graph"
    } else {
        "blocked"
    };
    let next_command = readiness::primary_non_ready(readiness)
        .and_then(|verdict| verdict.full_repair.first().cloned());
    let human_summary = agent_preflight_summary(local_ready, full_ready, local);

    args::AgentPreflightOutput {
        usable: local_ready || full_ready,
        mode: mode.to_string(),
        local_graph: agent_preflight_lane(local),
        local_refresh: local_refresh.unwrap_or_else(|| readiness::local_refresh_output(local)),
        full_retrieval: agent_preflight_lane(agent),
        local_default: readiness_lanes
            .get("local_default")
            .cloned()
            .expect("local_default readiness lane"),
        agent_packet_search: readiness_lanes
            .get("agent_packet_search")
            .cloned()
            .expect("agent_packet_search readiness lane"),
        readiness_lanes,
        safe_surfaces,
        blocked_surfaces,
        next_command,
        human_summary,
    }
}

fn surface_strings(surfaces: &[&str]) -> Vec<String> {
    surfaces
        .iter()
        .map(|surface| (*surface).to_string())
        .collect()
}

fn agent_preflight_lane(
    verdict: &codestory_contracts::api::ReadinessVerdictDto,
) -> args::AgentPreflightLaneOutput {
    let sidecar = verdict.sidecar.as_ref();
    args::AgentPreflightLaneOutput {
        ready: verdict.status == ReadinessStatusDto::Ready,
        status: verdict.status,
        failed_layer: readiness::failed_layer(verdict),
        summary: verdict.summary.clone(),
        embedding_device_policy: sidecar
            .and_then(|sidecar| sidecar.embedding_device_policy.clone()),
        embedding_device_state: sidecar.and_then(|sidecar| sidecar.embedding_device_state.clone()),
        embedding_device_observation_source: sidecar
            .and_then(|sidecar| sidecar.embedding_device_observation_source.clone()),
        embedding_detected_provider: sidecar
            .and_then(|sidecar| sidecar.embedding_detected_provider.clone()),
        embedding_detected_gpu: sidecar.and_then(|sidecar| sidecar.embedding_detected_gpu.clone()),
        embedding_accelerator_requested: sidecar
            .map(|sidecar| sidecar.embedding_accelerator_requested),
        embedding_accelerator_request_provider: sidecar
            .and_then(|sidecar| sidecar.embedding_accelerator_request_provider.clone()),
        embedding_accelerator_request_device: sidecar
            .and_then(|sidecar| sidecar.embedding_accelerator_request_device.clone()),
        embedding_cpu_allowed: sidecar.map(|sidecar| sidecar.embedding_cpu_allowed),
    }
}

fn agent_preflight_summary(
    local_ready: bool,
    full_ready: bool,
    local: &codestory_contracts::api::ReadinessVerdictDto,
) -> String {
    match (local_ready, full_ready) {
        (_, true) => "Local graph and full retrieval are ready.".to_string(),
        (true, false) => "Local graph is ready. Full retrieval needs a rebuild.".to_string(),
        (false, _) => format!(
            "Local graph is not ready: {} Full retrieval is also unavailable for agent packet/search.",
            local.summary
        ),
    }
}

fn render_agent_preflight_markdown(output: &args::AgentPreflightOutput) -> String {
    let mut markdown = String::new();
    let _ = writeln!(markdown, "# Agent Preflight");
    let _ = writeln!(markdown, "usable: `{}`", output.usable);
    let _ = writeln!(markdown, "mode: `{}`", output.mode);
    let _ = writeln!(
        markdown,
        "local_graph: {}",
        readiness::status_label(output.local_graph.status)
    );
    let _ = writeln!(
        markdown,
        "local_refresh: {}",
        readiness::local_refresh_state_label(output.local_refresh.state)
    );
    if let Some(layer) = output.local_graph.failed_layer {
        let _ = writeln!(markdown, "local_graph_failed_layer: `{layer}`");
    }
    let _ = writeln!(
        markdown,
        "full_retrieval: {}",
        readiness::status_label(output.full_retrieval.status)
    );
    if let Some(layer) = output.full_retrieval.failed_layer {
        let _ = writeln!(markdown, "full_retrieval_failed_layer: `{layer}`");
    }
    if let (Some(policy), Some(state), Some(cpu_allowed)) = (
        output.full_retrieval.embedding_device_policy.as_deref(),
        output.full_retrieval.embedding_device_state.as_deref(),
        output.full_retrieval.embedding_cpu_allowed,
    ) {
        let source = output
            .full_retrieval
            .embedding_device_observation_source
            .as_deref()
            .map(|source| format!(" observation_source=`{source}`"))
            .unwrap_or_default();
        let detected = output
            .full_retrieval
            .embedding_detected_provider
            .as_deref()
            .map(|provider| {
                let gpu = output
                    .full_retrieval
                    .embedding_detected_gpu
                    .as_deref()
                    .unwrap_or("unknown");
                format!(" detected_provider=`{provider}` detected_gpu=`{gpu}`")
            })
            .unwrap_or_default();
        let request = output
            .full_retrieval
            .embedding_accelerator_requested
            .filter(|requested| *requested)
            .map(|_| {
                let provider = output
                    .full_retrieval
                    .embedding_accelerator_request_provider
                    .as_deref()
                    .unwrap_or("unknown");
                let device = output
                    .full_retrieval
                    .embedding_accelerator_request_device
                    .as_deref()
                    .unwrap_or("unknown");
                format!(" accelerator_request=`{provider}:{device}`")
            })
            .unwrap_or_default();
        let _ = writeln!(
            markdown,
            "full_retrieval_embedding_device: policy=`{policy}` observed=`{state}`{source}{detected}{request} cpu_allowed={cpu_allowed}"
        );
    }
    let _ = writeln!(markdown, "human_summary: {}", output.human_summary);
    if let Some(command) = output.next_command.as_deref() {
        let _ = writeln!(markdown, "next_command: `{command}`");
    }
    markdown
}

fn run_search(cmd: SearchCommand) -> Result<()> {
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

mod drill;
use drill::*;

mod source_commands;
use source_commands::*;

mod server;
use server::*;

pub(crate) mod resolution;
use resolution::*;

pub(crate) mod diagnostics;
use diagnostics::*;

pub(crate) mod artifacts;
pub(crate) use artifacts::preflight_output_file;
use artifacts::{ensure_dot_only_for_trail, write_context_bundle};

pub(crate) mod rendering;
use rendering::*;

#[cfg(test)]
mod tests;
