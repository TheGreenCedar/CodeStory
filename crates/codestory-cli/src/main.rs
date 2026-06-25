//! Command-line integration entry point for CodeStory.
//!
//! This binary keeps command parsing, runtime setup, and output emission in one
//! place so extension work has a single dispatch boundary. Subcommands should
//! parse into types from `args`, open project state through `RuntimeContext`,
//! and emit through `output` helpers so markdown, JSON, DOT, stdout, and
//! `--output-file` behavior stays consistent.
//!
//! User-facing command usage belongs in CLI help and external docs. Rustdoc in
//! this crate documents the code contracts that command handlers, output DTOs,
//! and local integration transports rely on.

use anyhow::{Context, Result, bail};
use clap::{CommandFactory, Parser};
use clap_complete::{Shell, generate};
use codestory_contracts::api::{
    AffectedAnalysisRequest, AffectedChangeKindDto, AffectedChangeRecordDto, AgentAnswerDto,
    AgentAskRequest, AgentPacketDto, AgentPacketRequestDto, AgentResponseModeDto,
    AgentRetrievalPresetDto, AgentRetrievalProfileSelectionDto, AnswerReadinessReportDto,
    AppEventPayload, BookmarkCategoryDto, BookmarkDto, ClaimReadinessDto,
    CreateBookmarkCategoryRequest, CreateBookmarkRequest, EvidenceItemDto, EvidencePacketDto,
    EvidenceSourceLocationDto, EvidenceTypeDto, FrameworkRouteCoverageDto, GraphArtifactDto,
    GroundingBudgetDto, IndexFreshnessDto, IndexFreshnessStatusDto, IndexMode, IndexedFilesRequest,
    NodeId, NodeKind, NodeOccurrencesRequest, PacketBudgetModeDto, PacketSufficiencyStatusDto,
    PacketTaskClassDto, ProjectSummary, ReadinessGoalDto, ReadinessStatusDto, RepoTextScanStatsDto,
    RetrievalFallbackReasonDto, RetrievalScoreBreakdownDto, RetrievalShadowDto, SearchHit,
    SearchMatchQualityDto, SearchQueryAssessmentDto, SearchRepoTextMode, SearchRequest,
    SourceOccurrenceDto, SourceTruthCheckDto, TrailCallerScope, TrailConfigDto, TrailContextDto,
    TrailDirection, TrailMode,
};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    fmt::Write as _,
    fs,
    io::{IsTerminal, Read},
    net::TcpListener,
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

mod args;
mod config;
mod display;
mod drill_targeting;
mod explore;
mod http_transport;
mod managed_embeddings;
mod output;
mod query_resolution;
mod readiness;
mod report;
mod retrieval;
mod runtime;
mod stdio_catalog;
mod stdio_transport;

use args::{
    AffectedChangeSource, AffectedCommand, AffectedStdinFormat, BookmarkAction, BookmarkAddCommand,
    BookmarkAddOutput, BookmarkCommand, BookmarkListCommand, BookmarkListOutput, BookmarkOutput,
    BookmarkRemoveCommand, BookmarkRemoveOutput, CacheAction, CacheCommand, Cli, CliDirection,
    CliTrailMode, Command, CompletionShell, ContextCommand, DoctorCheckOutput, DoctorCommand,
    DoctorOutput, DoctorSidecarStatusOutput, DrillAnchorConsumerOutput,
    DrillAnchorConsumerSummaryOutput, DrillAnchorOutput, DrillAnchorTextConsumerHintOutput,
    DrillAnchorTimingsOutput, DrillAnswerQualityContractOutput, DrillBridgeEvidenceOutput,
    DrillBridgeGraphPathOutput, DrillBridgeOutput, DrillClaimLedgerEntryOutput,
    DrillClaimLedgerOutput, DrillClaimLedgerScoringOutput, DrillCommand, DrillCommandStatusOutput,
    DrillExecutionBoundaryOutput, DrillMechanicalOutput, DrillOutput, DrillRuntimeTimingsOutput,
    DrillSuiteAnswerQualityOutput, DrillSuiteCommand, DrillSuiteExpectationOutput,
    DrillSuiteLayerFindingOutput, DrillSuiteOutput, DrillSuiteRepoOutput,
    DrillSuiteRetrievalBlockerOutput, DrillSummaryAnchorStatusOutput, DrillSummaryAnchorsOutput,
    DrillSummaryBridgeStatusOutput, DrillSummaryBridgesOutput, DrillSummaryMechanicalOutput,
    DrillSummaryOpenGapsOutput, DrillSummaryOutput, DrillSummarySourceTruthOutput,
    DrillSummarySourceTruthTargetOutput, DrillSummaryStatsOutput, DrillSummaryVerdictOutput,
    DrillVerificationChecklistItemOutput, FilesCommand, GenerateCompletionsCommand, GroundCommand,
    IndexCommand, IndexDryRunOutput, IndexOutput, PacketCommand, ProjectArgs, QueryCommand,
    QueryOutput, QueryResolutionOutput, QuerySelectorOutput, ReadinessLaneOutput, ReadyCommand,
    ReadyOutput, RepoTextMode, SearchCommand, SearchHitOutput, SearchOutput, ServeCommand,
    SetupAction, SetupCommand, SidecarAction, SidecarCommand, SmokeCommand, SmokeProfile,
    SnippetCommand, SnippetJsonOutput, SymbolCommand, SymbolJsonOutput, SymbolWorkflowCommand,
    TaskAction, TaskBriefCommand, TaskCommand, TrailCommand, TrailJsonOutput,
    VerificationTargetOutput, build_trail_request,
};
#[cfg(test)]
use explore::{ExploreTuiAction, ExploreTuiState, explore_tui_action};
#[cfg(test)]
use http_transport::search_repo_text_mode_param;
use output::{
    context_packet_json, emit, emit_text, render_agent_citation, render_context_markdown,
    render_doctor_markdown, render_drill_markdown, render_ground_markdown,
    render_index_dry_run_markdown, render_index_markdown, render_query_markdown,
    render_ready_markdown, render_search_hit_output, render_search_markdown,
    render_snippet_markdown, render_symbol_markdown, render_symbol_mermaid, render_trail_dot,
    render_trail_markdown, render_trail_mermaid, render_trail_story_markdown,
    validate_output_file_parent,
};
use runtime::{
    AmbiguousTargetError, RuntimeContext, ensure_index_ready, map_api_error, refresh_label,
    resolve_refresh_request, resolve_target,
};
use serde::Deserialize;
#[cfg(test)]
use stdio_catalog::{
    prompts_list_json as stdio_prompts_list_json, resources_list_json as stdio_resources_list_json,
    tools_list_json as stdio_tools_list_json,
};

#[derive(Debug, Clone, Copy)]
struct RepoTextOutputConfig {
    mode: RepoTextMode,
    enabled: bool,
}

const CONTEXT_BUNDLE_OUTPUT_BYTE_CAP: usize = 5 * 1024 * 1024;
const CONTEXT_BUNDLE_MARKDOWN_SOFT_CAP: usize = 2 * 1024 * 1024;
const CONTEXT_BUNDLE_TRUNCATION_SUFFIX: &str =
    "\n\n... bundle content truncated by context bundle byte cap\n";
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

fn drill_anchor_jobs(requested: usize, refresh: args::RefreshMode, total_anchors: usize) -> usize {
    if total_anchors <= 1 {
        1
    } else {
        drill_read_only_jobs(requested, refresh).min(total_anchors)
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

fn with_drill_command_duration(
    start: Instant,
    mut status: DrillCommandStatusOutput,
) -> DrillCommandStatusOutput {
    status.duration_ms = elapsed_ms(start);
    status
}

fn main() -> Result<()> {
    let cli = Cli::parse();

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
        Command::Setup(cmd) => run_setup(cmd),
        Command::Cache(cmd) => run_cache(cmd),
        Command::Search(cmd) => run_search(cmd),
        Command::Drill(cmd) => run_drill(cmd),
        Command::DrillSuite(cmd) => run_drill_suite(cmd),
        Command::Symbol(cmd) => run_symbol(cmd),
        Command::Impact(cmd) => run_symbol_workflow(SymbolWorkflowKind::Impact, cmd),
        Command::TestMap(cmd) => run_symbol_workflow(SymbolWorkflowKind::TestMap, cmd),
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
        Command::Serve(cmd) => run_serve(cmd),
        Command::GenerateCompletions(cmd) => run_generate_completions(cmd),
        Command::Retrieval(cmd) => retrieval::run_retrieval(cmd),
        Command::Sidecar(cmd) => run_sidecar(cmd),
    }
}

fn run_sidecar(cmd: SidecarCommand) -> Result<()> {
    match cmd.action {
        SidecarAction::Status(status_cmd) => retrieval::run_retrieval_status(status_cmd),
        SidecarAction::Unknown(args) => {
            let subcommand = args.first().map(String::as_str).unwrap_or("<unknown>");
            bail!(
                "unknown sidecar subcommand `{subcommand}`; use `codestory-cli sidecar status` or `codestory-cli retrieval status`"
            )
        }
    }
}

fn new_agent_surface_runtime(project: &ProjectArgs) -> Result<RuntimeContext> {
    RuntimeContext::new_agent_sidecar(project)
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

fn run_setup(cmd: SetupCommand) -> Result<()> {
    match cmd.action {
        SetupAction::Embeddings(cmd) => run_setup_embeddings(cmd),
    }
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

fn run_setup_embeddings(cmd: args::SetupEmbeddingsCommand) -> Result<()> {
    ensure_dot_only_for_trail(cmd.format, "setup embeddings")?;
    preflight_output_file(cmd.output_file.as_deref())?;
    let project_root = runtime::canonicalize_project_root(&cmd.project.project)?;
    let cache_override =
        runtime::trusted_cache_override(&project_root, cmd.project.cache_dir.as_deref())?;
    let next_commands =
        setup_embeddings_next_commands(&cmd.project.project, cmd.project.cache_dir.as_deref());
    let managed_root = managed_embeddings::managed_root(cache_override.as_deref())?;
    let mut output = managed_embeddings::setup_embeddings(
        &managed_root,
        cmd.quant,
        cmd.variant,
        cmd.dry_run,
        !cmd.no_start,
    )?;
    output.next_commands = next_commands;
    let markdown = managed_embeddings::render_setup_embeddings_markdown(&output);
    emit(cmd.format, &output, markdown, cmd.output_file.as_deref())
}

fn setup_embeddings_next_commands(
    project: &std::path::Path,
    cache_dir: Option<&std::path::Path>,
) -> Vec<String> {
    let mut args = format!(" --project {}", quote_command_path(project));
    if let Some(cache_dir) = cache_dir {
        let _ = write!(args, " --cache-dir {}", quote_command_path(cache_dir));
    }
    vec![
        format!("codestory-cli doctor{args}"),
        format!("codestory-cli index{args} --refresh full"),
    ]
}

fn quote_command_path(path: &std::path::Path) -> String {
    display::quote_command_path(path)
}

fn quote_command_value(value: &str) -> String {
    display::quote_command_value(value)
}

fn quote_command_argument_value(value: &str) -> String {
    display::quote_command_argument_value(value)
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

fn preflight_output_file(output_file: Option<&std::path::Path>) -> Result<()> {
    if let Some(path) = output_file {
        validate_output_file_parent(path)?;
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
        let summary = runtime.open_project_summary()?;
        let refresh_mode =
            resolve_refresh_request(cmd.refresh, &summary).unwrap_or(IndexMode::Incremental);
        let dry_run = runtime
            .index
            .dry_run_index(refresh_mode)
            .map_err(map_api_error)?;
        let output = IndexDryRunOutput { dry_run: &dry_run };
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
    emit(cmd.format, &output, markdown, cmd.output_file.as_deref())?;
    if failed {
        bail!("smoke profile {} failed", output.profile);
    }
    Ok(())
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
        changed_paths: vec![fake_path.to_string()],
        change_records: vec![affected_path_record(
            fake_path,
            AffectedChangeKindDto::Unknown,
            "smoke",
        )],
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
    if sidecar.retrieval_mode == "full" {
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
    let runtime = new_agent_surface_runtime(&cmd.project)?;
    let opened = runtime.ensure_open(cmd.refresh)?;
    ensure_index_ready(&opened, "context")?;

    let resolved = resolve_context_target(&runtime, &cmd, cmd.format, cmd.output_file.as_deref())?;
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
    if let Some(bundle_dir) = cmd.bundle.as_deref() {
        write_context_bundle(bundle_dir, &output, &answer.graphs, &markdown)?;
    }
    emit(cmd.format, &output, markdown, cmd.output_file.as_deref())
}

fn run_packet(cmd: PacketCommand) -> Result<()> {
    ensure_dot_only_for_trail(cmd.format, "packet")?;
    preflight_output_file(cmd.output_file.as_deref())?;
    let runtime = new_agent_surface_runtime(&cmd.project)?;
    let opened = runtime.ensure_open(cmd.refresh)?;
    ensure_index_ready(&opened, "packet")?;

    let packet = runtime
        .browser
        .packet(AgentPacketRequestDto {
            question: cmd.question,
            budget: cmd.budget.into(),
            task_class: cmd.task_class.map(Into::into),
            extra_probes: cmd.extra_probes,
            include_evidence: !cmd.no_evidence,
            latency_budget_ms: cmd.latency_budget_ms,
        })
        .map_err(map_api_error)?;
    if let Some(path) = &cmd.step_trace_out {
        let trace = codestory_runtime::packet_step_trace_json(&packet.answer);
        std::fs::write(path, serde_json::to_string_pretty(&trace)?)?;
    }
    let markdown = render_packet_markdown(&runtime.project_root, &packet);
    emit(cmd.format, &packet, markdown, cmd.output_file.as_deref())
}

fn run_task(cmd: TaskCommand) -> Result<()> {
    match cmd.action {
        TaskAction::Brief(cmd) => run_task_brief(cmd),
    }
}

fn run_task_brief(cmd: TaskBriefCommand) -> Result<()> {
    ensure_dot_only_for_trail(cmd.format, "task brief")?;
    preflight_output_file(cmd.output_file.as_deref())?;
    let runtime = new_agent_surface_runtime(&cmd.project)?;
    let opened = runtime.ensure_open(cmd.refresh)?;
    ensure_index_ready(&opened, "task brief")?;

    let packet = runtime
        .browser
        .packet(AgentPacketRequestDto {
            question: cmd.prompt,
            budget: cmd.budget.into(),
            task_class: Some(PacketTaskClassDto::EditPlanning),
            extra_probes: cmd.extra_probes,
            include_evidence: !cmd.no_evidence,
            latency_budget_ms: cmd.latency_budget_ms,
        })
        .map_err(map_api_error)?;
    let brief = build_task_brief_output(&runtime.project_root, &packet);
    let markdown = render_task_brief_markdown(&brief);
    emit(cmd.format, &brief, markdown, cmd.output_file.as_deref())
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

fn packet_sufficiency_label(status: PacketSufficiencyStatusDto) -> &'static str {
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
    let runtime = if cmd.repair {
        if matches!(cmd.goal, None | Some(args::ReadyGoal::Agent)) {
            new_agent_surface_runtime(&cmd.project)?
        } else {
            RuntimeContext::new(&cmd.project)?
        }
    } else {
        RuntimeContext::new_inspect_only(&cmd.project)?
    };
    let repaired_sidecar = if cmd.repair {
        repair_ready_state(&runtime, cmd.goal)?
    } else {
        None
    };
    let (summary, local_refresh) = if cmd.wait_fresh && !cmd.repair {
        wait_for_local_freshness(&cmd.project, &runtime)?
    } else {
        (runtime.open_project_summary()?, None)
    };
    let sidecar = ready_sidecar_status(&runtime, repaired_sidecar);
    let mut verdicts = build_summary_readiness(
        &summary.root,
        &summary.stats,
        summary.freshness.as_ref(),
        &sidecar,
    );
    if let Some(goal) = cmd.goal {
        let goal = goal.as_dto();
        verdicts.retain(|verdict| verdict.goal == goal);
    }
    let output = ReadyOutput {
        verdicts,
        local_refresh,
    };
    let markdown = render_ready_markdown(&output);
    emit(cmd.format, &output, markdown, cmd.output_file.as_deref())
}

fn wait_for_local_freshness(
    project: &ProjectArgs,
    inspect_runtime: &RuntimeContext,
) -> Result<(ProjectSummary, Option<readiness::LocalRefreshOutput>)> {
    let summary = inspect_runtime.open_project_summary()?;
    if !local_freshness_needs_refresh(&summary) {
        let mut output = local_refresh_output_from_summary(&summary);
        if output.state == readiness::LocalRefreshState::Fresh {
            output.reason = Some("already_fresh".to_string());
        }
        return Ok((summary, Some(output)));
    }

    let index_runtime = RuntimeContext::new(project)?;
    match index_runtime.ensure_open(args::RefreshMode::Incremental) {
        Ok(opened) => {
            let mut output = local_refresh_output_from_summary(&opened.summary);
            if output.state == readiness::LocalRefreshState::Fresh {
                output.reason = Some("refreshed".to_string());
            } else {
                output.state = readiness::LocalRefreshState::Failed;
                output.blocks_local_surfaces = true;
                output.reason = Some("refresh_did_not_reach_fresh".to_string());
            }
            Ok((opened.summary, Some(output)))
        }
        Err(error) => {
            let mut output = local_refresh_output_from_summary(&summary);
            output.state = classify_local_refresh_failure_state(&error);
            output.blocks_local_surfaces = true;
            output.readiness_status = ReadinessStatusDto::RepairIndex;
            output.reason = Some(error.to_string());
            Ok((summary, Some(output)))
        }
    }
}

fn local_freshness_needs_refresh(summary: &ProjectSummary) -> bool {
    summary.freshness.as_ref().is_some_and(|freshness| {
        matches!(
            freshness.status,
            IndexFreshnessStatusDto::Stale | IndexFreshnessStatusDto::NotChecked
        )
    })
}

fn local_refresh_output_from_summary(summary: &ProjectSummary) -> readiness::LocalRefreshOutput {
    let verdict = readiness::build_readiness_verdict(
        ReadinessGoalDto::LocalNavigation,
        readiness::ReadinessInputs {
            project: &summary.root,
            stats: &summary.stats,
            freshness: summary.freshness.as_ref(),
            setup: None,
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
        readiness::LocalRefreshState::SkippedLocked
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
    let sidecar = doctor_sidecar_status(&runtime);
    let readiness = build_summary_readiness(
        &summary.root,
        &summary.stats,
        summary.freshness.as_ref(),
        &sidecar,
    );
    let readiness_lanes = build_readiness_lanes_for_runtime(&runtime, &readiness);
    let output =
        build_agent_preflight_output(&readiness, Path::new(&summary.root), readiness_lanes);
    let markdown = render_agent_preflight_markdown(&output);
    emit(cmd.format, &output, markdown, cmd.output_file.as_deref())
}

fn repair_ready_state(
    runtime: &RuntimeContext,
    goal: Option<args::ReadyGoal>,
) -> Result<Option<codestory_retrieval::SidecarRuntimeConfig>> {
    let opened = runtime.ensure_open(args::RefreshMode::Auto)?;
    ensure_index_ready(&opened, "ready repair")?;
    if !matches!(goal, None | Some(args::ReadyGoal::Agent)) {
        return Ok(None);
    }

    let storage_scope = codestory_retrieval::BootstrapStorageScope::from_parts(
        Some(runtime.project_root.as_path()),
        Some(runtime.storage_path.as_path()),
        Some(runtime.cache_root.as_path()),
    );
    let sidecar = codestory_retrieval::sidecar_runtime_for_project(
        &runtime.project_root,
        codestory_retrieval::SidecarProfile::Agent,
    );
    let bootstrap = codestory_retrieval::bootstrap_sidecars_with_runtime(
        &sidecar,
        Some(runtime.project_root.as_path()),
        &storage_scope,
        None,
        false,
        Duration::from_secs(90),
    )
    .context("ready repair retrieval bootstrap")?;
    ensure_ready_repair_embed_liveness(&bootstrap.infrastructure)?;
    codestory_retrieval::repair_project_qdrant_collection(
        &runtime.project_root,
        &runtime.storage_path,
    )
    .context("ready repair project qdrant repair")?;
    runtime
        .index
        .run_indexing_blocking(IndexMode::Full)
        .map_err(map_api_error)
        .context("ready repair retrieval index refresh")?;
    retrieval::finalize_retrieval_index_for_sidecar_runtime(runtime, &sidecar)
        .context("ready repair retrieval index finalize")?;
    codestory_retrieval::strict_sidecar_status_for_runtime(
        &runtime.project_root,
        Some(&runtime.storage_path),
        sidecar.clone(),
    )
    .context("ready repair final retrieval status")?;
    Ok(Some(sidecar))
}

fn ensure_ready_repair_embed_liveness(
    infrastructure: &codestory_retrieval::InfrastructureHealth,
) -> Result<()> {
    if infrastructure.embed_reachable {
        return Ok(());
    }
    bail!(
        "ready repair embedding sidecar liveness failed before mandatory Qdrant semantic smoke: {}; zoekt_reachable={} ({}); qdrant_reachable={} ({})",
        infrastructure.embed_detail,
        infrastructure.zoekt_reachable,
        infrastructure.zoekt_detail,
        infrastructure.qdrant_reachable,
        infrastructure.qdrant_detail
    )
}

const LOCAL_GRAPH_AGENT_SURFACES: &[&str] = &[
    "ground", "files", "symbol", "callers", "callees", "trail", "trace", "snippet", "affected",
];
const FULL_RETRIEVAL_AGENT_SURFACES: &[&str] = &["packet_full", "search_full", "context_full"];
const AGENT_RUN_MISSING_ID: &str = "agent-run-missing";

fn build_agent_preflight_output(
    readiness: &[codestory_contracts::api::ReadinessVerdictDto],
    project_root: &Path,
    readiness_lanes: BTreeMap<String, ReadinessLaneOutput>,
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
    let repair_command = readiness::primary_non_ready(readiness)
        .and_then(|verdict| verdict.full_repair.first().cloned());
    let human_summary = agent_preflight_summary(local_ready, full_ready, local);

    args::AgentPreflightOutput {
        usable: local_ready || full_ready,
        mode: mode.to_string(),
        local_graph: agent_preflight_lane(local),
        local_refresh: readiness::local_refresh_output(local),
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
        sidecar_setup: stdio_transport::stdio_sidecar_setup_status(project_root),
        safe_surfaces,
        blocked_surfaces,
        repair_command,
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
    args::AgentPreflightLaneOutput {
        ready: verdict.status == ReadinessStatusDto::Ready,
        status: verdict.status,
        failed_layer: readiness::failed_layer(verdict),
        summary: verdict.summary.clone(),
    }
}

fn agent_preflight_summary(
    local_ready: bool,
    full_ready: bool,
    local: &codestory_contracts::api::ReadinessVerdictDto,
) -> String {
    match (local_ready, full_ready) {
        (_, true) => "Local graph and full retrieval are ready.".to_string(),
        (true, false) => "Local graph is ready. Full retrieval needs sidecar repair.".to_string(),
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
    if let Some(state) = output
        .sidecar_setup
        .get("state")
        .and_then(|value| value.as_str())
    {
        let _ = writeln!(markdown, "sidecar_setup: `{state}`");
    }
    let _ = writeln!(markdown, "human_summary: {}", output.human_summary);
    if let Some(command) = output.repair_command.as_deref() {
        let _ = writeln!(markdown, "repair_command: `{command}`");
    }
    markdown
}

fn run_search(cmd: SearchCommand) -> Result<()> {
    ensure_dot_only_for_trail(cmd.format, "search")?;
    preflight_output_file(cmd.output_file.as_deref())?;
    let runtime = new_agent_surface_runtime(&cmd.project)?;
    let opened = runtime.ensure_open(cmd.refresh)?;
    ensure_index_ready(&opened, "search")?;
    let search_results = runtime
        .browser
        .search_results(search_request_from_command(&cmd))
        .map_err(map_api_error)?;
    let output = search_output_from_results(&runtime, &search_results, cmd.why);
    let markdown = render_search_markdown(&runtime.project_root, &output);
    emit(cmd.format, &output, markdown, cmd.output_file.as_deref())
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

fn run_drill(cmd: DrillCommand) -> Result<()> {
    ensure_dot_only_for_trail(cmd.format, "drill")?;
    let output = execute_drill(&cmd)?;
    let contents = write_drill_outputs(cmd.format, &cmd.output_dir, &output)?;
    print!("{}", contents.selected);
    Ok(())
}

fn execute_drill(cmd: &DrillCommand) -> Result<DrillOutput> {
    let total_timer = Instant::now();
    let setup_timer = Instant::now();
    validate_drill_output_dir(&cmd.output_dir)?;
    let runtime = RuntimeContext::new(&cmd.project)?;
    let before = runtime.open_project_summary()?;
    let opened = runtime.ensure_open_from_summary(cmd.refresh, before.clone())?;
    ensure_index_ready(&opened, "drill")?;
    if cmd.refresh != args::RefreshMode::None {
        retrieval::finalize_retrieval_index_for_runtime(&runtime)
            .context("drill retrieval index finalize")?;
    }
    let sidecar_retrieval_mode = codestory_retrieval::strict_sidecar_status(
        &runtime.project_root,
        Some(&runtime.storage_path),
    )
    .ok()
    .map(|status| status.retrieval_mode);
    let refresh = refresh_label(cmd.refresh, opened.refresh_mode);
    let setup_ms = elapsed_ms(setup_timer);

    let mut all_verification_targets = Vec::new();
    let stale_freshness = opened
        .summary
        .freshness
        .as_ref()
        .is_some_and(|freshness| freshness.status == IndexFreshnessStatusDto::Stale);
    let drill_anchors = drill_targeting::validated_drill_anchors(&cmd.anchors, "drill")?;
    let question_search_timer = Instant::now();
    let question_search_result = cmd
        .question
        .as_deref()
        .map(|question| {
            run_drill_question_search(
                &runtime,
                &cmd.output_dir,
                cmd.format,
                question,
                &drill_anchors,
            )
        })
        .transpose()?;
    let (question_search, question_search_output) = match question_search_result {
        Some((status, output)) => (Some(status), Some(output)),
        None => (None, None),
    };
    let question_search_ms = elapsed_ms(question_search_timer);
    let drill_jobs = drill_read_only_jobs(cmd.jobs, cmd.refresh);
    let anchor_resolution_timer = Instant::now();
    let anchor_outputs = run_drill_anchors_in_order(
        &runtime,
        &opened,
        &cmd.output_dir,
        cmd.format,
        &drill_anchors,
        drill_anchor_jobs(cmd.jobs, cmd.refresh, drill_anchors.len()),
    )?;
    for anchor_output in &anchor_outputs {
        all_verification_targets.extend(anchor_output.verification_targets.iter().cloned());
    }
    let anchor_resolution_ms = elapsed_ms(anchor_resolution_timer);
    if let Some(search_output) = question_search_output.as_ref() {
        all_verification_targets.extend(drill_question_search_verification_targets(
            search_output,
            "question search source-truth target",
            16,
        ));
    }
    let mut question_supplemental_searches = Vec::new();
    let supplemental_search_timer = Instant::now();
    if let Some(question) = cmd.question.as_deref() {
        for (status, search_output) in run_drill_question_supplemental_searches(
            &runtime,
            &cmd.output_dir,
            cmd.format,
            question,
            &anchor_outputs,
        )? {
            all_verification_targets.extend(drill_question_search_verification_targets(
                &search_output,
                "supplemental question search source-truth target",
                6,
            ));
            question_supplemental_searches.push(status);
        }
    }
    let supplemental_search_ms = elapsed_ms(supplemental_search_timer);
    dedupe_verification_targets(&mut all_verification_targets);
    let bridge_evidence_timer = Instant::now();
    let bridge_outputs = run_drill_bridges(
        &runtime,
        &cmd.output_dir,
        cmd.format,
        &anchor_outputs,
        stale_freshness,
        drill_jobs,
    );
    let bridge_evidence_ms = elapsed_ms(bridge_evidence_timer);
    let evidence_assembly_timer = Instant::now();
    let claim_ledger_template = drill_claim_ledger_template(&anchor_outputs, &bridge_outputs);
    let next_commands = drill_next_commands(
        &runtime.project_root,
        &anchor_outputs,
        &bridge_outputs,
        stale_freshness,
    );
    let evidence_packet = drill_evidence_packet(
        cmd.question.as_deref(),
        question_search.as_ref(),
        &question_supplemental_searches,
        &anchor_outputs,
        &bridge_outputs,
        &all_verification_targets,
        &next_commands,
    );
    let evidence_assembly_ms = elapsed_ms(evidence_assembly_timer);
    let drill_timings = DrillRuntimeTimingsOutput {
        total_ms: elapsed_ms(total_timer),
        setup_ms,
        question_search_ms,
        anchor_resolution_ms,
        supplemental_search_ms,
        bridge_evidence_ms,
        evidence_assembly_ms,
    };

    Ok(DrillOutput {
        project: display::clean_path_string(&opened.summary.root),
        label: cmd.label.clone(),
        question: cmd.question.clone(),
        output_dir: display::clean_path_string(&cmd.output_dir.to_string_lossy()),
        mechanical: DrillMechanicalOutput {
            before_files: before.stats.file_count,
            before_nodes: before.stats.node_count,
            before_edges: before.stats.edge_count,
            before_errors: before.stats.error_count,
            after_files: opened.summary.stats.file_count,
            after_nodes: opened.summary.stats.node_count,
            after_edges: opened.summary.stats.edge_count,
            after_errors: opened.summary.stats.error_count,
            refresh,
            retrieval: opened.summary.retrieval.clone(),
            sidecar_retrieval_mode,
            freshness: opened.summary.freshness.clone(),
            phase_timings: opened.phase_timings.clone(),
            drill_timings,
        },
        question_search,
        question_supplemental_searches,
        anchors: anchor_outputs,
        bridges: bridge_outputs,
        execution_boundaries: drill_execution_boundaries(),
        verification_targets: all_verification_targets,
        evidence_packet,
        answer_quality_contract: drill_answer_quality_contract(),
        claim_ledger_template,
        verification_checklist: drill_verification_checklist(),
        next_commands,
    })
}

fn write_drill_outputs(
    format: args::OutputFormat,
    output_dir: &std::path::Path,
    output: &DrillOutput,
) -> Result<DrillReportContents> {
    let report_ext = match format {
        args::OutputFormat::Markdown => "md",
        args::OutputFormat::Json => "json",
        args::OutputFormat::Dot => unreachable!("dot was rejected above"),
    };
    let markdown = render_drill_markdown(output);
    let contents = render_drill_contents(format, output, &markdown)?;
    let report_path = output_dir.join(format!("drill-report.{report_ext}"));
    write_drill_report_file(&report_path, &contents.selected)?;
    let markdown_path = output_dir.join("drill-report.md");
    if report_path != markdown_path {
        write_drill_report_file(&markdown_path, &contents.markdown)?;
    }
    let json_path = output_dir.join("drill-report.json");
    if report_path != json_path {
        write_drill_report_file(&json_path, &contents.json)?;
    }
    let summary = drill_summary(output);
    let summary_json = ensure_trailing_newline(
        serde_json::to_string_pretty(&summary).context("Failed to serialize drill summary JSON")?,
    );
    write_drill_report_file(&output_dir.join("drill-summary.json"), &summary_json)?;
    Ok(contents)
}

fn run_drill_suite(cmd: DrillSuiteCommand) -> Result<()> {
    ensure_dot_only_for_trail(cmd.format, "drill-suite")?;
    validate_drill_output_dir(&cmd.output_dir)?;
    let suite_output = execute_codestory_real_repo_drill_suite(&cmd)?;
    emit_drill_suite_progress(format!(
        "writing suite reports output_dir={}",
        display::clean_path_string(&cmd.output_dir.to_string_lossy())
    ));
    write_drill_suite_outputs(cmd.format, &cmd.output_dir, &suite_output)?;
    emit_drill_suite_progress(format!(
        "done repos={} ready={} degraded={} blocked={} answer_ready={} answer_degraded={} answer_failed={} answer_pending={} output_dir={}",
        suite_output.repo_count,
        suite_output.ready_count,
        suite_output.degraded_count,
        suite_output.blocked_count,
        suite_output.answer_ready_count,
        suite_output.answer_degraded_count,
        suite_output.answer_failed_count,
        suite_output.answer_pending_count,
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
struct DrillSuiteCaseManifest {
    #[serde(default)]
    suite: Option<String>,
    cases: Vec<DrillSuiteCaseConfig>,
}

#[derive(Debug, Deserialize)]
struct DrillSuiteCaseConfig {
    slug: String,
    project: std::path::PathBuf,
    question: String,
    anchors: Vec<String>,
    #[serde(default)]
    expect: DrillSuiteCaseExpectConfig,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct DrillSuiteCaseExpectConfig {
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
struct DrillSuiteCase {
    slug: String,
    project_root: std::path::PathBuf,
    question: String,
    anchors: Vec<String>,
    expectations: DrillSuiteExpectationOutput,
}

#[derive(Debug, Deserialize)]
struct DrillSuiteSourceTruthLedger {
    #[allow(dead_code)]
    schema_version: Option<u32>,
    #[allow(dead_code)]
    suite: Option<String>,
    #[serde(default)]
    cases: Vec<DrillSuiteLedgerCase>,
}

#[derive(Clone, Debug, Deserialize)]
struct DrillSuiteLedgerCase {
    slug: String,
    #[serde(default)]
    draft_written: Option<bool>,
    #[serde(default)]
    claims: Vec<DrillSuiteLedgerClaim>,
    #[serde(default)]
    layer_findings: Vec<DrillSuiteLedgerLayerFinding>,
}

#[derive(Clone, Debug, Deserialize)]
struct DrillSuiteLedgerClaim {
    id: String,
    text: String,
    classification: DrillSuiteClaimClassification,
    #[serde(default)]
    changed_after_source_read: Option<bool>,
    #[serde(default)]
    source_files: Vec<String>,
    #[allow(dead_code)]
    notes: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum DrillSuiteClaimClassification {
    Correct,
    Partial,
    Misleading,
    Unsupported,
}

#[derive(Clone, Debug, Deserialize)]
struct DrillSuiteLedgerLayerFinding {
    layer: String,
    status: String,
    detail: String,
}

fn emit_drill_suite_progress(message: impl AsRef<str>) {
    eprintln!("[drill-suite] {}", message.as_ref());
}

fn drill_suite_repo_progress_start_message(
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

fn drill_suite_repo_progress_done_message(
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

fn execute_codestory_real_repo_drill_suite(cmd: &DrillSuiteCommand) -> Result<DrillSuiteOutput> {
    let owner_root = cmd
        .project
        .project
        .canonicalize()
        .with_context(|| format!("Failed to resolve {}", cmd.project.project.display()))?;
    let (suite_name, cases) = drill_suite_cases_from_manifest(&cmd.case_file, &owner_root)?;
    let ledger_supplied = cmd.ledger.is_some();
    let ledger_cases = drill_suite_ledger_cases(cmd.ledger.as_deref())?;
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
    let repos = run_drill_suite_cases(
        cmd,
        cases,
        &ledger_cases,
        ledger_supplied,
        suite_jobs,
        drill_jobs,
    );

    let degraded_count = drill_suite_verdict_count(&repos, "degraded");
    let blocked_count = drill_suite_verdict_count(&repos, "blocked");
    let ready_count = drill_suite_verdict_count(&repos, "ready");
    let answer_ready_count = drill_suite_answer_status_count(&repos, "ready");
    let answer_degraded_count = drill_suite_answer_status_count(&repos, "degraded");
    let answer_failed_count = drill_suite_answer_status_count(&repos, "failed");
    let answer_pending_count = drill_suite_answer_pending_count(&repos);
    let next_actions = repos
        .iter()
        .map(|repo| format!("{}: {}", repo.slug, drill_suite_next_action(repo)))
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
        answer_ready_count,
        answer_degraded_count,
        answer_failed_count,
        answer_pending_count,
        repos,
        retrieval_blockers,
        next_actions,
    })
}

fn drill_suite_case_jobs(
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

fn run_drill_suite_cases(
    cmd: &DrillSuiteCommand,
    cases: Vec<DrillSuiteCase>,
    ledger_cases: &BTreeMap<String, DrillSuiteLedgerCase>,
    ledger_supplied: bool,
    jobs: usize,
    drill_jobs: usize,
) -> Vec<DrillSuiteRepoOutput> {
    let total_cases = cases.len();
    if jobs <= 1 || total_cases <= 1 {
        return cases
            .iter()
            .enumerate()
            .map(|(case_index, case)| {
                run_drill_suite_case(
                    cmd,
                    case_index,
                    total_cases,
                    case,
                    ledger_cases.get(&case.slug),
                    ledger_supplied,
                    drill_jobs,
                )
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
                        let repo = run_drill_suite_case(
                            cmd,
                            *case_index,
                            total_cases,
                            case,
                            ledger_cases.get(&case.slug),
                            ledger_supplied,
                            1,
                        );
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

fn run_drill_suite_case(
    cmd: &DrillSuiteCommand,
    case_index: usize,
    total_cases: usize,
    case: &DrillSuiteCase,
    ledger_case: Option<&DrillSuiteLedgerCase>,
    ledger_supplied: bool,
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
        format: cmd.format,
        jobs: drill_jobs,
    };
    match execute_drill(&drill_cmd).and_then(|drill_output| {
        write_drill_outputs(cmd.format, &repo_output_dir, &drill_output)?;
        Ok(drill_summary(&drill_output))
    }) {
        Ok(summary) => {
            let answer_quality = drill_suite_answer_quality(
                &summary,
                &case.expectations,
                ledger_case,
                ledger_supplied,
            );
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
                answer_quality,
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
                ledger_case,
                ledger_supplied,
            )
        }
    }
}

fn drill_suite_verdict_count(repos: &[DrillSuiteRepoOutput], status: &str) -> usize {
    repos
        .iter()
        .filter(|repo| repo.summary.verdict.status == status)
        .count()
}

fn drill_suite_answer_status_count(repos: &[DrillSuiteRepoOutput], status: &str) -> usize {
    repos
        .iter()
        .filter(|repo| repo.answer_quality.final_answer_status == status)
        .count()
}

fn drill_suite_answer_pending_count(repos: &[DrillSuiteRepoOutput]) -> usize {
    repos
        .iter()
        .filter(|repo| {
            drill_suite_answer_status_is_pending(&repo.answer_quality.final_answer_status)
        })
        .count()
}

fn drill_suite_answer_status_is_pending(status: &str) -> bool {
    matches!(status, "pending_source_verification" | "blocked")
}

fn drill_suite_case_cache_dir(
    suite_cache_dir: Option<&std::path::Path>,
    slug: &str,
) -> Option<std::path::PathBuf> {
    suite_cache_dir.map(|cache_dir| cache_dir.join(output_slug(slug)))
}

fn drill_suite_cases_from_manifest(
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

fn drill_suite_expectations_from_config(
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

#[cfg(test)]
fn empty_drill_suite_expectations() -> DrillSuiteExpectationOutput {
    DrillSuiteExpectationOutput {
        source_truth_files: Vec::new(),
        false_claims: Vec::new(),
        min_anchor_resolution: None,
        allow_partial_bridges: None,
    }
}

fn drill_suite_ledger_cases(
    ledger_path: Option<&std::path::Path>,
) -> Result<BTreeMap<String, DrillSuiteLedgerCase>> {
    let Some(ledger_path) = ledger_path else {
        return Ok(BTreeMap::new());
    };
    let ledger_path = absolute_existing_path(ledger_path).with_context(|| {
        format!(
            "Failed to resolve drill-suite ledger file {}",
            display::clean_path_string(&ledger_path.to_string_lossy())
        )
    })?;
    let ledger_text = fs::read_to_string(&ledger_path).with_context(|| {
        format!(
            "Failed to read drill-suite ledger file {}",
            display::clean_path_string(&ledger_path.to_string_lossy())
        )
    })?;
    let ledger: DrillSuiteSourceTruthLedger =
        serde_json::from_str(&ledger_text).with_context(|| {
            format!(
                "Failed to parse drill-suite ledger file {} as JSON",
                display::clean_path_string(&ledger_path.to_string_lossy())
            )
        })?;
    let mut cases = BTreeMap::new();
    for case in ledger.cases {
        let slug = output_slug(&case.slug);
        if slug.is_empty() {
            bail!("drill-suite ledger case slug cannot be empty");
        }
        if cases.insert(slug.clone(), case).is_some() {
            bail!("drill-suite ledger case slug `{slug}` is duplicated");
        }
    }
    Ok(cases)
}

fn absolute_existing_path(path: &std::path::Path) -> Result<std::path::PathBuf> {
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

fn blocked_drill_suite_repo_output(
    case: &DrillSuiteCase,
    repo_output_dir: &std::path::Path,
    refresh: args::RefreshMode,
    format: args::OutputFormat,
    error: &str,
    ledger_case: Option<&DrillSuiteLedgerCase>,
    ledger_supplied: bool,
) -> DrillSuiteRepoOutput {
    let project = display::clean_path_string(&case.project_root.to_string_lossy());
    let output_dir = display::clean_path_string(&repo_output_dir.to_string_lossy());
    let anchor_statuses = case
        .anchors
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
        .collect::<Vec<_>>();
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
        summary: DrillSummaryOutput {
            summary_version: 1,
            project,
            label: Some(case.slug.clone()),
            question: Some(case.question.clone()),
            output_dir: output_dir.clone(),
            full_report_json: String::new(),
            full_report_markdown: String::new(),
            mechanical: DrillSummaryMechanicalOutput {
                refresh: refresh_label(refresh, None),
                before: drill_summary_stats(0, 0, 0, 0),
                after: drill_summary_stats(0, 0, 0, 1),
                index_ready: false,
                error_delta: 1,
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
                statuses: anchor_statuses,
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
        },
        expectations: case.expectations.clone(),
        answer_quality: drill_suite_blocked_answer_quality(
            &case.expectations,
            ledger_case,
            ledger_supplied,
        ),
    }
}

fn write_drill_suite_outputs(
    format: args::OutputFormat,
    output_dir: &std::path::Path,
    output: &DrillSuiteOutput,
) -> Result<()> {
    let markdown = render_drill_suite_markdown(output);
    let json = ensure_trailing_newline(
        serde_json::to_string_pretty(output).context("Failed to serialize drill suite JSON")?,
    );
    write_drill_report_file(&output_dir.join("suite-report.md"), &markdown)?;
    write_drill_report_file(&output_dir.join("suite-report.json"), &json)?;
    let selected = match format {
        args::OutputFormat::Markdown => ensure_trailing_newline(markdown),
        args::OutputFormat::Json => json,
        args::OutputFormat::Dot => unreachable!("dot was rejected above"),
    };
    let report_ext = drill_artifact_extension(format);
    write_drill_report_file(
        &output_dir.join(format!("drill-suite-report.{report_ext}")),
        &selected,
    )
}

fn drill_artifact_extension(format: args::OutputFormat) -> &'static str {
    match format {
        args::OutputFormat::Markdown => "md",
        args::OutputFormat::Json => "json",
        args::OutputFormat::Dot => unreachable!("dot was rejected above"),
    }
}

fn render_drill_suite_markdown(output: &DrillSuiteOutput) -> String {
    let mut markdown = String::new();
    render_drill_suite_header(&mut markdown, output);
    render_drill_suite_retrieval_blockers(&mut markdown, &output.retrieval_blockers);
    render_drill_suite_repo_table(&mut markdown, &output.repos);
    render_drill_suite_answer_quality_findings(&mut markdown, &output.repos);
    render_drill_suite_repo_artifacts(&mut markdown, &output.repos);
    render_drill_suite_next_actions(&mut markdown, &output.next_actions);
    ensure_trailing_newline(markdown)
}

fn render_drill_suite_header(markdown: &mut String, output: &DrillSuiteOutput) {
    let _ = writeln!(markdown, "# CodeStory Real-Repo Agent Drill Suite");
    let _ = writeln!(markdown);
    let _ = writeln!(markdown, "- suite: `{}`", output.suite);
    let _ = writeln!(markdown, "- project: `{}`", output.project);
    let _ = writeln!(markdown, "- case_file: `{}`", output.case_file);
    let _ = writeln!(markdown, "- output_dir: `{}`", output.output_dir);
    let _ = writeln!(
        markdown,
        "- repos: {} total, {} ready, {} degraded, {} blocked",
        output.repo_count, output.ready_count, output.degraded_count, output.blocked_count
    );
    let _ = writeln!(
        markdown,
        "- answer_quality: {} ready, {} degraded, {} failed, {} pending",
        output.answer_ready_count,
        output.answer_degraded_count,
        output.answer_failed_count,
        output.answer_pending_count
    );
}

fn render_drill_suite_retrieval_blockers(
    markdown: &mut String,
    blockers: &[DrillSuiteRetrievalBlockerOutput],
) {
    if blockers.is_empty() {
        return;
    }

    let _ = writeln!(markdown);
    let _ = writeln!(markdown, "## Retrieval Blockers");
    for blocker in blockers {
        let _ = writeln!(
            markdown,
            "- `{}` repos={} [{}]: {}",
            blocker.status,
            blocker.repo_count,
            blocker.repos.join(", "),
            blocker.next_action
        );
    }
}

fn render_drill_suite_repo_table(markdown: &mut String, repos: &[DrillSuiteRepoOutput]) {
    let _ = writeln!(markdown);
    let _ = writeln!(
        markdown,
        "| repo | verdict | answer quality | freshness | retrieval | anchors | bridges | source truth | reports | next action |"
    );
    let _ = writeln!(markdown, "|---|---|---|---|---|---:|---:|---|---|---|");
    for repo in repos {
        let reports = drill_suite_repo_report_label(repo);
        let _ = writeln!(
            markdown,
            "| `{}` | {} | {} | {} | {} | {}/{} | {} | {} | {} | {} |",
            repo.slug,
            repo.summary.verdict.status,
            drill_suite_answer_quality_label(&repo.answer_quality),
            repo.summary
                .mechanical
                .freshness_status
                .as_deref()
                .unwrap_or("unknown"),
            drill_suite_retrieval_label(repo.summary.mechanical.retrieval_status.as_deref()),
            repo.summary.anchors.resolved,
            repo.summary.anchors.requested,
            drill_suite_bridge_label(&repo.summary.bridges),
            drill_suite_source_truth_label_for_repo(repo),
            reports,
            drill_suite_next_action(repo).replace('|', "\\|")
        );
    }
}

fn render_drill_suite_answer_quality_findings(
    markdown: &mut String,
    repos: &[DrillSuiteRepoOutput],
) {
    if !repos.iter().any(|repo| {
        !repo.answer_quality.warnings.is_empty()
            || !repo.answer_quality.missing_expected_files.is_empty()
            || !repo.answer_quality.forbidden_claim_hits.is_empty()
    }) {
        return;
    }

    let _ = writeln!(markdown);
    let _ = writeln!(markdown, "## Answer Quality Findings");
    for repo in repos {
        let quality = &repo.answer_quality;
        if quality.warnings.is_empty()
            && quality.missing_expected_files.is_empty()
            && quality.forbidden_claim_hits.is_empty()
        {
            continue;
        }
        let _ = writeln!(
            markdown,
            "- `{}`: status={} ledger={} claims={}/{} correct partial={} misleading={} unsupported={} material_revisions={}",
            repo.slug,
            quality.final_answer_status,
            quality.ledger_status,
            quality.claim_correct_count,
            quality.claim_count,
            quality.claim_partial_count,
            quality.claim_misleading_count,
            quality.claim_unsupported_count,
            quality.material_revision_count
        );
        if !quality.missing_expected_files.is_empty() {
            let _ = writeln!(
                markdown,
                "  - missing_expected_files: {}",
                quality.missing_expected_files.join(", ")
            );
        }
        for hit in &quality.forbidden_claim_hits {
            let _ = writeln!(markdown, "  - forbidden_claim_hit: {hit}");
        }
        for warning in &quality.warnings {
            let _ = writeln!(markdown, "  - {warning}");
        }
    }
}

fn render_drill_suite_repo_artifacts(markdown: &mut String, repos: &[DrillSuiteRepoOutput]) {
    if repos.is_empty() {
        return;
    }

    let _ = writeln!(markdown);
    let _ = writeln!(markdown, "## Repo Artifacts");
    for repo in repos {
        if repo.summary.full_report_markdown.is_empty() && repo.summary.full_report_json.is_empty()
        {
            let _ = writeln!(
                markdown,
                "- `{}`: no per-repo artifacts were written because the case blocked before evidence collection",
                repo.slug
            );
            continue;
        }
        let markdown_report =
            drill_suite_join_artifact_path(&repo.output_dir, &repo.summary.full_report_markdown);
        let json_report =
            drill_suite_join_artifact_path(&repo.output_dir, &repo.summary.full_report_json);
        let bridge_artifacts = drill_suite_join_artifact_path(
            &repo.output_dir,
            &format!("*-bridge.{}", repo.artifact_extension),
        );
        let _ = writeln!(
            markdown,
            "- `{}`: report `{}`; json `{}`; bridge artifacts `{}`",
            repo.slug, markdown_report, json_report, bridge_artifacts
        );
    }
}

fn render_drill_suite_next_actions(markdown: &mut String, next_actions: &[String]) {
    if next_actions.is_empty() {
        return;
    }

    let _ = writeln!(markdown);
    let _ = writeln!(markdown, "## Next Actions");
    for action in next_actions {
        let _ = writeln!(markdown, "- {action}");
    }
}

fn drill_suite_answer_quality_label(quality: &DrillSuiteAnswerQualityOutput) -> String {
    let expected = if quality.expected_file_count > 0 {
        format!(
            "; expected_files={}/{}",
            quality.expected_file_found_count, quality.expected_file_count
        )
    } else {
        String::new()
    };
    format!(
        "{} ({}, claims={} correct={} partial={} misleading={} unsupported={} revisions={}{})",
        quality.final_answer_status,
        quality.ledger_status,
        quality.claim_count,
        quality.claim_correct_count,
        quality.claim_partial_count,
        quality.claim_misleading_count,
        quality.claim_unsupported_count,
        quality.material_revision_count,
        expected
    )
    .replace('|', "\\|")
}

fn drill_suite_repo_report_label(repo: &DrillSuiteRepoOutput) -> String {
    if repo.summary.full_report_markdown.is_empty() && repo.summary.full_report_json.is_empty() {
        return "not written (blocked before evidence)".to_string();
    }
    let markdown_report =
        drill_suite_join_artifact_path(&repo.output_dir, &repo.summary.full_report_markdown);
    let json_report =
        drill_suite_join_artifact_path(&repo.output_dir, &repo.summary.full_report_json);
    format!("`{markdown_report}` / `{json_report}`").replace('|', "\\|")
}

fn drill_suite_join_artifact_path(output_dir: &str, artifact: &str) -> String {
    if artifact.contains(':')
        || artifact.starts_with('/')
        || artifact.starts_with('\\')
        || artifact.contains('/')
        || artifact.contains('\\')
    {
        return artifact.to_string();
    }
    format!(
        "{}/{}",
        output_dir.trim_end_matches(['/', '\\']),
        artifact.trim_start_matches(['/', '\\'])
    )
}

fn drill_suite_bridge_label(bridges: &DrillSummaryBridgesOutput) -> String {
    format!(
        "{} graph / {} partial / {} unresolved-error",
        bridges.graph_path, bridges.partial, bridges.unresolved_or_error
    )
}

fn drill_suite_source_truth_label_for_repo(repo: &DrillSuiteRepoOutput) -> String {
    let quality = &repo.answer_quality;
    if quality.ledger_status == "present"
        && !matches!(
            quality.final_answer_status.as_str(),
            "pending_source_verification" | "blocked"
        )
    {
        return format!(
            "ledger claims={} correct={} partial={} misleading={} unsupported={} revisions={}; packet {} targets / {} pending",
            quality.claim_count,
            quality.claim_correct_count,
            quality.claim_partial_count,
            quality.claim_misleading_count,
            quality.claim_unsupported_count,
            quality.material_revision_count,
            repo.summary.source_truth.target_file_count,
            repo.summary.source_truth.pending_check_count
        )
        .replace('|', "\\|");
    }
    drill_suite_source_truth_label(&repo.summary.source_truth)
}

fn drill_suite_source_truth_label(source_truth: &DrillSummarySourceTruthOutput) -> String {
    if source_truth.required
        || source_truth.pending_check_count > 0
        || source_truth.verified_check_count > 0
    {
        return format!(
            "{} targets / {} verified / {} pending",
            source_truth.target_file_count,
            source_truth.verified_check_count,
            source_truth.pending_check_count
        );
    }
    format!(
        "{} targets / {} checks",
        source_truth.target_file_count, source_truth.check_count
    )
}

fn drill_suite_next_action(repo: &DrillSuiteRepoOutput) -> String {
    let quality = &repo.answer_quality;
    match quality.final_answer_status.as_str() {
        "ready" => {
            if repo.summary.verdict.status == "ready" {
                "answer is source-verified; keep the artifacts as the ready baseline".to_string()
            } else if repo.summary.bridges.partial > 0 || repo.summary.bridges.graph_path == 0 {
                format!(
                    "answer is source-verified; improve graph/bridge evidence before promoting the mechanical verdict ({} partial bridge(s), {} graph bridge(s))",
                    repo.summary.bridges.partial, repo.summary.bridges.graph_path
                )
            } else {
                "answer is source-verified; inspect the mechanical degraded reason before promotion"
                    .to_string()
            }
        }
        "degraded" => {
            if quality.material_revision_count > 0 || quality.claim_partial_count > 0 {
                format!(
                    "revise partial or materially changed claims, then rerun with the updated ledger (partial={}, revisions={})",
                    quality.claim_partial_count, quality.material_revision_count
                )
            } else {
                "inspect answer-quality warnings and update the ledger or expected evidence"
                    .to_string()
            }
        }
        "failed" => format!(
            "remove or correct misleading/unsupported final claims before trusting the answer (misleading={}, unsupported={})",
            quality.claim_misleading_count, quality.claim_unsupported_count
        ),
        "blocked" => repo.summary.verdict.next_action.clone(),
        _ => repo.summary.verdict.next_action.clone(),
    }
}

fn drill_suite_retrieval_blockers(
    repos: &[DrillSuiteRepoOutput],
) -> Vec<DrillSuiteRetrievalBlockerOutput> {
    let mut grouped = BTreeMap::<String, Vec<String>>::new();
    for repo in repos {
        let Some(status) = repo.summary.mechanical.retrieval_status.as_ref() else {
            continue;
        };
        if drill_suite_retrieval_label(Some(status)) == "full" {
            continue;
        }
        grouped
            .entry(status.clone())
            .or_default()
            .push(repo.slug.clone());
    }
    grouped
        .into_iter()
        .map(|(status, repos)| {
            let next_action = if status.contains("MissingEmbeddingRuntime") {
                "run `codestory-cli setup embeddings --project <repo>`, then rebuild with `codestory-cli retrieval index --project <repo> --refresh full` before trusting packet/search evidence".to_string()
            } else if status.contains("MissingSemanticDocs") {
                "rerun `codestory-cli retrieval index --project <repo> --refresh full` after semantic setup before trusting packet/search evidence".to_string()
            } else {
                "inspect doctor/retrieval status and repair to retrieval_mode=full before treating broad search quality as repo-specific".to_string()
            };
            DrillSuiteRetrievalBlockerOutput {
                status,
                repo_count: repos.len(),
                repos,
                next_action,
            }
        })
        .collect()
}

fn drill_suite_answer_quality(
    summary: &DrillSummaryOutput,
    expectations: &DrillSuiteExpectationOutput,
    ledger_case: Option<&DrillSuiteLedgerCase>,
    ledger_supplied: bool,
) -> DrillSuiteAnswerQualityOutput {
    let (missing_expected_files, expected_file_found_count, expected_file_recall) =
        drill_suite_expected_file_stats(expectations, &summary.source_truth.target_files);
    let expected_file_count = expectations.source_truth_files.len();
    let ledger_status = drill_suite_ledger_status(ledger_case, ledger_supplied);
    let mut warnings = Vec::new();
    let mut layer_findings = Vec::new();
    let mut draft_written = None;
    let mut claim_count = 0usize;
    let mut claim_correct_count = 0usize;
    let mut claim_partial_count = 0usize;
    let mut claim_misleading_count = 0usize;
    let mut claim_unsupported_count = 0usize;
    let claim_unclassified_count = 0usize;
    let mut material_revision_count = 0usize;
    let mut forbidden_claim_hits = Vec::new();

    if !missing_expected_files.is_empty() {
        warnings.push(format!(
            "{} expected source-truth file(s) were not emitted as drill targets",
            missing_expected_files.len()
        ));
    }

    if summary.verdict.status == "blocked" {
        warnings.push("drill blocked before answer-quality scoring could complete".to_string());
        return DrillSuiteAnswerQualityOutput {
            ledger_status,
            final_answer_status: "blocked".to_string(),
            draft_written,
            claim_count,
            claim_correct_count,
            claim_partial_count,
            claim_misleading_count,
            claim_unsupported_count,
            claim_unclassified_count,
            material_revision_count,
            expected_file_count,
            expected_file_found_count,
            expected_file_missing_count: missing_expected_files.len(),
            expected_file_recall,
            missing_expected_files,
            forbidden_claim_count: 0,
            forbidden_claim_hits,
            layer_findings,
            warnings,
        };
    }

    let Some(ledger_case) = ledger_case else {
        warnings.push(if ledger_supplied {
            "ledger was supplied, but this repo slug had no matching case".to_string()
        } else {
            "no source-truth ledger supplied; final answer quality is still pending".to_string()
        });
        return DrillSuiteAnswerQualityOutput {
            ledger_status,
            final_answer_status: "pending_source_verification".to_string(),
            draft_written,
            claim_count,
            claim_correct_count,
            claim_partial_count,
            claim_misleading_count,
            claim_unsupported_count,
            claim_unclassified_count,
            material_revision_count,
            expected_file_count,
            expected_file_found_count,
            expected_file_missing_count: missing_expected_files.len(),
            expected_file_recall,
            missing_expected_files,
            forbidden_claim_count: 0,
            forbidden_claim_hits,
            layer_findings,
            warnings,
        };
    };

    draft_written = ledger_case.draft_written;
    claim_count = ledger_case.claims.len();
    for claim in &ledger_case.claims {
        match claim.classification {
            DrillSuiteClaimClassification::Correct => claim_correct_count += 1,
            DrillSuiteClaimClassification::Partial => claim_partial_count += 1,
            DrillSuiteClaimClassification::Misleading => claim_misleading_count += 1,
            DrillSuiteClaimClassification::Unsupported => claim_unsupported_count += 1,
        }
        if claim.source_files.is_empty() {
            warnings.push(format!(
                "ledger claim `{}` has no source_files verification evidence",
                claim.id
            ));
        }
        if claim.changed_after_source_read.unwrap_or(false) {
            material_revision_count += 1;
        }
        if drill_suite_claim_has_forbidden_final_text(claim, expectations) {
            forbidden_claim_hits.push(format!("{}: {}", claim.id, claim.text));
        }
    }
    layer_findings = ledger_case
        .layer_findings
        .iter()
        .map(|finding| DrillSuiteLayerFindingOutput {
            layer: finding.layer.clone(),
            status: finding.status.clone(),
            detail: finding.detail.clone(),
        })
        .collect();

    if claim_count == 0 {
        warnings.push("ledger case has no verified claims".to_string());
    }
    if draft_written == Some(false) {
        warnings.push("ledger reports that no CodeStory-only draft was written".to_string());
    }

    let final_answer_status = if draft_written == Some(false) || claim_count == 0 {
        "pending_source_verification"
    } else if claim_unsupported_count > 0
        || claim_misleading_count > 0
        || !forbidden_claim_hits.is_empty()
    {
        "failed"
    } else if claim_partial_count > 0
        || material_revision_count > 0
        || !missing_expected_files.is_empty()
    {
        "degraded"
    } else {
        "ready"
    };

    DrillSuiteAnswerQualityOutput {
        ledger_status,
        final_answer_status: final_answer_status.to_string(),
        draft_written,
        claim_count,
        claim_correct_count,
        claim_partial_count,
        claim_misleading_count,
        claim_unsupported_count,
        claim_unclassified_count,
        material_revision_count,
        expected_file_count,
        expected_file_found_count,
        expected_file_missing_count: missing_expected_files.len(),
        expected_file_recall,
        missing_expected_files,
        forbidden_claim_count: forbidden_claim_hits.len(),
        forbidden_claim_hits,
        layer_findings,
        warnings,
    }
}

fn drill_suite_blocked_answer_quality(
    expectations: &DrillSuiteExpectationOutput,
    ledger_case: Option<&DrillSuiteLedgerCase>,
    ledger_supplied: bool,
) -> DrillSuiteAnswerQualityOutput {
    let (missing_expected_files, expected_file_found_count, expected_file_recall) =
        drill_suite_expected_file_stats(expectations, &[]);
    let ledger_status = drill_suite_ledger_status(ledger_case, ledger_supplied);
    DrillSuiteAnswerQualityOutput {
        ledger_status,
        final_answer_status: "blocked".to_string(),
        draft_written: ledger_case.and_then(|case| case.draft_written),
        claim_count: ledger_case
            .map(|case| case.claims.len())
            .unwrap_or_default(),
        claim_correct_count: 0,
        claim_partial_count: 0,
        claim_misleading_count: 0,
        claim_unsupported_count: 0,
        claim_unclassified_count: 0,
        material_revision_count: 0,
        expected_file_count: expectations.source_truth_files.len(),
        expected_file_found_count,
        expected_file_missing_count: missing_expected_files.len(),
        expected_file_recall,
        missing_expected_files,
        forbidden_claim_count: 0,
        forbidden_claim_hits: Vec::new(),
        layer_findings: ledger_case
            .map(|case| {
                case.layer_findings
                    .iter()
                    .map(|finding| DrillSuiteLayerFindingOutput {
                        layer: finding.layer.clone(),
                        status: finding.status.clone(),
                        detail: finding.detail.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default(),
        warnings: vec!["drill blocked before answer-quality scoring could complete".to_string()],
    }
}

fn drill_suite_ledger_status(
    ledger_case: Option<&DrillSuiteLedgerCase>,
    ledger_supplied: bool,
) -> String {
    if ledger_case.is_some() {
        "present".to_string()
    } else if ledger_supplied {
        "case_missing".to_string()
    } else {
        "not_supplied".to_string()
    }
}

fn drill_suite_expected_file_stats(
    expectations: &DrillSuiteExpectationOutput,
    target_files: &[String],
) -> (Vec<String>, usize, Option<f32>) {
    if expectations.source_truth_files.is_empty() {
        return (Vec::new(), 0, None);
    }
    let target_keys = target_files
        .iter()
        .map(|path| drill_suite_path_key(path))
        .collect::<HashSet<_>>();
    let mut missing = Vec::new();
    let mut found = 0usize;
    for expected in &expectations.source_truth_files {
        if target_keys.contains(&drill_suite_path_key(expected)) {
            found += 1;
        } else {
            missing.push(expected.clone());
        }
    }
    (
        missing,
        found,
        Some(found as f32 / expectations.source_truth_files.len() as f32),
    )
}

fn drill_suite_claim_has_forbidden_final_text(
    claim: &DrillSuiteLedgerClaim,
    expectations: &DrillSuiteExpectationOutput,
) -> bool {
    if matches!(
        claim.classification,
        DrillSuiteClaimClassification::Misleading | DrillSuiteClaimClassification::Unsupported
    ) {
        return false;
    }
    let claim_text = drill_suite_text_key(&claim.text);
    expectations.false_claims.iter().any(|false_claim| {
        let false_claim = drill_suite_text_key(false_claim);
        !false_claim.is_empty() && claim_text.contains(&false_claim)
    })
}

fn drill_suite_path_key(path: &str) -> String {
    let mut value = path.trim().replace('\\', "/");
    while let Some(stripped) = value.strip_prefix("./") {
        value = stripped.to_string();
    }
    value.trim_matches('/').to_ascii_lowercase()
}

fn drill_suite_text_key(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

fn drill_execution_boundaries() -> Vec<DrillExecutionBoundaryOutput> {
    vec![
        DrillExecutionBoundaryOutput {
            command: "drill".to_string(),
            flow: vec![
                "codestory-cli::run_drill validates output dir and opens RuntimeContext"
                    .to_string(),
                "RuntimeContext::ensure_open refreshes workspace/index state before evidence commands"
                    .to_string(),
                "run_drill_anchor runs search, symbol, trail, explore, and function-body snippet artifacts"
                    .to_string(),
                "drill_evidence_packet and drill_summary turn artifacts into answer-readiness evidence"
                    .to_string(),
            ],
            source_files: vec![
                "crates/codestory-cli/src/main.rs".to_string(),
                "crates/codestory-runtime/src/lib.rs".to_string(),
                "crates/codestory-runtime/src/grounding.rs".to_string(),
            ],
        },
        DrillExecutionBoundaryOutput {
            command: "trail".to_string(),
            flow: vec![
                "codestory-cli::run_trail resolves the requested symbol into a NodeId".to_string(),
                "CodebaseBrowser::trail_context loads focus details and delegates graph traversal"
                    .to_string(),
                "graph_builders::graph_trail converts TrailConfigDto into store traversal and DTO edges"
                    .to_string(),
                "codestory-store::get_trail performs bounded graph traversal over persisted index edges"
                    .to_string(),
            ],
            source_files: vec![
                "crates/codestory-cli/src/main.rs".to_string(),
                "crates/codestory-runtime/src/browser.rs".to_string(),
                "crates/codestory-runtime/src/grounding.rs".to_string(),
                "crates/codestory-runtime/src/graph_builders.rs".to_string(),
                "crates/codestory-store/src/storage_impl/trail.rs".to_string(),
            ],
        },
        DrillExecutionBoundaryOutput {
            command: "search/snippet".to_string(),
            flow: vec![
                "codestory-cli search/snippet requests go through RuntimeContext and CodebaseBrowser"
                    .to_string(),
                "SearchService combines indexed-symbol, repo-text, semantic, graph, and search-plan evidence"
                    .to_string(),
                "snippet_context reads source-backed line/function ranges for the selected NodeId"
                    .to_string(),
            ],
            source_files: vec![
                "crates/codestory-cli/src/main.rs".to_string(),
                "crates/codestory-runtime/src/services.rs".to_string(),
                "crates/codestory-runtime/src/grounding.rs".to_string(),
                "crates/codestory-store/src/search.rs".to_string(),
            ],
        },
    ]
}

fn validate_drill_output_dir(output_dir: &std::path::Path) -> Result<()> {
    fs::create_dir_all(output_dir).with_context(|| {
        format!(
            "Failed to create drill output directory {}",
            display::clean_path_string(&output_dir.to_string_lossy())
        )
    })
}

struct DrillReportContents {
    selected: String,
    markdown: String,
    json: String,
}

fn render_drill_contents(
    format: args::OutputFormat,
    output: &DrillOutput,
    markdown: &str,
) -> Result<DrillReportContents> {
    let markdown = ensure_trailing_newline(markdown.to_string());
    let json = ensure_trailing_newline(
        serde_json::to_string_pretty(output).context("Failed to serialize drill JSON")?,
    );
    let selected = match format {
        args::OutputFormat::Markdown => markdown.clone(),
        args::OutputFormat::Json => json.clone(),
        args::OutputFormat::Dot => bail!("--format dot is only supported by `trail`"),
    };
    Ok(DrillReportContents {
        selected,
        markdown,
        json,
    })
}

fn write_drill_report_file(path: &std::path::Path, content: &str) -> Result<()> {
    fs::write(path, content).with_context(|| {
        format!(
            "Failed to write drill report {}",
            display::clean_path_string(&path.to_string_lossy())
        )
    })
}

fn drill_summary(output: &DrillOutput) -> DrillSummaryOutput {
    let readiness = &output.evidence_packet.readiness;
    let anchor_statuses: Vec<_> = output
        .anchors
        .iter()
        .map(|anchor| {
            let failed_command_count = anchor
                .commands
                .iter()
                .filter(|command| command.status != "ok")
                .count();
            let command_duration_ms = anchor
                .commands
                .iter()
                .map(|command| command.duration_ms)
                .sum();
            let slowest = anchor
                .commands
                .iter()
                .max_by_key(|command| command.duration_ms);
            DrillSummaryAnchorStatusOutput {
                anchor: anchor.anchor.clone(),
                status: if anchor.chosen_anchor.is_some() {
                    "resolved".to_string()
                } else {
                    "unresolved".to_string()
                },
                typed_hit_count: anchor.typed_hit_count,
                selected: anchor
                    .chosen_anchor
                    .as_ref()
                    .map(|hit| hit.display_name.clone()),
                selected_node_id: anchor.chosen_anchor.as_ref().map(|hit| hit.node_id.clone()),
                selected_node_ref: anchor
                    .chosen_anchor
                    .as_ref()
                    .and_then(|hit| hit.node_ref.clone()),
                selected_kind: anchor.chosen_anchor.as_ref().map(|hit| hit.kind),
                selected_file_path: anchor
                    .chosen_anchor
                    .as_ref()
                    .and_then(|hit| hit.file_path.clone()),
                selected_line: anchor.chosen_anchor.as_ref().and_then(|hit| hit.line),
                caller_count: anchor
                    .consumer_summary
                    .as_ref()
                    .map(|summary| summary.caller_count)
                    .unwrap_or_default(),
                consumer_count: anchor
                    .consumer_summary
                    .as_ref()
                    .map(|summary| summary.consumer_count)
                    .unwrap_or_default(),
                text_hint_count: anchor
                    .consumer_summary
                    .as_ref()
                    .map(|summary| summary.text_hint_count)
                    .unwrap_or_default(),
                command_count: anchor.commands.len(),
                failed_command_count,
                command_duration_ms,
                total_duration_ms: anchor.timings.total_ms,
                resolution_duration_ms: anchor.timings.resolution_ms,
                consumer_summary_duration_ms: anchor.timings.consumer_summary_ms,
                slowest_command: slowest.map(|command| command.command.clone()),
                slowest_command_ms: slowest
                    .map(|command| command.duration_ms)
                    .unwrap_or_default(),
                source_truth_target_count: anchor.verification_targets.len(),
            }
        })
        .collect();
    let resolved = anchor_statuses
        .iter()
        .filter(|anchor| anchor.status == "resolved")
        .count();
    let failed_anchor_commands = anchor_statuses
        .iter()
        .map(|anchor| anchor.failed_command_count)
        .sum();

    let bridge_statuses: Vec<_> = output
        .bridges
        .iter()
        .map(|bridge| DrillSummaryBridgeStatusOutput {
            from_anchor: bridge.evidence.from_anchor.clone(),
            to_anchor: bridge.evidence.to_anchor.clone(),
            status: bridge.evidence.status.clone(),
            confidence: bridge.evidence.confidence.clone(),
            strategy: bridge.evidence.strategy.clone(),
            command_status: bridge.command.status.clone(),
        })
        .collect();
    let graph_path = bridge_statuses
        .iter()
        .filter(|bridge| drill_bridge_status_is_graph(&bridge.status))
        .count();
    let partial = bridge_statuses
        .iter()
        .filter(|bridge| drill_bridge_status_is_partial(&bridge.status))
        .count();
    let unresolved_or_error = bridge_statuses
        .iter()
        .filter(|bridge| {
            drill_bridge_status_is_unresolved(&bridge.status) || bridge.command_status != "ok"
        })
        .count();

    let mut target_files: Vec<_> = readiness
        .source_truth_checks
        .iter()
        .map(|check| check.path.clone())
        .collect();
    dedupe_and_rank_drill_files(&mut target_files);
    let target_file_details =
        drill_summary_source_truth_target_details(&target_files, &readiness.source_truth_checks);

    let has_source_truth_checks = !readiness.source_truth_checks.is_empty();
    let needs_source_truth = has_source_truth_checks;
    let stale_freshness = output
        .mechanical
        .freshness
        .as_ref()
        .is_some_and(|freshness| freshness.status == IndexFreshnessStatusDto::Stale);
    let open_gap_friendly = !readiness.needs_verification.is_empty()
        || !readiness.inferred_claims.is_empty()
        || needs_source_truth
        || stale_freshness;

    DrillSummaryOutput {
        summary_version: 1,
        project: output.project.clone(),
        label: output.label.clone(),
        question: output.question.clone(),
        output_dir: output.output_dir.clone(),
        full_report_json: "drill-report.json".to_string(),
        full_report_markdown: "drill-report.md".to_string(),
        mechanical: DrillSummaryMechanicalOutput {
            refresh: output.mechanical.refresh.clone(),
            before: drill_summary_stats(
                output.mechanical.before_files,
                output.mechanical.before_nodes,
                output.mechanical.before_edges,
                output.mechanical.before_errors,
            ),
            after: drill_summary_stats(
                output.mechanical.after_files,
                output.mechanical.after_nodes,
                output.mechanical.after_edges,
                output.mechanical.after_errors,
            ),
            index_ready: output.mechanical.after_files > 0 && output.mechanical.after_errors == 0,
            error_delta: i64::from(output.mechanical.after_errors)
                - i64::from(output.mechanical.before_errors),
            retrieval_status: output
                .mechanical
                .retrieval
                .as_ref()
                .map(|retrieval| {
                    drill_summary_retrieval_status(
                        retrieval,
                        output.mechanical.sidecar_retrieval_mode.as_deref(),
                    )
                })
                .or_else(|| output.mechanical.sidecar_retrieval_mode.clone()),
            freshness_status: output
                .mechanical
                .freshness
                .as_ref()
                .map(drill_summary_freshness_status),
            stale_file_count: output
                .mechanical
                .freshness
                .as_ref()
                .map(drill_summary_stale_file_count)
                .unwrap_or_default(),
            freshness_samples: output
                .mechanical
                .freshness
                .as_ref()
                .map(drill_summary_freshness_samples)
                .unwrap_or_default(),
            phase_timing_available: output.mechanical.phase_timings.is_some(),
            drill_timings: output.mechanical.drill_timings.clone(),
        },
        anchors: DrillSummaryAnchorsOutput {
            requested: output.anchors.len(),
            resolved,
            unresolved: output.anchors.len().saturating_sub(resolved),
            failed_command_count: failed_anchor_commands,
            statuses: anchor_statuses,
        },
        bridges: DrillSummaryBridgesOutput {
            total: output.bridges.len(),
            graph_path,
            partial,
            unresolved_or_error,
            statuses: bridge_statuses,
        },
        source_truth: DrillSummarySourceTruthOutput {
            required: needs_source_truth,
            check_count: readiness.source_truth_checks.len(),
            pending_check_count: if has_source_truth_checks {
                readiness.source_truth_checks.len()
            } else {
                0
            },
            verified_check_count: 0,
            target_file_count: target_files.len(),
            target_files,
            target_file_details,
            checklist_item_count: output.verification_checklist.len(),
            claim_count: output.claim_ledger_template.claims.len(),
            pending_claim_count: output.claim_ledger_template.claims.len(),
            verified_claim_count: 0,
        },
        open_gaps: DrillSummaryOpenGapsOutput {
            overall_status: readiness.overall_status,
            answer_quality_status: drill_answer_quality_status(
                needs_source_truth,
                output.claim_ledger_template.claims.len(),
            ),
            safe_to_say_count: readiness.safe_to_say.len(),
            inferred_claim_count: readiness.inferred_claims.len(),
            needs_verification_count: readiness.needs_verification.len(),
            needs_verification_claim_count: readiness.needs_verification.len(),
            pending_claim_count: if needs_source_truth {
                output.claim_ledger_template.claims.len()
            } else {
                0
            },
            pending_source_truth_check_count: if needs_source_truth {
                readiness.source_truth_checks.len()
            } else {
                0
            },
            next_command_count: readiness.next_commands.len(),
            open_gap_friendly,
            status: if open_gap_friendly {
                "open_gaps_explicit".to_string()
            } else {
                "no_open_gaps_reported".to_string()
            },
        },
        verdict: drill_summary_verdict(
            output,
            resolved,
            graph_path,
            partial,
            unresolved_or_error,
            needs_source_truth,
            open_gap_friendly,
            stale_freshness,
        ),
    }
}

fn drill_answer_quality_status(needs_source_truth: bool, claim_count: usize) -> String {
    if needs_source_truth && claim_count > 0 {
        "pending_source_verification".to_string()
    } else if needs_source_truth {
        "pending_source_truth_checks".to_string()
    } else {
        "ready_from_codestory_evidence".to_string()
    }
}

fn drill_bridge_status_is_graph(status: &str) -> bool {
    matches!(
        status,
        "graph_path" | "reverse_graph_path" | "graph_shared_file"
    )
}

fn drill_bridge_status_is_partial(status: &str) -> bool {
    matches!(
        status,
        "shared_file_only"
            | "evidence_hint_only"
            | "framework_route"
            | "component_usage"
            | "data_collection_usage"
            | "source_truth_only"
    )
}

fn drill_bridge_status_is_unresolved(status: &str) -> bool {
    matches!(status, "no_bridge_found" | "unresolved_anchor" | "error")
}

#[allow(clippy::too_many_arguments)]
fn drill_summary_verdict(
    output: &DrillOutput,
    resolved_anchors: usize,
    graph_path_bridges: usize,
    partial_bridges: usize,
    unresolved_or_error_bridges: usize,
    needs_source_truth: bool,
    open_gap_friendly: bool,
    stale_freshness: bool,
) -> DrillSummaryVerdictOutput {
    let failed_anchor_commands = output
        .anchors
        .iter()
        .flat_map(|anchor| anchor.commands.iter())
        .filter(|command| command.status != "ok")
        .count();
    let unresolved_anchors = output.anchors.len().saturating_sub(resolved_anchors);
    if output.mechanical.after_files == 0 || output.mechanical.after_errors > 0 {
        return DrillSummaryVerdictOutput {
            status: "blocked".to_string(),
            reason: "index is not ready or contains indexing errors".to_string(),
            next_action: "inspect doctor/index output before trusting drill evidence".to_string(),
        };
    }
    if unresolved_anchors > 0 || failed_anchor_commands > 0 {
        return DrillSummaryVerdictOutput {
            status: "blocked".to_string(),
            reason: format!(
                "unresolved_anchors={unresolved_anchors} failed_anchor_commands={failed_anchor_commands}"
            ),
            next_action: "repair anchor selection or inspect command errors before answering"
                .to_string(),
        };
    }
    if stale_freshness {
        return DrillSummaryVerdictOutput {
            status: "degraded".to_string(),
            reason: format!(
                "index_freshness=stale source_truth_required={} graph_bridges={graph_path_bridges}/{} partial_bridges={partial_bridges} unresolved_or_error_bridges={unresolved_or_error_bridges} pending_source_truth_checks={}",
                needs_source_truth,
                output.bridges.len(),
                output.evidence_packet.readiness.source_truth_checks.len()
            ),
            next_action: drill_stale_freshness_next_action(output),
        };
    }
    if needs_source_truth || open_gap_friendly || unresolved_or_error_bridges > 0 {
        return DrillSummaryVerdictOutput {
            status: "degraded".to_string(),
            reason: format!(
                "source_truth_required={} graph_bridges={graph_path_bridges}/{} partial_bridges={partial_bridges} unresolved_or_error_bridges={unresolved_or_error_bridges} pending_source_truth_checks={}",
                needs_source_truth,
                output.bridges.len(),
                output.evidence_packet.readiness.source_truth_checks.len()
            ),
            next_action: drill_degraded_next_action(output, unresolved_or_error_bridges),
        };
    }
    DrillSummaryVerdictOutput {
        status: "ready".to_string(),
        reason: "all anchors resolved and no open source-truth blockers were reported".to_string(),
        next_action: "answer from the evidence packet and keep source verification focused"
            .to_string(),
    }
}

fn drill_stale_freshness_next_action(output: &DrillOutput) -> String {
    let project = quote_command_path(std::path::Path::new(&output.project));
    let mut action = format!(
        "refresh stale index evidence first with `codestory-cli index --project {project} --refresh incremental`, then rerun drill before finalizing"
    );
    if let Some(freshness) = output.mechanical.freshness.as_ref() {
        let samples = freshness
            .samples
            .iter()
            .take(3)
            .map(|sample| sample.path.clone())
            .collect::<Vec<_>>();
        if !samples.is_empty() {
            let _ = write!(action, "; stale samples: {}", samples.join("; "));
        }
    }
    action
}

fn drill_degraded_next_action(output: &DrillOutput, unresolved_or_error_bridges: usize) -> String {
    let failed_bridge_count = output
        .bridges
        .iter()
        .filter(|bridge| bridge.command.status != "ok" || bridge.evidence.status == "error")
        .count();
    if failed_bridge_count > 0 {
        return format!(
            "repair or rerun {failed_bridge_count} failed bridge evidence command(s) before treating degraded bridges as verification targets"
        );
    }
    let degraded_bridge_count = output
        .bridges
        .iter()
        .filter(|bridge| !drill_bridge_status_is_graph(&bridge.evidence.status))
        .count()
        .max(unresolved_or_error_bridges);
    let mut files = output
        .evidence_packet
        .readiness
        .source_truth_checks
        .iter()
        .map(|check| check.path.clone())
        .collect::<Vec<_>>();
    dedupe_and_rank_drill_files(&mut files);

    let mut action = "write a CodeStory-only draft".to_string();
    let pending_claim_count = output.claim_ledger_template.claims.len();
    if pending_claim_count > 0 && degraded_bridge_count > 0 {
        let _ = write!(
            action,
            ", then verify {pending_claim_count} pending claim(s), starting with {degraded_bridge_count} degraded bridge(s)"
        );
    } else if pending_claim_count > 0 {
        let _ = write!(
            action,
            ", then verify {pending_claim_count} pending claim(s)"
        );
    } else if degraded_bridge_count > 0 {
        let _ = write!(
            action,
            ", then verify {degraded_bridge_count} degraded bridge(s)"
        );
    } else {
        action.push_str(", then verify source-truth targets");
    }
    if !files.is_empty() {
        let preview = files.into_iter().take(3).collect::<Vec<_>>().join("; ");
        let _ = write!(action, " including {preview}");
    }
    if !output.evidence_packet.readiness.next_commands.is_empty() {
        action.push_str("; use emitted bridge/consumer follow-up commands before finalizing");
    }
    action
}

fn drill_summary_stats(files: u32, nodes: u32, edges: u32, errors: u32) -> DrillSummaryStatsOutput {
    DrillSummaryStatsOutput {
        files,
        nodes,
        edges,
        errors,
    }
}

fn drill_summary_retrieval_status(
    retrieval: &codestory_contracts::api::RetrievalStateDto,
    sidecar_retrieval_mode: Option<&str>,
) -> String {
    if let Some(mode) = sidecar_retrieval_mode {
        if mode == "full" {
            return "full".to_string();
        }
        return format!(
            "{mode}:sidecar_degraded; legacy={}",
            drill_summary_legacy_retrieval_status(retrieval)
        );
    }
    drill_summary_legacy_retrieval_status(retrieval)
}

fn drill_summary_legacy_retrieval_status(
    retrieval: &codestory_contracts::api::RetrievalStateDto,
) -> String {
    let mode = match retrieval.mode {
        codestory_contracts::api::RetrievalModeDto::Hybrid => "hybrid",
        codestory_contracts::api::RetrievalModeDto::Symbolic => "symbolic",
    };
    let readiness = if retrieval.semantic_ready {
        "semantic_ready"
    } else {
        "semantic_unavailable"
    };
    match retrieval.fallback_reason {
        Some(reason) => format!("{mode}:{readiness}:diagnostic={reason:?}"),
        None => format!("{mode}:{readiness}"),
    }
}

fn drill_suite_retrieval_label(status: Option<&str>) -> &str {
    match status {
        Some("full") => "full",
        Some(value) if value.contains("sidecar_degraded") => "needs-retrieval-repair",
        Some(value) if value.contains("semantic_ready") || value == "hybrid-ready" => "degraded",
        Some(value) if value.contains("semantic_unavailable") => "needs-retrieval-repair",
        Some("hybrid") => "degraded",
        Some("symbolic") => "needs-retrieval-repair",
        Some(_) => "partial",
        None => "unknown",
    }
}

fn drill_summary_source_truth_target_details(
    target_files: &[String],
    checks: &[SourceTruthCheckDto],
) -> Vec<DrillSummarySourceTruthTargetOutput> {
    target_files
        .iter()
        .map(|path| {
            let check_reasons = checks
                .iter()
                .filter(|check| normalize_drill_path(&check.path) == normalize_drill_path(path))
                .map(|check| check.reason.clone())
                .collect::<Vec<_>>();
            let role = drill_source_truth_target_role(path, &check_reasons);
            DrillSummarySourceTruthTargetOutput {
                path: path.clone(),
                role: role.clone(),
                rank_reason: drill_source_truth_target_rank_reason(path, &role),
                check_reasons,
            }
        })
        .collect()
}

fn normalize_drill_path(path: &str) -> String {
    path.replace('\\', "/").to_ascii_lowercase()
}

fn drill_path_is_framework_route_or_page(path: &str) -> bool {
    let normalized = normalize_drill_path(path);
    normalized.ends_with("/route.ts")
        || normalized.ends_with("/route.tsx")
        || normalized.ends_with("/route.js")
        || normalized.ends_with("/route.jsx")
        || normalized.ends_with("/page.tsx")
        || normalized.ends_with("/page.jsx")
        || ((normalized.contains("/app/") || normalized.contains("/pages/"))
            && (normalized.ends_with(".tsx") || normalized.ends_with(".jsx")))
}

fn drill_path_is_native_or_jvm_source(path: &str) -> bool {
    let normalized = normalize_drill_path(path);
    matches!(
        normalized.rsplit('.').next(),
        Some("c" | "cc" | "cpp" | "cxx" | "h" | "hh" | "hpp" | "hxx" | "java")
    )
}

fn drill_source_truth_target_role(path: &str, reasons: &[String]) -> String {
    let path = normalize_drill_path(path);
    let reason_text = reasons.join(" ").to_ascii_lowercase();
    if drill_path_is_framework_route_or_page(&path) {
        return "public_surface".to_string();
    }
    if path.contains("/components/") && !path.contains("/components/admin") {
        return "runtime_entrypoint".to_string();
    }
    if path.contains("/collections/") || reason_text.contains("collection") {
        return "data_store".to_string();
    }
    if path.contains("comment-auth") || reason_text.contains("auth") {
        return "comment_auth".to_string();
    }
    if path.contains("/tests/") || path.contains(".spec.") || path.contains(".test.") {
        return "test_support".to_string();
    }
    if path.contains("/admin/") || path.contains("/components/admin") {
        return "admin_support".to_string();
    }
    if drill_bridge_evidence_is_generated_path(&format!("/{path}")) {
        return "generated_or_auxiliary".to_string();
    }
    "anchor_definition".to_string()
}

fn drill_source_truth_target_rank_reason(path: &str, role: &str) -> String {
    match role {
        "public_surface" => "ranked ahead as public runtime surface evidence".to_string(),
        "runtime_entrypoint" => "ranked ahead as runtime/component evidence".to_string(),
        "data_store" => "kept as Payload/data-store evidence".to_string(),
        "comment_auth" => "kept as comment authentication evidence".to_string(),
        "test_support" => "demoted behind runtime evidence as test support".to_string(),
        "admin_support" => "demoted behind public runtime evidence as admin support".to_string(),
        "generated_or_auxiliary" => {
            "demoted behind source files as generated or auxiliary evidence".to_string()
        }
        _ if normalize_drill_path(path).contains("/src/") => {
            "ranked as production source evidence".to_string()
        }
        _ => "ranked after primary source surfaces".to_string(),
    }
}

fn drill_summary_freshness_status(freshness: &IndexFreshnessDto) -> String {
    match freshness.status {
        IndexFreshnessStatusDto::Fresh => "fresh".to_string(),
        IndexFreshnessStatusDto::Stale => "stale".to_string(),
        IndexFreshnessStatusDto::NotChecked => "not_checked".to_string(),
    }
}

fn drill_summary_stale_file_count(freshness: &IndexFreshnessDto) -> u32 {
    if freshness.status == IndexFreshnessStatusDto::Stale {
        freshness
            .changed_file_count
            .saturating_add(freshness.new_file_count)
            .saturating_add(freshness.removed_file_count)
    } else {
        0
    }
}

fn drill_summary_freshness_samples(freshness: &IndexFreshnessDto) -> Vec<String> {
    freshness
        .samples
        .iter()
        .take(8)
        .map(|sample| format!("{:?}: {}", sample.kind, sample.path))
        .collect()
}

fn run_drill_anchors_in_order(
    runtime: &RuntimeContext,
    opened: &runtime::OpenedProject,
    output_dir: &std::path::Path,
    format: args::OutputFormat,
    anchors: &[String],
    jobs: usize,
) -> Result<Vec<DrillAnchorOutput>> {
    let jobs = jobs.min(anchors.len()).max(1);
    if jobs == 1 || anchors.len() <= 1 {
        return anchors
            .iter()
            .map(|anchor| run_drill_anchor(runtime, opened, output_dir, format, anchor))
            .collect();
    }

    let indexed_anchors = anchors.iter().enumerate().collect::<Vec<_>>();
    let chunk_size = indexed_anchors.len().div_ceil(jobs);
    let mut indexed_outputs = Vec::with_capacity(anchors.len());
    std::thread::scope(|scope| -> Result<()> {
        let mut handles = Vec::new();
        for chunk in indexed_anchors.chunks(chunk_size) {
            handles.push(scope.spawn(move || {
                chunk
                    .iter()
                    .map(|(index, anchor)| {
                        (
                            *index,
                            run_drill_anchor(runtime, opened, output_dir, format, anchor),
                        )
                    })
                    .collect::<Vec<_>>()
            }));
        }
        for handle in handles {
            let mut chunk = match handle.join() {
                Ok(chunk) => chunk,
                Err(_) => bail!("drill anchor worker panicked"),
            };
            indexed_outputs.append(&mut chunk);
        }
        Ok(())
    })?;

    indexed_outputs.sort_by_key(|(index, _)| *index);
    indexed_outputs
        .into_iter()
        .map(|(_, output)| output)
        .collect()
}

fn run_drill_anchor(
    runtime: &RuntimeContext,
    opened: &runtime::OpenedProject,
    output_dir: &std::path::Path,
    format: args::OutputFormat,
    anchor: &str,
) -> Result<DrillAnchorOutput> {
    let anchor_timer = Instant::now();
    let mut commands = Vec::new();
    let safe_anchor = output_slug(anchor);
    let search_timer = Instant::now();
    let search_results = runtime
        .browser
        .search_results(SearchRequest {
            query: anchor.to_string(),
            repo_text: SearchRepoTextMode::Auto,
            limit_per_source: 10,
            expand_search_plan: false,
            hybrid_weights: None,
            hybrid_limits: None,
        })
        .map_err(map_api_error)?;
    let search_output = search_output_from_results(runtime, &search_results, true);
    let search_markdown = render_search_markdown(&runtime.project_root, &search_output);
    let search_status = with_drill_command_duration(
        search_timer,
        write_drill_artifact(
            output_dir,
            format,
            &format!("{safe_anchor}-search"),
            "search",
            &search_output,
            search_markdown,
        ),
    );
    let search_ms = search_status.duration_ms;
    commands.push(search_status);

    let chosen =
        drill_targeting::choose_drill_anchor_hit(anchor, &search_results.indexed_symbol_hits)
            .cloned();
    let typed_hit_count = search_results
        .indexed_symbol_hits
        .iter()
        .filter(|hit| hit.kind != NodeKind::UNKNOWN)
        .count();
    let Some(chosen) = chosen else {
        let timings = DrillAnchorTimingsOutput {
            total_ms: elapsed_ms(anchor_timer),
            search_ms,
            resolution_ms: 0,
            consumer_summary_ms: 0,
            command_artifacts_ms: commands.iter().map(|command| command.duration_ms).sum(),
        };
        return Ok(DrillAnchorOutput {
            anchor: anchor.to_string(),
            typed_hit_count,
            chosen_anchor: None,
            verification_targets: Vec::new(),
            consumer_summary: None,
            timings,
            commands,
        });
    };

    let target = runtime::ResolvedTarget {
        selector: QuerySelectorOutput::Query,
        requested: anchor.to_string(),
        file_filter: None,
        selected: chosen.clone(),
        alternatives: search_results.indexed_symbol_hits.clone(),
    };
    let resolution_timer = Instant::now();
    let resolution = build_query_resolution_output_with_runtime(runtime, &target);
    let resolution_ms = elapsed_ms(resolution_timer);
    let verification_targets = resolution.resolved.verification_targets.clone();
    let consumer_summary_timer = Instant::now();
    let consumer_summary =
        drill_anchor_consumer_summary(runtime, anchor, &chosen, &verification_targets);
    let consumer_summary_ms = elapsed_ms(consumer_summary_timer);

    commands.push(run_drill_symbol_context(
        runtime,
        output_dir,
        format,
        &safe_anchor,
        &target,
        &resolution,
        &verification_targets,
    ));
    commands.push(run_drill_trail_context(
        runtime,
        output_dir,
        format,
        &safe_anchor,
        &target,
        &resolution,
    ));
    commands.push(run_drill_explore_context(
        runtime,
        opened,
        output_dir,
        format,
        &safe_anchor,
        &target,
    ));
    commands.push(run_drill_snippet_context(
        runtime,
        output_dir,
        format,
        &safe_anchor,
        &target,
        &resolution,
        &verification_targets,
    ));
    let timings = DrillAnchorTimingsOutput {
        total_ms: elapsed_ms(anchor_timer),
        search_ms,
        resolution_ms,
        consumer_summary_ms,
        command_artifacts_ms: commands.iter().map(|command| command.duration_ms).sum(),
    };

    Ok(DrillAnchorOutput {
        anchor: anchor.to_string(),
        typed_hit_count,
        chosen_anchor: Some(resolution.resolved),
        verification_targets,
        consumer_summary,
        timings,
        commands,
    })
}

fn run_drill_explore_context(
    runtime: &RuntimeContext,
    opened: &runtime::OpenedProject,
    output_dir: &std::path::Path,
    format: args::OutputFormat,
    safe_anchor: &str,
    target: &runtime::ResolvedTarget,
) -> DrillCommandStatusOutput {
    let command_timer = Instant::now();
    let status = match explore::build_explore_artifact_for_target(
        runtime,
        opened,
        target,
        args::RefreshMode::None,
        Some(args::ExploreProfile::Architecture),
        3,
        48,
    ) {
        Ok(artifact) => write_drill_artifact(
            output_dir,
            format,
            &format!("{safe_anchor}-explore"),
            "explore",
            &artifact.json,
            artifact.markdown,
        ),
        Err(error) => drill_status_error("explore", error),
    };
    with_drill_command_duration(command_timer, status)
}

fn run_drill_symbol_context(
    runtime: &RuntimeContext,
    output_dir: &std::path::Path,
    format: args::OutputFormat,
    safe_anchor: &str,
    target: &runtime::ResolvedTarget,
    resolution: &QueryResolutionOutput,
    verification_targets: &[VerificationTargetOutput],
) -> DrillCommandStatusOutput {
    let command_timer = Instant::now();
    let status = match runtime
        .browser
        .symbol_context(target.selected.node_id.clone())
    {
        Ok(symbol) => {
            let markdown = render_symbol_markdown(
                &runtime.project_root,
                target,
                &symbol,
                verification_targets,
            );
            let output = SymbolJsonOutput {
                resolution: resolution.clone(),
                symbol: &symbol,
                verification_targets: verification_targets.to_vec(),
            };
            write_drill_artifact(
                output_dir,
                format,
                &format!("{safe_anchor}-symbol"),
                "symbol",
                &output,
                markdown,
            )
        }
        Err(error) => drill_status_error("symbol", error),
    };
    with_drill_command_duration(command_timer, status)
}

fn run_drill_trail_context(
    runtime: &RuntimeContext,
    output_dir: &std::path::Path,
    format: args::OutputFormat,
    safe_anchor: &str,
    target: &runtime::ResolvedTarget,
    resolution: &QueryResolutionOutput,
) -> DrillCommandStatusOutput {
    let command_timer = Instant::now();
    let status = match runtime
        .browser
        .trail_context(drill_trail_request(&target.selected.node_id))
    {
        Ok(trail) => {
            let notes = trail_guidance_notes(&trail);
            let trail_cmd = drill_trail_command(&cmd_project_args(&runtime.project_root), target);
            let mut markdown = if let Some(story) = trail.story.as_ref() {
                render_trail_story_markdown(
                    &runtime.project_root,
                    target,
                    &trail,
                    &trail_cmd,
                    story,
                )
            } else {
                render_trail_markdown(&runtime.project_root, target, &trail, &trail_cmd)
            };
            if !notes.is_empty() {
                let _ = writeln!(markdown, "notes:");
                for note in &notes {
                    let _ = writeln!(markdown, "- {note}");
                }
            }
            let output = TrailJsonOutput {
                resolution: resolution.clone(),
                trail: &trail,
                notes,
            };
            write_drill_artifact(
                output_dir,
                format,
                &format!("{safe_anchor}-trail"),
                "trail",
                &output,
                markdown,
            )
        }
        Err(error) => drill_status_error("trail", error),
    };
    with_drill_command_duration(command_timer, status)
}

fn run_drill_snippet_context(
    runtime: &RuntimeContext,
    output_dir: &std::path::Path,
    format: args::OutputFormat,
    safe_anchor: &str,
    target: &runtime::ResolvedTarget,
    resolution: &QueryResolutionOutput,
    verification_targets: &[VerificationTargetOutput],
) -> DrillCommandStatusOutput {
    let command_timer = Instant::now();
    let target = prefer_function_body_target(&runtime.project_root, target.clone());
    let target_changed = target.selected.node_id.0 != resolution.resolved.node_id;
    let resolution = if target_changed {
        build_query_resolution_output_with_runtime(runtime, &target)
    } else {
        resolution.clone()
    };
    let verification_targets = if target_changed {
        resolution.resolved.verification_targets.clone()
    } else {
        verification_targets.to_vec()
    };
    let status = match runtime
        .browser
        .snippet_function_body_context(target.selected.node_id.clone(), 40)
    {
        Ok(snippet) => {
            let markdown = render_snippet_markdown(
                &runtime.project_root,
                &target,
                &snippet,
                false,
                &verification_targets,
            );
            let output = SnippetJsonOutput {
                resolution,
                snippet: &snippet,
                verification_targets,
            };
            write_drill_artifact(
                output_dir,
                format,
                &format!("{safe_anchor}-snippet"),
                "snippet",
                &output,
                markdown,
            )
        }
        Err(error) => drill_status_error("snippet", error),
    };
    with_drill_command_duration(command_timer, status)
}

fn run_drill_question_search(
    runtime: &RuntimeContext,
    output_dir: &std::path::Path,
    format: args::OutputFormat,
    question: &str,
    anchors: &[String],
) -> Result<(DrillCommandStatusOutput, SearchOutput)> {
    let command_timer = Instant::now();
    let query = drill_question_search_query(question, anchors);
    let search_results = runtime
        .browser
        .search_results(SearchRequest {
            query,
            repo_text: SearchRepoTextMode::On,
            limit_per_source: 25,
            expand_search_plan: true,
            hybrid_weights: None,
            hybrid_limits: None,
        })
        .map_err(map_api_error)?;
    let search_output = search_output_from_results(runtime, &search_results, true);
    let search_markdown = render_search_markdown(&runtime.project_root, &search_output);
    let status = write_drill_artifact(
        output_dir,
        format,
        "question-search",
        "question_search",
        &search_output,
        search_markdown,
    );
    Ok((
        with_drill_command_duration(command_timer, status),
        search_output,
    ))
}

fn drill_question_search_query(question: &str, anchors: &[String]) -> String {
    if anchors.is_empty() {
        return question.to_string();
    }
    format!("{question}\nSeed anchors: {}", anchors.join(", "))
}

fn run_drill_question_supplemental_searches(
    runtime: &RuntimeContext,
    output_dir: &std::path::Path,
    format: args::OutputFormat,
    question: &str,
    anchors: &[DrillAnchorOutput],
) -> Result<Vec<(DrillCommandStatusOutput, SearchOutput)>> {
    let mut outputs = Vec::new();
    for query in drill_question_supplemental_queries(&runtime.project_root, question, anchors) {
        let command_timer = Instant::now();
        let search_results = runtime
            .browser
            .search_results(SearchRequest {
                query: query.clone(),
                repo_text: SearchRepoTextMode::Auto,
                limit_per_source: 10,
                expand_search_plan: true,
                hybrid_weights: None,
                hybrid_limits: None,
            })
            .map_err(map_api_error)?;
        let search_output = search_output_from_results(runtime, &search_results, true);
        let search_markdown = render_search_markdown(&runtime.project_root, &search_output);
        let slug = format!("question-supplement-{}", output_slug(&query));
        let status = write_drill_artifact(
            output_dir,
            format,
            &slug,
            "question_supplement_search",
            &search_output,
            search_markdown,
        );
        outputs.push((
            with_drill_command_duration(command_timer, status),
            search_output,
        ));
    }
    Ok(outputs)
}

fn drill_question_supplemental_queries(
    project_root: &std::path::Path,
    question: &str,
    anchors: &[DrillAnchorOutput],
) -> Vec<String> {
    let lower = question.to_ascii_lowercase();
    let tokens = drill_question_alnum_tokens(&lower);
    let mut queries = Vec::new();
    if contains_any_token(
        &tokens,
        &["public", "page", "pages", "surface", "surfaces", "home"],
    ) {
        queries.push("Home".to_string());
    }
    if contains_any_token(&tokens, &["comment", "comments"]) {
        queries.push("Comments".to_string());
    }
    if contains_any_token(
        &tokens,
        &["post", "posts", "writing", "article", "articles"],
    ) {
        queries.push("Posts".to_string());
    }
    if contains_any_token(&tokens, &["social", "elsewhere", "feed"]) {
        queries.push("social entries".to_string());
        queries.push("elsewhere feed".to_string());
    }
    if contains_any_token(&tokens, &["store", "storage", "persist", "persistence"]) {
        if let Some(project_name) = project_root.file_name().and_then(|name| name.to_str()) {
            queries.push(format!("{project_name}-store"));
        }
        queries.push("Store".to_string());
    }
    for anchor in anchors {
        if let Some(path) = anchor
            .chosen_anchor
            .as_ref()
            .and_then(|hit| hit.file_path.as_deref())
            && path.contains("/collections/")
            && !queries.iter().any(|query| query == &anchor.anchor)
        {
            queries.push(anchor.anchor.clone());
        }
    }
    let mut seen = HashSet::new();
    queries
        .into_iter()
        .filter(|query| seen.insert(query.to_ascii_lowercase()))
        .take(10)
        .collect()
}

fn contains_any_token(tokens: &[String], needles: &[&str]) -> bool {
    needles.iter().any(|needle| {
        tokens
            .iter()
            .any(|token| token.eq_ignore_ascii_case(needle))
    })
}

fn drill_question_search_verification_targets(
    search_output: &SearchOutput,
    reason_prefix: &str,
    max_files: usize,
) -> Vec<VerificationTargetOutput> {
    let mut candidates = search_output
        .indexed_symbol_hits
        .iter()
        .chain(search_output.suggestions.iter())
        .chain(search_output.repo_text_hits.iter())
        .filter_map(|hit| {
            drill_question_search_verification_target(search_output, hit, reason_prefix)
        })
        .collect::<Vec<_>>();
    candidates.sort_by_cached_key(|target| {
        (
            drill_file_rank_for_agent(Some(&target.path)),
            drill_question_target_path_rank(&target.path),
            target.path.clone(),
            target.line,
        )
    });

    let mut seen_paths = HashSet::new();
    candidates
        .into_iter()
        .filter(|target| seen_paths.insert(normalize_drill_path(&target.path)))
        .take(max_files)
        .collect()
}

fn drill_question_search_verification_target(
    search_output: &SearchOutput,
    hit: &SearchHitOutput,
    reason_prefix: &str,
) -> Option<VerificationTargetOutput> {
    let base_target = hit.verification_targets.first();
    let path = base_target
        .map(|target| target.path.clone())
        .or_else(|| hit.file_path.clone())?;
    if path.trim().is_empty() || drill_question_target_is_low_signal(&path) {
        return None;
    }
    if !drill_question_hit_should_be_target(search_output, hit, &path, reason_prefix) {
        return None;
    }
    let line = base_target
        .map(|target| target.line)
        .or(hit.line)
        .unwrap_or(1);
    let node_ref = base_target
        .and_then(|target| target.node_ref.clone())
        .or_else(|| hit.node_ref.clone());
    let query = truncate_utf8_with_suffix(&search_output.query.replace('\n', " "), 96, "...");
    Some(VerificationTargetOutput {
        role: "question_search".to_string(),
        path,
        line,
        node_ref,
        reason: format!(
            "{reason_prefix}: {} matched {} ({})",
            hit.display_name,
            query,
            drill_question_match_quality_label(hit.match_quality)
        ),
    })
}

fn drill_question_hit_should_be_target(
    search_output: &SearchOutput,
    hit: &SearchHitOutput,
    path: &str,
    reason_prefix: &str,
) -> bool {
    if hit.match_quality == SearchMatchQualityDto::RepoText {
        return true;
    }
    if reason_prefix.starts_with("supplemental") {
        return drill_supplemental_hit_matches_query(&search_output.query, hit, path);
    }
    matches!(
        hit.match_quality,
        SearchMatchQualityDto::Exact
            | SearchMatchQualityDto::NormalizedExact
            | SearchMatchQualityDto::Prefix
    )
}

fn drill_supplemental_hit_matches_query(query: &str, hit: &SearchHitOutput, path: &str) -> bool {
    let query = query.trim().to_ascii_lowercase();
    let haystack = format!(
        "{} {} {}",
        hit.display_name,
        path,
        hit.excerpt.as_deref().unwrap_or_default()
    )
    .to_ascii_lowercase();
    if query == "get /" {
        return haystack.contains("get / ") || haystack.contains("get /(");
    }

    drill_question_alnum_tokens(&query)
        .into_iter()
        .any(|token| haystack.contains(&token))
}

fn drill_question_alnum_tokens(value: &str) -> Vec<String> {
    value
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|token| token.len() >= 3)
        .map(ToOwned::to_owned)
        .collect()
}

fn drill_question_target_is_low_signal(path: &str) -> bool {
    let normalized = normalize_drill_path(path);
    normalized.contains("/node_modules/")
        || normalized.ends_with("package-lock.json")
        || normalized.ends_with("pnpm-lock.yaml")
        || normalized.ends_with("yarn.lock")
        || normalized.ends_with("cargo.lock")
}

fn drill_question_target_path_rank(path: &str) -> u8 {
    let normalized = normalize_drill_path(path);
    if drill_path_is_framework_route_or_page(&normalized) {
        0
    } else if normalized.contains("/components/") && !normalized.contains("/components/admin/") {
        1
    } else if normalized.contains("/collections/") {
        2
    } else if normalized.contains("/src/lib/") || normalized.contains("/src/") {
        3
    } else if normalized.contains("/tests/") || normalized.contains(".spec.") {
        8
    } else {
        5
    }
}

fn drill_question_match_quality_label(quality: SearchMatchQualityDto) -> &'static str {
    match quality {
        SearchMatchQualityDto::Exact => "exact",
        SearchMatchQualityDto::NormalizedExact => "normalized_exact",
        SearchMatchQualityDto::Prefix => "prefix",
        SearchMatchQualityDto::Fuzzy => "fuzzy",
        SearchMatchQualityDto::SemanticSuggestion => "semantic_suggestion",
        SearchMatchQualityDto::RepoText => "repo_text",
    }
}

fn run_drill_bridges(
    runtime: &RuntimeContext,
    output_dir: &std::path::Path,
    format: args::OutputFormat,
    anchors: &[DrillAnchorOutput],
    stale_freshness: bool,
    jobs: usize,
) -> Vec<DrillBridgeOutput> {
    let pairs = drill_bridge_pairs(anchors);
    let evidence =
        build_drill_bridge_evidence_in_order(runtime, anchors, &pairs, stale_freshness, jobs);
    let mut bridges = Vec::new();
    for ((from_index, to_index), evidence) in pairs.into_iter().zip(evidence) {
        let command_timer = Instant::now();
        let from = &anchors[from_index];
        let to = &anchors[to_index];
        let markdown = render_drill_bridge_markdown(&evidence);
        let command = with_drill_command_duration(
            command_timer,
            write_drill_artifact(
                output_dir,
                format,
                &format!(
                    "{}-to-{}-bridge",
                    output_slug(&from.anchor),
                    output_slug(&to.anchor)
                ),
                "bridge",
                &evidence,
                markdown,
            ),
        );
        bridges.push(DrillBridgeOutput { evidence, command });
    }
    bridges
}

fn drill_bridge_pairs(anchors: &[DrillAnchorOutput]) -> Vec<(usize, usize)> {
    let mut pairs = Vec::new();
    for from_index in 0..anchors.len() {
        for to_index in from_index.saturating_add(1)..anchors.len() {
            pairs.push((from_index, to_index));
        }
    }
    pairs
}

fn build_drill_bridge_evidence_in_order(
    runtime: &RuntimeContext,
    anchors: &[DrillAnchorOutput],
    pairs: &[(usize, usize)],
    stale_freshness: bool,
    jobs: usize,
) -> Vec<DrillBridgeEvidenceOutput> {
    let neighborhood_file_cache = DrillBridgeNeighborhoodFileCache::default();
    let jobs = jobs.min(pairs.len()).max(1);
    if jobs == 1 || pairs.len() <= 1 {
        return pairs
            .iter()
            .map(|(from_index, to_index)| {
                build_drill_bridge_evidence(
                    runtime,
                    &anchors[*from_index],
                    &anchors[*to_index],
                    stale_freshness,
                    &neighborhood_file_cache,
                )
            })
            .collect();
    }

    let mut evidence_by_pair = vec![None; pairs.len()];
    let chunk_size = pairs.len().div_ceil(jobs);
    std::thread::scope(|scope| {
        let mut handles = Vec::new();
        for (chunk_index, chunk) in pairs.chunks(chunk_size).enumerate() {
            let start_index = chunk_index * chunk_size;
            let neighborhood_file_cache = &neighborhood_file_cache;
            handles.push(scope.spawn(move || {
                chunk
                    .iter()
                    .enumerate()
                    .map(|(offset, (from_index, to_index))| {
                        let evidence = build_drill_bridge_evidence(
                            runtime,
                            &anchors[*from_index],
                            &anchors[*to_index],
                            stale_freshness,
                            neighborhood_file_cache,
                        );
                        (start_index + offset, evidence)
                    })
                    .collect::<Vec<_>>()
            }));
        }

        for handle in handles {
            for (pair_index, evidence) in handle.join().expect("drill bridge worker panicked") {
                evidence_by_pair[pair_index] = Some(evidence);
            }
        }
    });

    evidence_by_pair
        .into_iter()
        .map(|evidence| evidence.expect("drill bridge worker should fill every pair"))
        .collect()
}

fn build_drill_bridge_evidence(
    runtime: &RuntimeContext,
    from: &DrillAnchorOutput,
    to: &DrillAnchorOutput,
    stale_freshness: bool,
    neighborhood_file_cache: &DrillBridgeNeighborhoodFileCache,
) -> DrillBridgeEvidenceOutput {
    let Some(from_node) = from.chosen_anchor.clone() else {
        return unresolved_drill_bridge(from, to, "from anchor was not resolved");
    };
    let Some(to_node) = to.chosen_anchor.clone() else {
        return unresolved_drill_bridge(from, to, "to anchor was not resolved");
    };

    let from_id = NodeId(from_node.node_id.clone());
    let to_id = NodeId(to_node.node_id.clone());
    let forward = runtime
        .browser
        .trail_context(drill_bridge_request(&from_id, &to_id));

    match forward {
        Ok(trail) if drill_bridge_has_graph_path(&trail, &from_id, &to_id) => {
            graph_path_drill_bridge(
                &runtime.project_root,
                from,
                to,
                from_node,
                to_node,
                &trail,
                stale_freshness,
            )
        }
        Ok(trail) => {
            let reverse = runtime
                .browser
                .trail_context(drill_bridge_request(&to_id, &from_id));
            if let Ok(reverse_trail) = reverse
                && drill_bridge_has_graph_path(&reverse_trail, &to_id, &from_id)
            {
                return reverse_graph_path_drill_bridge(
                    &runtime.project_root,
                    from,
                    to,
                    from_node,
                    to_node,
                    &reverse_trail,
                    stale_freshness,
                );
            }

            let shared_files = neighborhood_file_cache.shared_files(runtime, &from_id, &to_id);
            fallback_drill_bridge_with_search_hints(
                runtime,
                &runtime.project_root,
                from,
                to,
                from_node,
                to_node,
                &trail,
                shared_files,
                stale_freshness,
            )
        }
        Err(error) => drill_bridge_error(
            &runtime.project_root,
            from,
            to,
            from_node,
            to_node,
            &error.code,
            &error.message,
            stale_freshness,
        ),
    }
}

#[derive(Default)]
struct DrillBridgeNeighborhoodFileCache {
    files_by_node: Mutex<HashMap<String, HashSet<String>>>,
}

impl DrillBridgeNeighborhoodFileCache {
    fn shared_files(
        &self,
        runtime: &RuntimeContext,
        from_id: &NodeId,
        to_id: &NodeId,
    ) -> Vec<String> {
        let from_files = self.files_for(runtime, from_id);
        let to_files = self.files_for(runtime, to_id);
        drill_bridge_shared_files_from_neighborhoods(&from_files, &to_files)
    }

    fn files_for(&self, runtime: &RuntimeContext, node_id: &NodeId) -> HashSet<String> {
        self.files_for_key(&node_id.0, || {
            drill_bridge_neighborhood_files(runtime, node_id)
        })
    }

    fn files_for_key(
        &self,
        node_key: &str,
        load: impl FnOnce() -> HashSet<String>,
    ) -> HashSet<String> {
        let mut files_by_node = self
            .files_by_node
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(files) = files_by_node.get(node_key) {
            return files.clone();
        }
        let files = load();
        files_by_node.insert(node_key.to_string(), files.clone());
        files
    }
}

fn graph_path_drill_bridge(
    project_root: &std::path::Path,
    from: &DrillAnchorOutput,
    to: &DrillAnchorOutput,
    from_node: SearchHitOutput,
    to_node: SearchHitOutput,
    trail: &TrailContextDto,
    stale_freshness: bool,
) -> DrillBridgeEvidenceOutput {
    let endpoint_files = drill_bridge_endpoint_files(Some(&from_node), Some(&to_node));
    let mut evidence = DrillBridgeEvidenceOutput {
        from_anchor: from.anchor.clone(),
        to_anchor: to.anchor.clone(),
        status: "graph_path".to_string(),
        strategy: "to_target_symbol_forward".to_string(),
        confidence: drill_graph_path_confidence(trail.trail.truncated, "high", "medium"),
        evidence_kind: drill_bridge_evidence_kind_for_trail(trail),
        from_node: Some(from_node),
        to_node: Some(to_node),
        graph_path: Some(drill_bridge_graph_path_output(
            project_root,
            "forward",
            trail,
        )),
        shared_files: Vec::new(),
        endpoint_files,
        evidence_files: Vec::new(),
        next_commands: Vec::new(),
        notes: vec![
            "bridge uses TrailMode::ToTargetSymbol from the earlier anchor to the later anchor"
                .to_string(),
        ],
    };
    evidence.next_commands = drill_bridge_next_commands(project_root, &evidence, stale_freshness);
    evidence
}

fn reverse_graph_path_drill_bridge(
    project_root: &std::path::Path,
    from: &DrillAnchorOutput,
    to: &DrillAnchorOutput,
    from_node: SearchHitOutput,
    to_node: SearchHitOutput,
    trail: &TrailContextDto,
    stale_freshness: bool,
) -> DrillBridgeEvidenceOutput {
    let endpoint_files = drill_bridge_endpoint_files(Some(&from_node), Some(&to_node));
    let mut evidence = DrillBridgeEvidenceOutput {
        from_anchor: from.anchor.clone(),
        to_anchor: to.anchor.clone(),
        status: "reverse_graph_path".to_string(),
        strategy: "to_target_symbol_reverse".to_string(),
        confidence: drill_graph_path_confidence(trail.trail.truncated, "medium", "low"),
        evidence_kind: drill_bridge_evidence_kind_for_trail(trail),
        from_node: Some(from_node),
        to_node: Some(to_node),
        graph_path: Some(drill_bridge_graph_path_output(project_root, "reverse", trail)),
        shared_files: Vec::new(),
        endpoint_files,
        evidence_files: Vec::new(),
        next_commands: Vec::new(),
        notes: vec![
            "no forward graph path was visible; a reverse graph path was found".to_string(),
            "treat reverse paths as relationship evidence, not proof of the requested execution direction"
                .to_string(),
        ],
    };
    evidence.next_commands = drill_bridge_next_commands(project_root, &evidence, stale_freshness);
    evidence
}

#[allow(clippy::too_many_arguments)]
#[cfg(test)]
fn fallback_drill_bridge(
    project_root: &std::path::Path,
    from: &DrillAnchorOutput,
    to: &DrillAnchorOutput,
    from_node: SearchHitOutput,
    to_node: SearchHitOutput,
    trail: &TrailContextDto,
    shared_files: Vec<String>,
    stale_freshness: bool,
) -> DrillBridgeEvidenceOutput {
    let endpoint_files = drill_bridge_endpoint_files(Some(&from_node), Some(&to_node));
    let evidence_files = drill_bridge_evidence_hint_files(from, to);
    fallback_drill_bridge_with_evidence_files(
        project_root,
        from,
        to,
        from_node,
        to_node,
        trail,
        shared_files,
        endpoint_files,
        evidence_files,
        stale_freshness,
    )
}

#[allow(clippy::too_many_arguments)]
fn fallback_drill_bridge_with_search_hints(
    runtime: &RuntimeContext,
    project_root: &std::path::Path,
    from: &DrillAnchorOutput,
    to: &DrillAnchorOutput,
    from_node: SearchHitOutput,
    to_node: SearchHitOutput,
    trail: &TrailContextDto,
    shared_files: Vec<String>,
    stale_freshness: bool,
) -> DrillBridgeEvidenceOutput {
    let endpoint_files = drill_bridge_endpoint_files(Some(&from_node), Some(&to_node));
    let mut evidence_files = drill_bridge_evidence_hint_files(from, to);
    let import_hints = drill_bridge_import_hub_hint_files(runtime, from, to, &from_node, &to_node);
    evidence_files.extend(import_hints.iter().cloned());
    if import_hints.is_empty() {
        evidence_files.extend(drill_bridge_search_hint_files(
            runtime,
            from,
            to,
            &endpoint_files,
        ));
    }
    dedupe_and_rank_drill_files(&mut evidence_files);
    evidence_files.truncate(12);
    fallback_drill_bridge_with_evidence_files(
        project_root,
        from,
        to,
        from_node,
        to_node,
        trail,
        shared_files,
        endpoint_files,
        evidence_files,
        stale_freshness,
    )
}

fn drill_bridge_import_hub_hint_files(
    runtime: &RuntimeContext,
    from: &DrillAnchorOutput,
    to: &DrillAnchorOutput,
    from_node: &SearchHitOutput,
    to_node: &SearchHitOutput,
) -> Vec<String> {
    let mut files = Vec::new();
    if let Some(path) = from_node.file_path.as_deref() {
        files.extend(drill_bridge_import_hub_candidates_from_endpoint(
            runtime, path, &to.anchor,
        ));
    }
    if let Some(path) = to_node.file_path.as_deref() {
        files.extend(drill_bridge_import_hub_candidates_from_endpoint(
            runtime,
            path,
            &from.anchor,
        ));
    }
    dedupe_and_rank_drill_files(&mut files);
    files.truncate(12);
    files
}

fn drill_bridge_import_hub_candidates_from_endpoint(
    runtime: &RuntimeContext,
    endpoint_file: &str,
    opposite_anchor: &str,
) -> Vec<String> {
    let Some(endpoint_path) = drill_relative_source_path(&runtime.project_root, endpoint_file)
    else {
        return Vec::new();
    };
    let Some(source) = drill_read_source_file(&endpoint_path) else {
        return Vec::new();
    };
    let mut files = Vec::new();
    for specifier in drill_js_relative_import_specifiers(&source)
        .into_iter()
        .take(32)
    {
        let Some(candidate) =
            drill_resolve_relative_import(&runtime.project_root, &endpoint_path, &specifier)
        else {
            continue;
        };
        let relative = display::relative_path(&runtime.project_root, &candidate.to_string_lossy());
        if drill_bridge_evidence_file_rank(&relative) >= 9 {
            continue;
        }
        let Some(candidate_source) = drill_read_source_file(&candidate) else {
            continue;
        };
        if candidate_source.contains(opposite_anchor) {
            files.push(relative);
        }
    }
    files
}

fn drill_bridge_search_hint_files(
    runtime: &RuntimeContext,
    from: &DrillAnchorOutput,
    to: &DrillAnchorOutput,
    endpoint_files: &[String],
) -> Vec<String> {
    let Ok(results) = runtime.browser.search_results(SearchRequest {
        query: format!("{} {}", from.anchor, to.anchor),
        repo_text: SearchRepoTextMode::On,
        limit_per_source: 25,
        expand_search_plan: false,
        hybrid_weights: None,
        hybrid_limits: None,
    }) else {
        return Vec::new();
    };
    let mut files = drill_bridge_search_hint_files_from_hits(
        &runtime.project_root,
        endpoint_files,
        &results.repo_text_hits,
        &results.indexed_symbol_hits,
    );
    files.retain(|path| {
        drill_file_contains_terms(&runtime.project_root, path, &[&from.anchor, &to.anchor])
    });
    files
}

fn drill_relative_source_path(
    project_root: &std::path::Path,
    path: &str,
) -> Option<std::path::PathBuf> {
    let path = std::path::Path::new(path);
    if path.is_absolute() || drill_path_has_escape_component(path) {
        return None;
    }
    let root = fs::canonicalize(project_root).ok()?;
    let candidate = fs::canonicalize(project_root.join(path)).ok()?;
    candidate.starts_with(&root).then_some(candidate)
}

fn drill_path_has_escape_component(path: &std::path::Path) -> bool {
    path.components().any(|component| {
        matches!(
            component,
            std::path::Component::ParentDir
                | std::path::Component::RootDir
                | std::path::Component::Prefix(_)
        )
    })
}

fn drill_read_source_file(path: &std::path::Path) -> Option<String> {
    let metadata = fs::metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > 1_000_000 {
        return None;
    }
    fs::read_to_string(path).ok()
}

fn drill_file_contains_terms(project_root: &std::path::Path, path: &str, terms: &[&str]) -> bool {
    let Some(path) = drill_relative_source_path(project_root, path) else {
        return false;
    };
    let Some(source) = drill_read_source_file(&path) else {
        return false;
    };
    terms.iter().all(|term| source.contains(term))
}

fn drill_js_relative_import_specifiers(source: &str) -> Vec<String> {
    let mut specifiers = Vec::new();
    for line in source.lines() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with("import ") {
            continue;
        }
        if let Some(specifier) = drill_quoted_js_specifier(trimmed)
            && specifier.starts_with('.')
        {
            specifiers.push(specifier.to_string());
        }
    }
    specifiers
}

fn drill_quoted_js_specifier(line: &str) -> Option<&str> {
    let from_index = line.find(" from ");
    let search = from_index
        .map(|index| &line[index + " from ".len()..])
        .unwrap_or(line);
    let quote_index = search.find(['\'', '"'])?;
    let quote = search[quote_index..].chars().next()?;
    let rest = &search[quote_index + quote.len_utf8()..];
    let end = rest.find(quote)?;
    Some(&rest[..end])
}

fn drill_resolve_relative_import(
    project_root: &std::path::Path,
    endpoint_path: &std::path::Path,
    specifier: &str,
) -> Option<std::path::PathBuf> {
    let specifier_path = std::path::Path::new(specifier);
    if specifier_path.is_absolute() || !specifier.starts_with('.') {
        return None;
    }
    let root = fs::canonicalize(project_root).ok()?;
    let endpoint = fs::canonicalize(endpoint_path).ok()?;
    if !endpoint.starts_with(&root) {
        return None;
    }
    let base = endpoint.parent()?.join(specifier_path);
    let mut candidates = vec![base.clone()];
    if base.extension().is_none() {
        for extension in ["js", "jsx", "ts", "tsx", "mjs", "cjs"] {
            candidates.push(base.with_extension(extension));
        }
        for extension in ["js", "jsx", "ts", "tsx", "mjs", "cjs"] {
            candidates.push(base.join(format!("index.{extension}")));
        }
    }
    candidates.into_iter().find_map(|candidate| {
        let candidate = fs::canonicalize(candidate).ok()?;
        (candidate.is_file() && candidate.starts_with(&root)).then_some(candidate)
    })
}

#[allow(clippy::too_many_arguments)]
fn fallback_drill_bridge_with_evidence_files(
    project_root: &std::path::Path,
    from: &DrillAnchorOutput,
    to: &DrillAnchorOutput,
    from_node: SearchHitOutput,
    to_node: SearchHitOutput,
    trail: &TrailContextDto,
    shared_files: Vec<String>,
    endpoint_files: Vec<String>,
    evidence_files: Vec<String>,
    stale_freshness: bool,
) -> DrillBridgeEvidenceOutput {
    let classification = drill_fallback_bridge_classification(
        from,
        to,
        &endpoint_files,
        &shared_files,
        &evidence_files,
    );
    let mut notes = vec![
        "no forward TrailMode::ToTargetSymbol graph path was visible between these anchors"
            .to_string(),
    ];
    notes.push(classification.note.clone());
    notes.push(if !shared_files.is_empty() {
        "fallback neighborhood comparison found shared source files but no graph path".to_string()
    } else if !evidence_files.is_empty() {
        "fallback consumer/text evidence found source files to inspect but no graph path or shared file"
            .to_string()
    } else {
        "fallback neighborhood comparison found no shared source files or consumer/text evidence"
            .to_string()
    });
    let mut evidence = DrillBridgeEvidenceOutput {
        from_anchor: from.anchor.clone(),
        to_anchor: to.anchor.clone(),
        status: classification.status,
        strategy: classification.strategy,
        confidence: classification.confidence,
        evidence_kind: classification.evidence_kind,
        from_node: Some(from_node),
        to_node: Some(to_node),
        graph_path: Some(drill_bridge_graph_path_output(
            project_root,
            "forward_no_path",
            trail,
        )),
        shared_files,
        endpoint_files,
        evidence_files,
        next_commands: Vec::new(),
        notes,
    };
    evidence.next_commands = drill_bridge_next_commands(project_root, &evidence, stale_freshness);
    evidence
}

#[derive(Debug, Clone)]
struct DrillFallbackBridgeClassification {
    status: String,
    strategy: String,
    confidence: String,
    evidence_kind: String,
    note: String,
}

fn drill_fallback_bridge_classification(
    from: &DrillAnchorOutput,
    to: &DrillAnchorOutput,
    endpoint_files: &[String],
    shared_files: &[String],
    evidence_files: &[String],
) -> DrillFallbackBridgeClassification {
    if !shared_files.is_empty() {
        return DrillFallbackBridgeClassification {
            status: "graph_shared_file".to_string(),
            strategy: "to_target_symbol_then_graph_shared_files".to_string(),
            confidence: "medium".to_string(),
            evidence_kind: "graph_shared_file".to_string(),
            note: "typed graph neighborhoods found shared source files; this proves shared graph context, not execution direction"
                .to_string(),
        };
    }
    if evidence_files.is_empty() {
        return DrillFallbackBridgeClassification {
            status: "no_bridge_found".to_string(),
            strategy: "to_target_symbol_then_shared_files".to_string(),
            confidence: "low".to_string(),
            evidence_kind: "isolated_anchors".to_string(),
            note: "no bridge, shared-file, or source-truth candidate was found".to_string(),
        };
    }

    let mut files = endpoint_files
        .iter()
        .chain(evidence_files.iter())
        .map(|path| normalize_drill_path(path))
        .collect::<Vec<_>>();
    files.sort();
    files.dedup();
    let anchor_text = format!("{} {}", from.anchor, to.anchor);
    let anchor_tokens = drill_related_query_tokens(&anchor_text);
    if (drill_tokens_contain_all(&anchor_tokens, &["source", "group"]))
        || (drill_tokens_contain_all(&anchor_tokens, &["indexer", "java"]))
        || (drill_tokens_contain_all(&anchor_tokens, &["storage", "access"]))
        || files
            .iter()
            .any(|path| drill_path_is_native_or_jvm_source(path))
    {
        return DrillFallbackBridgeClassification {
            status: "source_truth_only".to_string(),
            strategy: "native_related_source_truth_targets".to_string(),
            confidence: "medium".to_string(),
            evidence_kind: "source_truth_only".to_string(),
            note: "native C++/Java bridge evidence requires source-truth verification because no typed cross-anchor graph path was visible"
                .to_string(),
        };
    }

    let has_payload_collection = files.iter().any(|path| path.contains("/collections/"))
        || [from, to].iter().any(|anchor| {
            anchor
                .chosen_anchor
                .as_ref()
                .and_then(|hit| hit.file_path.as_deref())
                .is_some_and(|path| normalize_drill_path(path).contains("/collections/"))
        });
    let has_payload_usage_surface = files.iter().any(|path| {
        drill_path_is_framework_route_or_page(path)
            || path.contains("/content-data/")
            || path.contains("/lib/comment-auth")
            || path.contains("/lib/social-feed")
    });
    if has_payload_collection && has_payload_usage_surface {
        return DrillFallbackBridgeClassification {
            status: "data_collection_usage".to_string(),
            strategy: "payload_collection_usage_source_targets".to_string(),
            confidence: "medium".to_string(),
            evidence_kind: "data_collection_usage".to_string(),
            note: "Payload collection and runtime usage files were surfaced as bridge candidates; source verification is still required"
                .to_string(),
        };
    }

    if files
        .iter()
        .any(|path| drill_path_is_framework_route_or_page(path))
    {
        return DrillFallbackBridgeClassification {
            status: "framework_route".to_string(),
            strategy: "framework_route_source_targets".to_string(),
            confidence: "medium".to_string(),
            evidence_kind: "framework_route".to_string(),
            note: "framework route/page evidence was surfaced as a bridge candidate; source verification is still required"
                .to_string(),
        };
    }

    if files
        .iter()
        .any(|path| path.contains("/components/") && !path.contains("/components/admin/"))
    {
        return DrillFallbackBridgeClassification {
            status: "component_usage".to_string(),
            strategy: "component_usage_source_targets".to_string(),
            confidence: "medium".to_string(),
            evidence_kind: "component_usage".to_string(),
            note: "component usage evidence was surfaced as a bridge candidate; source verification is still required"
                .to_string(),
        };
    }

    if files
        .iter()
        .any(|path| path.starts_with("crates/codestory-"))
    {
        return DrillFallbackBridgeClassification {
            status: "source_truth_only".to_string(),
            strategy: "codestory_layer_source_truth_targets".to_string(),
            confidence: "medium".to_string(),
            evidence_kind: "source_truth_only".to_string(),
            note: "CodeStory layer bridge evidence requires source-truth verification because the graph did not expose a direct typed path"
                .to_string(),
        };
    }

    DrillFallbackBridgeClassification {
        status: "evidence_hint_only".to_string(),
        strategy: "to_target_symbol_then_consumer_text_hints".to_string(),
        confidence: "low".to_string(),
        evidence_kind: "repo_text_hint".to_string(),
        note: "only generic repo-text or consumer hint evidence was found".to_string(),
    }
}

#[allow(clippy::too_many_arguments)]
fn drill_bridge_error(
    project_root: &std::path::Path,
    from: &DrillAnchorOutput,
    to: &DrillAnchorOutput,
    from_node: SearchHitOutput,
    to_node: SearchHitOutput,
    code: &str,
    message: &str,
    stale_freshness: bool,
) -> DrillBridgeEvidenceOutput {
    let endpoint_files = drill_bridge_endpoint_files(Some(&from_node), Some(&to_node));
    let mut evidence = DrillBridgeEvidenceOutput {
        from_anchor: from.anchor.clone(),
        to_anchor: to.anchor.clone(),
        status: "error".to_string(),
        strategy: "to_target_symbol_forward".to_string(),
        confidence: "low".to_string(),
        evidence_kind: "isolated_anchors".to_string(),
        from_node: Some(from_node),
        to_node: Some(to_node),
        graph_path: None,
        shared_files: Vec::new(),
        endpoint_files,
        evidence_files: Vec::new(),
        next_commands: Vec::new(),
        notes: vec![format!("bridge graph query failed: {code}: {message}")],
    };
    evidence.next_commands = drill_bridge_next_commands(project_root, &evidence, stale_freshness);
    evidence
}

fn drill_graph_path_confidence(truncated: bool, complete: &str, truncated_value: &str) -> String {
    if truncated { truncated_value } else { complete }.to_string()
}

fn drill_bridge_evidence_kind_for_trail(trail: &TrailContextDto) -> String {
    if trail.trail.edges.iter().any(|edge| {
        edge.callsite_identity
            .as_deref()
            .is_some_and(|identity| identity.starts_with("payload:"))
    }) || trail
        .trail
        .nodes
        .iter()
        .any(|node| node.label.contains("payload collection "))
    {
        return "data_collection_usage".to_string();
    }
    if trail.trail.nodes.iter().any(|node| {
        node.label.contains(" route; confidence=")
            || node
                .qualified_name
                .as_deref()
                .is_some_and(|name| name.starts_with("framework::"))
    }) {
        return "framework_route".to_string();
    }
    if trail
        .trail
        .edges
        .iter()
        .any(|edge| edge.kind == codestory_contracts::api::EdgeKind::CALL)
        && trail.trail.nodes.iter().any(|node| {
            matches!(node.kind, NodeKind::FUNCTION | NodeKind::METHOD)
                && node.file_path.as_deref().is_some_and(|path| {
                    let path = path.to_ascii_lowercase();
                    path.ends_with(".tsx") || path.ends_with(".jsx")
                })
                && node
                    .label
                    .chars()
                    .next()
                    .is_some_and(|ch| ch.is_ascii_uppercase())
        })
    {
        return "component_usage".to_string();
    }
    "graph_path".to_string()
}

fn unresolved_drill_bridge(
    from: &DrillAnchorOutput,
    to: &DrillAnchorOutput,
    reason: &str,
) -> DrillBridgeEvidenceOutput {
    let endpoint_files =
        drill_bridge_endpoint_files(from.chosen_anchor.as_ref(), to.chosen_anchor.as_ref());
    DrillBridgeEvidenceOutput {
        from_anchor: from.anchor.clone(),
        to_anchor: to.anchor.clone(),
        status: "unresolved_anchor".to_string(),
        strategy: "not_run".to_string(),
        confidence: "low".to_string(),
        evidence_kind: "isolated_anchors".to_string(),
        from_node: from.chosen_anchor.clone(),
        to_node: to.chosen_anchor.clone(),
        graph_path: None,
        shared_files: Vec::new(),
        endpoint_files,
        evidence_files: Vec::new(),
        next_commands: Vec::new(),
        notes: vec![reason.to_string()],
    }
}

fn drill_bridge_endpoint_files(
    from_node: Option<&SearchHitOutput>,
    to_node: Option<&SearchHitOutput>,
) -> Vec<String> {
    let mut files = Vec::new();
    if let Some(path) = from_node.and_then(|node| node.file_path.clone()) {
        files.push(path);
    }
    if let Some(path) = to_node.and_then(|node| node.file_path.clone()) {
        files.push(path);
    }
    dedupe_and_rank_drill_files(&mut files);
    files
}

fn drill_bridge_next_commands(
    project_root: &std::path::Path,
    evidence: &DrillBridgeEvidenceOutput,
    stale_freshness: bool,
) -> Vec<String> {
    let project = quote_command_path(project_root);
    let refresh = if stale_freshness {
        "incremental"
    } else {
        "none"
    };
    let mut commands = Vec::new();
    let bridge_query =
        quote_command_value(&format!("{} {}", evidence.from_anchor, evidence.to_anchor));
    commands.push(format!(
        "codestory-cli search --project {project} --query {bridge_query} --refresh {refresh} --why"
    ));
    for node in [evidence.from_node.as_ref(), evidence.to_node.as_ref()]
        .into_iter()
        .flatten()
    {
        let id = quote_command_value(&node.node_id);
        commands.push(format!(
            "codestory-cli trail --project {project} --id {id} --refresh {refresh} --story --hide-speculative"
        ));
        commands.push(format!(
            "codestory-cli snippet --project {project} --id {id} --function-body --context 40 --refresh {refresh}"
        ));
    }

    let mut files = evidence
        .evidence_files
        .iter()
        .chain(evidence.shared_files.iter())
        .chain(evidence.endpoint_files.iter())
        .cloned()
        .collect::<Vec<_>>();
    dedupe_and_rank_drill_files(&mut files);
    for file in files.into_iter().take(5) {
        let query = quote_command_value(&file);
        commands.push(format!(
            "codestory-cli search --project {project} --query {query} --refresh {refresh} --why"
        ));
    }

    let mut seen = HashSet::new();
    commands
        .into_iter()
        .filter(|command| seen.insert(command.clone()))
        .collect()
}

fn drill_bridge_evidence_hint_files(
    from: &DrillAnchorOutput,
    to: &DrillAnchorOutput,
) -> Vec<String> {
    let mut files = Vec::new();
    push_anchor_evidence_hint_files(&mut files, from);
    push_anchor_evidence_hint_files(&mut files, to);
    let mut seen = HashSet::new();
    files.retain(|path| seen.insert(path.clone()));
    rank_drill_bridge_evidence_files(&mut files);
    files.truncate(12);
    files
}

fn drill_bridge_search_hint_files_from_hits(
    project_root: &std::path::Path,
    endpoint_files: &[String],
    repo_text_hits: &[SearchHit],
    indexed_symbol_hits: &[SearchHit],
) -> Vec<String> {
    let endpoint_keys = endpoint_files
        .iter()
        .map(|path| normalize_drill_path(path))
        .collect::<HashSet<_>>();
    let mut seen = HashSet::new();
    let mut files = Vec::new();
    for hit in repo_text_hits.iter().chain(indexed_symbol_hits.iter()) {
        let Some(path) = hit.file_path.as_deref() else {
            continue;
        };
        let path = display::relative_path(project_root, path);
        let key = normalize_drill_path(&path);
        if endpoint_keys.contains(&key)
            || drill_question_target_is_low_signal(&path)
            || drill_bridge_evidence_file_rank(&path) >= 9
        {
            continue;
        }
        if seen.insert(key) {
            files.push(path);
        }
    }
    rank_drill_bridge_evidence_files(&mut files);
    files.truncate(12);
    files
}

fn dedupe_and_rank_drill_files(files: &mut Vec<String>) {
    let mut seen = HashSet::new();
    files.retain(|path| seen.insert(path.clone()));
    rank_drill_bridge_evidence_files(files);
}

fn rank_drill_bridge_evidence_files(files: &mut Vec<String>) {
    let mut ranked = std::mem::take(files)
        .into_iter()
        .enumerate()
        .collect::<Vec<_>>();
    ranked.sort_by_key(|(index, path)| {
        (
            drill_bridge_evidence_file_rank(path),
            drill_bridge_evidence_file_subrank(path),
            *index,
        )
    });
    files.extend(ranked.into_iter().map(|(_, path)| path));
}

fn drill_bridge_evidence_file_rank(path: &str) -> u8 {
    let normalized = path.replace('\\', "/").to_ascii_lowercase();
    let normalized_with_root = format!("/{normalized}");
    if drill_bridge_evidence_is_generated_path(&normalized_with_root) {
        return 9;
    }
    if normalized.contains("/tests/")
        || normalized.starts_with("tests/")
        || normalized.contains("/test/")
        || normalized.contains(".test.")
        || normalized.contains(".spec.")
        || normalized.contains("/__tests__/")
    {
        return 7;
    }
    if normalized.contains("/benches/") || normalized.starts_with("benches/") {
        return 8;
    }
    if normalized.starts_with("scripts/")
        || normalized.contains("/scripts/")
        || normalized.contains("migration")
        || normalized.contains("migrate")
        || normalized.contains("import-")
    {
        return 10;
    }
    if drill_bridge_evidence_is_admin_path(&normalized_with_root) {
        return 6;
    }
    if normalized.starts_with("crates/") && normalized.contains("/src/") {
        return 0;
    }
    if normalized.starts_with("src/app/") || normalized.contains("/src/app/") {
        return 0;
    }
    if normalized.starts_with("src/pages/") || normalized.contains("/src/pages/") {
        return 0;
    }
    if normalized.starts_with("src/routes/") || normalized.contains("/src/routes/") {
        return 0;
    }
    if normalized.ends_with("/route.ts")
        || normalized.ends_with("/route.tsx")
        || normalized.ends_with("/page.ts")
        || normalized.ends_with("/page.tsx")
    {
        return 0;
    }
    if normalized.starts_with("src/components/") || normalized.contains("/src/components/") {
        return 1;
    }
    if normalized.starts_with("src/lib/content-data/")
        || normalized.contains("/src/lib/content-data/")
        || normalized.starts_with("src/lib/comment-auth")
        || normalized.contains("/src/lib/comment-auth")
        || normalized.starts_with("src/lib/comments")
        || normalized.contains("/src/lib/comments")
        || normalized.starts_with("src/lib/elsewhere")
        || normalized.contains("/src/lib/elsewhere")
        || normalized.starts_with("src/lib/social-feed")
        || normalized.contains("/src/lib/social-feed")
    {
        return 2;
    }
    if normalized.starts_with("src/collections/") || normalized.contains("/src/collections/") {
        return 3;
    }
    if normalized.starts_with("src/")
        || (normalized.contains("/src/")
            && !normalized.contains("/test")
            && !normalized.contains("/__tests__/"))
    {
        return 4;
    }
    5
}

fn drill_bridge_evidence_file_subrank(path: &str) -> u8 {
    let normalized = path.replace('\\', "/").to_ascii_lowercase();
    if normalized.ends_with("/page.ts") || normalized.ends_with("/page.tsx") {
        return 0;
    }
    if normalized.ends_with("/route.ts") || normalized.ends_with("/route.tsx") {
        return 1;
    }
    if normalized.contains("/rootruntimehome") {
        return 2;
    }
    if normalized.contains("/content-data/") {
        return 3;
    }
    if normalized.contains("/comment-auth") || normalized.contains("/comments") {
        return 4;
    }
    if normalized.contains("/elsewhere") || normalized.contains("/social-feed") {
        return 5;
    }
    if normalized.contains("/collections/") {
        return 6;
    }
    7
}

fn drill_bridge_evidence_is_admin_path(normalized_with_root: &str) -> bool {
    normalized_with_root.contains("/admin/")
        || normalized_with_root.contains("/components/admin")
        || normalized_with_root.contains("/app/(payload)")
        || normalized_with_root.contains("/payload-admin")
}

fn drill_bridge_evidence_is_generated_path(normalized_with_root: &str) -> bool {
    normalized_with_root.contains("/generated/")
        || normalized_with_root.contains("payload-types")
        || normalized_with_root.contains("/target/")
        || normalized_with_root.contains("/dist/")
        || normalized_with_root.contains("/build/")
}

fn push_anchor_evidence_hint_files(files: &mut Vec<String>, anchor: &DrillAnchorOutput) {
    let Some(summary) = anchor.consumer_summary.as_ref() else {
        return;
    };
    for caller in summary.callers.iter().take(3) {
        if let Some(path) = caller.file_path.as_ref() {
            files.push(path.clone());
        }
    }
    for consumer in summary.consumers.iter().take(3) {
        if let Some(path) = consumer.file_path.as_ref() {
            files.push(path.clone());
        }
        if let Some(path) = consumer.target_file_path.as_ref() {
            files.push(path.clone());
        }
    }
    for hint in summary.text_consumer_hints.iter().take(3) {
        if let Some(path) = hint.file_path.as_ref() {
            files.push(path.clone());
        }
    }
}

fn drill_file_rank_for_agent(path: Option<&str>) -> u8 {
    path.map(drill_bridge_evidence_file_rank).unwrap_or(9)
}

fn drill_bridge_request(root_id: &NodeId, target_id: &NodeId) -> TrailConfigDto {
    TrailConfigDto {
        root_id: root_id.clone(),
        mode: TrailMode::ToTargetSymbol,
        target_id: Some(target_id.clone()),
        depth: 0,
        direction: TrailDirection::Outgoing,
        caller_scope: TrailCallerScope::ProductionOnly,
        edge_filter: Vec::new(),
        show_utility_calls: false,
        hide_speculative: true,
        story: true,
        node_filter: Vec::new(),
        max_nodes: 160,
        layout_direction: codestory_contracts::api::LayoutDirection::Horizontal,
    }
}

fn drill_bridge_has_graph_path(trail: &TrailContextDto, from_id: &NodeId, to_id: &NodeId) -> bool {
    if from_id == to_id {
        return true;
    }
    let has_from = trail.trail.nodes.iter().any(|node| node.id == *from_id);
    let has_to = trail.trail.nodes.iter().any(|node| node.id == *to_id);
    has_from && has_to && !trail.trail.edges.is_empty()
}

fn drill_bridge_graph_path_output(
    project_root: &std::path::Path,
    mode: &str,
    trail: &TrailContextDto,
) -> DrillBridgeGraphPathOutput {
    let labels = trail
        .trail
        .nodes
        .iter()
        .map(|node| (node.id.0.clone(), node.label.clone()))
        .collect::<HashMap<_, _>>();
    let nodes = trail
        .trail
        .nodes
        .iter()
        .map(|node| {
            let path = node
                .file_path
                .as_deref()
                .map(|path| display::relative_path(project_root, path))
                .unwrap_or_else(|| "<no-file>".to_string());
            format!(
                "{} [{:?}] {} depth={}",
                node.label, node.kind, path, node.depth
            )
        })
        .collect();
    let edges = trail
        .trail
        .edges
        .iter()
        .map(|edge| {
            let source = labels
                .get(&edge.source.0)
                .cloned()
                .unwrap_or_else(|| edge.source.0.clone());
            let target = labels
                .get(&edge.target.0)
                .cloned()
                .unwrap_or_else(|| edge.target.0.clone());
            let certainty = edge.certainty.as_deref().unwrap_or("unknown");
            format!("{source} -{:?}/{certainty}-> {target}", edge.kind)
        })
        .collect();
    let edge_count = trail.trail.edges.len();
    let omitted_edge_count = trail.trail.omitted_edge_count;
    let no_path_without_omissions =
        mode.ends_with("_no_path") && edge_count == 0 && omitted_edge_count == 0;
    DrillBridgeGraphPathOutput {
        mode: mode.to_string(),
        node_count: trail.trail.nodes.len(),
        edge_count,
        truncated: trail.trail.truncated && !no_path_without_omissions,
        omitted_edge_count,
        nodes,
        edges,
    }
}

fn drill_bridge_shared_files_from_neighborhoods(
    from_files: &HashSet<String>,
    to_files: &HashSet<String>,
) -> Vec<String> {
    let mut shared = from_files
        .intersection(to_files)
        .cloned()
        .collect::<Vec<_>>();
    shared.sort();
    shared.truncate(12);
    shared
}

fn drill_bridge_neighborhood_files(runtime: &RuntimeContext, node_id: &NodeId) -> HashSet<String> {
    let Ok(trail) = runtime.browser.trail_context(TrailConfigDto {
        root_id: node_id.clone(),
        mode: TrailMode::Neighborhood,
        target_id: None,
        depth: 1,
        direction: TrailDirection::Both,
        caller_scope: TrailCallerScope::ProductionOnly,
        edge_filter: Vec::new(),
        show_utility_calls: false,
        hide_speculative: true,
        story: false,
        node_filter: Vec::new(),
        max_nodes: 80,
        layout_direction: codestory_contracts::api::LayoutDirection::Horizontal,
    }) else {
        return HashSet::new();
    };
    trail
        .trail
        .nodes
        .into_iter()
        .filter_map(|node| node.file_path)
        .map(|path| display::relative_path(&runtime.project_root, &path))
        .collect()
}

fn render_drill_bridge_markdown(evidence: &DrillBridgeEvidenceOutput) -> String {
    let mut markdown = String::new();
    let _ = writeln!(markdown, "# Bridge");
    let _ = writeln!(
        markdown,
        "from: `{}` to: `{}`",
        evidence.from_anchor, evidence.to_anchor
    );
    let _ = writeln!(
        markdown,
        "status: {} strategy: {} confidence: {} evidence_kind: {}",
        evidence.status, evidence.strategy, evidence.confidence, evidence.evidence_kind
    );
    if let Some(from_node) = evidence.from_node.as_ref() {
        let _ = writeln!(
            markdown,
            "from_node: {}",
            render_search_hit_output(from_node)
        );
    }
    if let Some(to_node) = evidence.to_node.as_ref() {
        let _ = writeln!(markdown, "to_node: {}", render_search_hit_output(to_node));
    }
    if let Some(path) = evidence.graph_path.as_ref() {
        let _ = writeln!(
            markdown,
            "graph_path: mode={} nodes={} edges={} truncated={} omitted_edges={}",
            path.mode, path.node_count, path.edge_count, path.truncated, path.omitted_edge_count
        );
        if !path.nodes.is_empty() {
            let _ = writeln!(markdown, "path_nodes:");
            for node in &path.nodes {
                let _ = writeln!(markdown, "- {node}");
            }
        }
        if !path.edges.is_empty() {
            let _ = writeln!(markdown, "path_edges:");
            for edge in &path.edges {
                let _ = writeln!(markdown, "- {edge}");
            }
        }
    }
    if !evidence.shared_files.is_empty() {
        let _ = writeln!(markdown, "shared_files:");
        for file in &evidence.shared_files {
            let _ = writeln!(markdown, "- `{file}`");
        }
    }
    if !evidence.endpoint_files.is_empty() {
        let _ = writeln!(markdown, "endpoint_files:");
        for file in &evidence.endpoint_files {
            let _ = writeln!(markdown, "- `{file}`");
        }
    }
    if !evidence.evidence_files.is_empty() {
        let _ = writeln!(markdown, "evidence_files:");
        for file in &evidence.evidence_files {
            let _ = writeln!(markdown, "- `{file}`");
        }
    }
    if !evidence.notes.is_empty() {
        let _ = writeln!(markdown, "notes:");
        for note in &evidence.notes {
            let _ = writeln!(markdown, "- {note}");
        }
    }
    if !evidence.next_commands.is_empty() {
        let _ = writeln!(markdown, "next_commands:");
        for command in &evidence.next_commands {
            let _ = writeln!(markdown, "- `{command}`");
        }
    }
    markdown
}

fn drill_verification_checklist() -> Vec<DrillVerificationChecklistItemOutput> {
    let allowed = ["correct", "partial", "misleading", "unsupported"]
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>();
    [
        "Record the CodeStory-only architecture answer before opening source files.",
        "Open only files named or implied by CodeStory evidence for source-truth verification.",
        "Classify each major claim as correct, partial, misleading, or unsupported.",
        "Record what changed after source reads; revisions are product findings, not cleanup.",
    ]
    .into_iter()
    .map(|item| DrillVerificationChecklistItemOutput {
        item: item.to_string(),
        allowed_classifications: allowed.clone(),
    })
    .collect()
}

fn drill_answer_quality_contract() -> DrillAnswerQualityContractOutput {
    DrillAnswerQualityContractOutput {
        code_story_only_draft_required: true,
        source_truth_verification_required: true,
        pass_condition: "The source-verified answer must not materially revise the CodeStory-only architecture claims; material revisions are product findings."
            .to_string(),
        score_inputs: [
            "anchor_recall: requested anchors with typed or resolvable hits",
            "evidence_command_success: search, symbol, trail, explore, and snippet artifacts written successfully",
            "verification_target_count: source files and lines named by CodeStory evidence",
            "source_packet_coverage: files, related files, and truncation notes emitted by explore",
            "source_correction_count: architecture claims changed after source reads",
            "unsupported_claim_count: claims not backed by CodeStory evidence",
        ]
        .into_iter()
        .map(str::to_string)
        .collect(),
        correction_buckets: ["correct", "partial", "misleading", "unsupported"]
            .into_iter()
            .map(str::to_string)
            .collect(),
    }
}

fn drill_claim_ledger_template(
    anchors: &[DrillAnchorOutput],
    bridges: &[DrillBridgeOutput],
) -> DrillClaimLedgerOutput {
    let mut claims = Vec::new();
    for (index, anchor) in anchors.iter().enumerate() {
        let expected_evidence = anchor
            .commands
            .iter()
            .filter_map(|command| command.artifact.clone())
            .collect::<Vec<_>>();
        let mut source_truth_files = unique_verification_target_paths(&anchor.verification_targets);
        dedupe_and_rank_drill_files(&mut source_truth_files);
        claims.push(DrillClaimLedgerEntryOutput {
            id: format!("anchor-{}", index + 1),
            claim: format!(
                "Candidate architecture claim involving `{}` requires source-truth verification before the final answer.",
                anchor.anchor
            ),
            expected_evidence,
            source_truth_files,
            pre_verification_confidence: if anchor.chosen_anchor.is_some() {
                "medium"
            } else {
                "low"
            }
            .to_string(),
            classification: None,
            changed_after_source_read: None,
            correction_note: None,
        });
    }
    for (index, bridge) in bridges.iter().enumerate() {
        let evidence = &bridge.evidence;
        let mut source_truth_files = evidence.shared_files.clone();
        source_truth_files.extend(evidence.endpoint_files.iter().cloned());
        source_truth_files.extend(evidence.evidence_files.iter().cloned());
        if let Some(from_node) = evidence.from_node.as_ref()
            && let Some(path) = from_node.file_path.as_ref()
        {
            source_truth_files.push(path.clone());
        }
        if let Some(to_node) = evidence.to_node.as_ref()
            && let Some(path) = to_node.file_path.as_ref()
        {
            source_truth_files.push(path.clone());
        }
        dedupe_and_rank_drill_files(&mut source_truth_files);
        let expected_evidence = bridge
            .command
            .artifact
            .clone()
            .into_iter()
            .collect::<Vec<_>>();
        claims.push(DrillClaimLedgerEntryOutput {
            id: format!("bridge-{}", index + 1),
            claim: format!(
                "Candidate bridge claim involving `{}` to `{}` currently has status `{}` using `{}` and requires source-truth verification before the final answer.",
                evidence.from_anchor, evidence.to_anchor, evidence.status, evidence.strategy
            ),
            expected_evidence,
            source_truth_files,
            pre_verification_confidence: evidence.confidence.clone(),
            classification: None,
            changed_after_source_read: None,
            correction_note: None,
        });
    }

    let pending_claim_count = claims.len() as u32;
    DrillClaimLedgerOutput {
        template_version: 1,
        instructions: vec![
            "Fill classification after source-truth verification: correct, partial, misleading, or unsupported."
                .to_string(),
            "Set changed_after_source_read=true when source reads materially revise the CodeStory-only claim."
                .to_string(),
            "Treat no_bridge_found as useful negative evidence, not as a silent failure.".to_string(),
        ],
        claims,
        scoring: DrillClaimLedgerScoringOutput {
            status: "pending_source_verification".to_string(),
            pending_claim_count,
            correct: 0,
            partial: 0,
            misleading: 0,
            unsupported: 0,
            material_revision_count: 0,
            score_formula:
                "quality_score=(correct + 0.5*partial) / max(1,total_claims); material revisions are reported separately"
                    .to_string(),
        },
    }
}

fn unique_verification_target_paths(targets: &[VerificationTargetOutput]) -> Vec<String> {
    let mut paths = targets
        .iter()
        .map(|target| target.path.clone())
        .collect::<Vec<_>>();
    paths.sort();
    paths.dedup();
    paths
}

fn write_drill_artifact<T: serde::Serialize>(
    output_dir: &std::path::Path,
    format: args::OutputFormat,
    stem: &str,
    command: &str,
    output: &T,
    markdown: String,
) -> DrillCommandStatusOutput {
    let ext = match format {
        args::OutputFormat::Markdown => "md",
        args::OutputFormat::Json => "json",
        args::OutputFormat::Dot => "txt",
    };
    let path = output_dir.join(format!("{stem}.{ext}"));
    let content = match format {
        args::OutputFormat::Markdown => markdown,
        args::OutputFormat::Json => match serde_json::to_string_pretty(output) {
            Ok(value) => value,
            Err(error) => return drill_status_error(command, error),
        },
        args::OutputFormat::Dot => unreachable!("dot was rejected by run_drill"),
    };
    match fs::write(&path, ensure_trailing_newline(content)) {
        Ok(()) => DrillCommandStatusOutput {
            command: command.to_string(),
            status: "ok".to_string(),
            duration_ms: 0,
            artifact: Some(display::clean_path_string(&path.to_string_lossy())),
            error: None,
        },
        Err(error) => drill_status_error(command, error),
    }
}

fn drill_status_error(command: &str, error: impl std::fmt::Debug) -> DrillCommandStatusOutput {
    DrillCommandStatusOutput {
        command: command.to_string(),
        status: "error".to_string(),
        duration_ms: 0,
        artifact: None,
        error: Some(format!("{error:?}")),
    }
}

fn ensure_trailing_newline(mut content: String) -> String {
    if !content.ends_with('\n') {
        content.push('\n');
    }
    content
}

#[derive(Debug, Clone)]
struct DrillConsumerTarget {
    node_id: NodeId,
    relation: String,
    query: Option<String>,
    preferred_file_path: Option<String>,
}

#[derive(Default)]
struct DrillConsumerSummaryAccumulator {
    inspected_any_target: bool,
    truncated: bool,
    omitted_edge_count: u32,
    callers: Vec<DrillAnchorConsumerOutput>,
    consumers: Vec<DrillAnchorConsumerOutput>,
    seen_consumers: HashSet<String>,
    seen_callers: HashSet<String>,
    notes: Vec<String>,
}

fn drill_anchor_consumer_summary(
    runtime: &RuntimeContext,
    anchor: &str,
    hit: &SearchHit,
    verification_targets: &[VerificationTargetOutput],
) -> Option<DrillAnchorConsumerSummaryOutput> {
    let selected_target = DrillConsumerTarget {
        node_id: hit.node_id.clone(),
        relation: "selected_anchor".to_string(),
        query: None,
        preferred_file_path: None,
    };

    let mut acc = DrillConsumerSummaryAccumulator::default();
    drill_inspect_consumer_target(runtime, selected_target, &mut acc);
    if acc.consumers.is_empty() {
        for target in drill_related_consumer_targets(runtime, anchor, hit, verification_targets) {
            drill_inspect_consumer_target(runtime, target, &mut acc);
        }
    }

    if !acc.inspected_any_target {
        return None;
    }

    let caller_count = acc.callers.len();
    let consumer_count = acc.consumers.len();
    acc.callers.sort_by_cached_key(|consumer| {
        (
            drill_file_rank_for_agent(consumer.file_path.as_deref()),
            consumer.file_path.clone().unwrap_or_default(),
            consumer.name.clone(),
        )
    });
    acc.consumers.sort_by_cached_key(|consumer| {
        (
            drill_file_rank_for_agent(consumer.file_path.as_deref()),
            drill_file_rank_for_agent(consumer.target_file_path.as_deref()),
            consumer.target_relation.clone().unwrap_or_default(),
            consumer.file_path.clone().unwrap_or_default(),
            format!("{:?}", consumer.edge_kind),
            consumer.name.clone(),
        )
    });
    acc.callers.truncate(10);
    acc.consumers.truncate(12);

    let mut text_consumer_hints = if consumer_count == 0 {
        drill_anchor_text_consumer_hints(runtime, anchor, hit)
    } else {
        Vec::new()
    };
    let text_hint_count = text_consumer_hints.len();
    text_consumer_hints.sort_by(|left, right| {
        drill_file_rank_for_agent(left.file_path.as_deref())
            .cmp(&drill_file_rank_for_agent(right.file_path.as_deref()))
            .then_with(|| {
                right
                    .score
                    .partial_cmp(&left.score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| {
                left.file_path
                    .as_deref()
                    .unwrap_or_default()
                    .cmp(right.file_path.as_deref().unwrap_or_default())
            })
            .then_with(|| left.name.cmp(&right.name))
    });
    text_consumer_hints.truncate(12);

    if caller_count == 0 {
        acc.notes.push(
            "no visible production callers were found in the incoming trail; runtime participation still needs source-truth verification"
                .to_string(),
        );
    }
    if consumer_count == 0 {
        acc.notes.push(
            "no visible production graph consumers were found in the bounded incoming trail"
                .to_string(),
        );
        if text_hint_count > 0 {
            acc.notes.push(format!(
                "repo-text found {text_hint_count} consumer hints; treat them as source-truth pointers, not typed graph edges"
            ));
        }
    }
    if acc.truncated || acc.omitted_edge_count > 0 {
        acc.notes.push(format!(
            "consumer summary is bounded: truncated={} omitted_edges={}",
            acc.truncated, acc.omitted_edge_count
        ));
    }

    Some(DrillAnchorConsumerSummaryOutput {
        caller_count,
        consumer_count,
        truncated: acc.truncated,
        omitted_edge_count: acc.omitted_edge_count,
        callers: acc.callers,
        consumers: acc.consumers,
        text_hint_count,
        text_consumer_hints,
        notes: acc.notes,
    })
}

fn drill_inspect_consumer_target(
    runtime: &RuntimeContext,
    target: DrillConsumerTarget,
    acc: &mut DrillConsumerSummaryAccumulator,
) {
    let Ok(references) =
        runtime
            .browser
            .direct_references_graph(DrillAnchorConsumerSummaryOutput::trail_request(
                &target.node_id,
            ))
    else {
        acc.notes.push(format!(
            "could not inspect consumer target `{}`{}",
            target.relation,
            target
                .query
                .as_ref()
                .map(|query| format!(" from query `{query}`"))
                .unwrap_or_default()
        ));
        return;
    };
    acc.inspected_any_target = true;
    acc.truncated |= references.truncated;
    acc.omitted_edge_count = acc
        .omitted_edge_count
        .saturating_add(references.omitted_edge_count);

    let nodes_by_id = references
        .nodes
        .iter()
        .map(|node| (node.id.0.clone(), node))
        .collect::<HashMap<_, _>>();
    let (target_name, target_kind, mut target_file_path) = nodes_by_id
        .get(&target.node_id.0)
        .map(|node| {
            (
                node.label.clone(),
                Some(node.kind),
                node.file_path
                    .as_deref()
                    .map(|path| display::relative_path(&runtime.project_root, path)),
            )
        })
        .unwrap_or_else(|| (target.relation.clone(), None, None));
    if target_file_path
        .as_ref()
        .is_none_or(|path| drill_file_rank_for_agent(Some(path)) > 0)
        && let Some(path) = target.preferred_file_path.clone()
    {
        target_file_path = Some(path);
    }

    if target.relation != "selected_anchor" {
        let query_note = target
            .query
            .as_ref()
            .map(|query| format!(" from query `{query}`"))
            .unwrap_or_default();
        acc.notes.push(format!(
            "included related consumer target `{target_name}` via `{}`{query_note}",
            target.relation
        ));
    }

    for edge in &references.edges {
        if edge.target != target.node_id || edge.source == target.node_id {
            continue;
        }
        let Some(source) = nodes_by_id.get(&edge.source.0) else {
            continue;
        };
        let consumer = DrillAnchorConsumerOutput {
            name: source.label.clone(),
            kind: source.kind,
            file_path: source
                .file_path
                .as_deref()
                .map(|path| display::relative_path(&runtime.project_root, path)),
            qualified_name: source.qualified_name.clone(),
            target_name: Some(target_name.clone()),
            target_kind,
            target_file_path: target_file_path.clone(),
            target_relation: Some(target.relation.clone()),
            edge_kind: edge.kind,
            confidence: edge.confidence,
            certainty: edge.certainty.clone(),
        };
        let consumer_key = format!("{}:{}:{:?}", edge.source.0, target.node_id.0, edge.kind);
        if acc.seen_consumers.insert(consumer_key) {
            acc.consumers.push(consumer.clone());
        }
        let caller_key = format!("{}:{}", edge.source.0, target.node_id.0);
        if edge.kind == codestory_contracts::api::EdgeKind::CALL
            && acc.seen_callers.insert(caller_key)
        {
            acc.callers.push(consumer);
        }
    }
}

fn drill_anchor_text_consumer_hints(
    runtime: &RuntimeContext,
    anchor: &str,
    hit: &SearchHit,
) -> Vec<DrillAnchorTextConsumerHintOutput> {
    let Ok(results) = runtime.browser.search_results(SearchRequest {
        query: anchor.to_string(),
        repo_text: SearchRepoTextMode::On,
        limit_per_source: 25,
        expand_search_plan: false,
        hybrid_weights: None,
        hybrid_limits: None,
    }) else {
        return Vec::new();
    };

    let selected_path = hit
        .file_path
        .as_deref()
        .map(|path| display::relative_path(&runtime.project_root, path));
    let mut seen = HashSet::new();
    let mut hints = Vec::new();
    for text_hit in results.repo_text_hits {
        let file_path = text_hit
            .file_path
            .as_deref()
            .map(|path| display::relative_path(&runtime.project_root, path));
        if selected_path.as_ref().is_some_and(|selected| {
            file_path
                .as_ref()
                .is_some_and(|candidate| candidate == selected)
        }) {
            continue;
        }
        let key = format!(
            "{}:{}",
            file_path.as_deref().unwrap_or(&text_hit.display_name),
            text_hit.line.unwrap_or_default()
        );
        if !seen.insert(key) {
            continue;
        }
        hints.push(DrillAnchorTextConsumerHintOutput {
            name: text_hit.display_name,
            kind: text_hit.kind,
            file_path,
            line: text_hit.line,
            score: text_hit.score,
        });
    }
    hints
}

impl DrillAnchorConsumerSummaryOutput {
    fn trail_request(node_id: &NodeId) -> TrailConfigDto {
        TrailConfigDto {
            root_id: node_id.clone(),
            mode: TrailMode::AllReferencing,
            target_id: None,
            depth: 0,
            direction: TrailDirection::Incoming,
            caller_scope: TrailCallerScope::ProductionOnly,
            edge_filter: Vec::new(),
            show_utility_calls: false,
            hide_speculative: true,
            story: false,
            node_filter: Vec::new(),
            max_nodes: 120,
            layout_direction: codestory_contracts::api::LayoutDirection::Horizontal,
        }
    }
}

fn drill_related_consumer_targets(
    runtime: &RuntimeContext,
    anchor: &str,
    hit: &SearchHit,
    verification_targets: &[VerificationTargetOutput],
) -> Vec<DrillConsumerTarget> {
    let mut related = Vec::new();
    let mut seen_nodes = HashSet::new();
    let mut slug_candidates =
        drill_payload_collection_slug_candidates(anchor, hit, verification_targets);
    slug_candidates.truncate(4);

    for slug in slug_candidates {
        for query in [
            format!("payload:collection:{slug}"),
            format!("payload collection {slug}"),
        ] {
            let Ok(results) = runtime.browser.search_results(SearchRequest {
                query: query.clone(),
                repo_text: SearchRepoTextMode::Off,
                limit_per_source: 25,
                expand_search_plan: true,
                hybrid_weights: None,
                hybrid_limits: None,
            }) else {
                continue;
            };
            let Some(payload_hit) = results.indexed_symbol_hits.iter().find(|candidate| {
                candidate.resolvable
                    && candidate.node_id != hit.node_id
                    && drill_is_payload_collection_hit(candidate, &slug)
            }) else {
                continue;
            };
            if seen_nodes.insert(payload_hit.node_id.0.clone()) {
                related.push(DrillConsumerTarget {
                    node_id: payload_hit.node_id.clone(),
                    relation: format!("related_payload_collection:{slug}"),
                    query: Some(query),
                    preferred_file_path: drill_preferred_payload_collection_source_file(
                        verification_targets,
                        &slug,
                    ),
                });
            }
            break;
        }
    }

    for (relation, query) in drill_native_related_queries(anchor, hit) {
        let Ok(results) = runtime.browser.search_results(SearchRequest {
            query: query.clone(),
            repo_text: SearchRepoTextMode::Off,
            limit_per_source: 25,
            expand_search_plan: false,
            hybrid_weights: None,
            hybrid_limits: None,
        }) else {
            continue;
        };
        let Some(native_hit) = results.indexed_symbol_hits.iter().find(|candidate| {
            candidate.resolvable
                && candidate.node_id != hit.node_id
                && drill_native_related_query_matches(candidate, &query)
        }) else {
            continue;
        };
        if seen_nodes.insert(native_hit.node_id.0.clone()) {
            related.push(DrillConsumerTarget {
                node_id: native_hit.node_id.clone(),
                relation,
                query: Some(query),
                preferred_file_path: None,
            });
        }
    }

    related
}

fn drill_native_related_queries(anchor: &str, hit: &SearchHit) -> Vec<(String, String)> {
    let mut queries = Vec::new();
    let mut seen = HashSet::new();
    let text = format!("{} {}", anchor, hit.display_name);
    let tokens = drill_related_query_tokens(&text);
    if drill_tokens_contain_all(&tokens, &["source", "group"])
        && drill_tokens_contain_any(&tokens, &["cxx", "cdb", "compile", "database"])
    {
        for (relation, query) in [
            (
                "related_native_role:indexing",
                "source group prepare indexing",
            ),
            (
                "related_native_role:command_provider",
                "source group indexer command provider",
            ),
            (
                "related_native_role:commands",
                "source group indexer commands",
            ),
            (
                "related_native_role:compilation_database",
                "source group compilation database pre index task",
            ),
            (
                "related_native_role:pre_index",
                "source group pre index task",
            ),
        ] {
            let query = query.to_string();
            if seen.insert(query.clone()) {
                queries.push((relation.to_string(), query));
            }
        }
    }
    if drill_tokens_contain_all(&tokens, &["indexer", "java"]) {
        let query = "java indexer do index".to_string();
        if seen.insert(query.clone()) {
            queries.push(("related_native_role:java_index".to_string(), query));
        }
    }
    if drill_tokens_contain_all(&tokens, &["storage", "access"]) {
        for query in [
            "storage access proxy",
            "persistent storage",
            "component factory storage access",
        ] {
            if seen.insert(query.to_string()) {
                queries.push((
                    format!(
                        "related_storage_access:{}",
                        codestory_runtime::terminal_symbol_segment(query)
                    ),
                    query.to_string(),
                ));
            }
        }
    }
    queries
}

fn drill_native_related_query_matches(hit: &SearchHit, query: &str) -> bool {
    let display = codestory_runtime::normalize_symbol_query(&hit.display_name);
    let query = codestory_runtime::normalize_symbol_query(query);
    if display == query {
        return true;
    }
    let query_terminal = codestory_runtime::terminal_symbol_segment(&query);
    let display_terminal = codestory_runtime::terminal_symbol_segment(&display);
    display.ends_with(&query)
        || (!query_terminal.is_empty() && display_terminal == query_terminal)
        || drill_related_query_matches_by_tokens(&hit.display_name, &query)
}

fn drill_related_query_matches_by_tokens(display_name: &str, query: &str) -> bool {
    let display_tokens = drill_related_query_tokens(display_name);
    let query_tokens = drill_related_query_tokens(query);
    !query_tokens.is_empty()
        && query_tokens
            .iter()
            .all(|token| display_tokens.contains(token))
}

fn drill_tokens_contain_all(tokens: &[String], required: &[&str]) -> bool {
    required
        .iter()
        .all(|required| tokens.iter().any(|token| token == required))
}

fn drill_tokens_contain_any(tokens: &[String], required: &[&str]) -> bool {
    required
        .iter()
        .any(|required| tokens.iter().any(|token| token == required))
}

fn drill_related_query_tokens(value: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut seen = HashSet::new();
    for token in codestory_runtime::symbol_query_tokens(value) {
        if token.len() >= 2 && seen.insert(token.clone()) {
            tokens.push(token);
        }
    }
    tokens
}

fn drill_is_payload_collection_hit(hit: &SearchHit, slug: &str) -> bool {
    let display = hit.display_name.to_ascii_lowercase();
    let slug = slug.to_ascii_lowercase();
    display.contains(&format!("payload::collection::{slug}"))
        || display.contains(&format!("payload collection {slug}"))
        || display.contains(&format!("collection::{slug}"))
}

fn drill_preferred_payload_collection_source_file(
    verification_targets: &[VerificationTargetOutput],
    slug: &str,
) -> Option<String> {
    let slug = slug.to_ascii_lowercase();
    verification_targets
        .iter()
        .filter(|target| {
            let path = target.path.replace('\\', "/").to_ascii_lowercase();
            path.contains("/collections/")
                || path.contains("/collection/")
                || path.contains(&format!("/{slug}."))
                || target
                    .node_ref
                    .as_deref()
                    .is_some_and(|node_ref| node_ref.to_ascii_lowercase().contains(&slug))
        })
        .min_by_key(|target| {
            (
                drill_file_rank_for_agent(Some(target.path.as_str())),
                target.line,
                target.path.clone(),
            )
        })
        .map(|target| target.path.clone())
}

fn drill_payload_collection_slug_candidates(
    anchor: &str,
    hit: &SearchHit,
    verification_targets: &[VerificationTargetOutput],
) -> Vec<String> {
    let mut candidates = Vec::new();
    let mut seen = HashSet::new();
    for value in [anchor, hit.display_name.as_str()]
        .into_iter()
        .chain(hit.file_path.as_deref())
        .chain(verification_targets.iter().flat_map(|target| {
            [target.path.as_str()]
                .into_iter()
                .chain(target.node_ref.as_deref())
        }))
    {
        for slug in drill_slug_candidates_from_value(value) {
            if seen.insert(slug.clone()) {
                candidates.push(slug);
            }
        }
    }
    candidates
}

fn drill_slug_candidates_from_value(value: &str) -> Vec<String> {
    let mut raw = Vec::new();
    raw.push(value.to_string());
    if let Some((_, tail)) = value.rsplit_once("payload:collection:") {
        raw.push(tail.to_string());
    }
    if let Some((_, tail)) = value.rsplit_once("payload::collection::") {
        raw.push(tail.to_string());
    }
    if let Some((_, tail)) = value.rsplit_once("collection::") {
        raw.push(tail.to_string());
    }

    let normalized_path = value.replace('\\', "/");
    if let Some(file_name) = normalized_path.rsplit('/').next() {
        raw.push(file_name.to_string());
        if let Some((stem, _)) = file_name.rsplit_once('.') {
            raw.push(stem.to_string());
        }
    }
    if let Some((_, tail)) = value.rsplit_once("::") {
        raw.push(tail.to_string());
    }

    let mut candidates = Vec::new();
    let mut seen = HashSet::new();
    for raw_value in raw {
        if let Some(slug) = drill_normalize_slug_candidate(&raw_value)
            && seen.insert(slug.clone())
        {
            candidates.push(slug);
        }
    }
    candidates
}

fn drill_normalize_slug_candidate(value: &str) -> Option<String> {
    let trimmed = value
        .trim()
        .trim_matches(|ch: char| !ch.is_ascii_alphanumeric());
    if trimmed.is_empty() || trimmed.len() > 80 {
        return None;
    }

    let mut slug = String::new();
    let mut prev_was_sep = false;
    let mut prev_was_lower_or_digit = false;
    for ch in trimmed.chars() {
        if ch.is_ascii_alphanumeric() {
            if ch.is_ascii_uppercase() && prev_was_lower_or_digit && !prev_was_sep {
                slug.push('-');
            }
            slug.push(ch.to_ascii_lowercase());
            prev_was_sep = false;
            prev_was_lower_or_digit = ch.is_ascii_lowercase() || ch.is_ascii_digit();
        } else if !slug.is_empty() && !prev_was_sep {
            slug.push('-');
            prev_was_sep = true;
            prev_was_lower_or_digit = false;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    if slug.is_empty()
        || slug.contains("payload-collection")
        || slug.contains("source-repos")
        || slug.contains("src-")
    {
        return None;
    }
    Some(slug)
}

fn drill_trail_request(root_id: &NodeId) -> TrailConfigDto {
    TrailConfigDto {
        root_id: root_id.clone(),
        mode: TrailMode::Neighborhood,
        target_id: None,
        depth: 2,
        direction: TrailDirection::Both,
        caller_scope: TrailCallerScope::ProductionOnly,
        edge_filter: Vec::new(),
        show_utility_calls: false,
        hide_speculative: true,
        story: true,
        node_filter: Vec::new(),
        max_nodes: 120,
        layout_direction: codestory_contracts::api::LayoutDirection::Horizontal,
    }
}

fn drill_trail_command(
    project: &args::ProjectArgs,
    target: &runtime::ResolvedTarget,
) -> TrailCommand {
    TrailCommand {
        project: project.clone(),
        target: args::TargetArgs {
            id: Some(target.selected.node_id.0.clone()),
            query: None,
            file: None,
            choose: None,
        },
        mode: args::CliTrailMode::Neighborhood,
        depth: Some(2),
        direction: Some(args::CliDirection::Both),
        max_nodes: 120,
        include_tests: false,
        show_utility_calls: false,
        hide_speculative: true,
        story: true,
        layout: args::CliLayout::Horizontal,
        refresh: args::RefreshMode::None,
        format: args::OutputFormat::Markdown,
        output_file: None,
        mermaid: false,
    }
}

fn cmd_project_args(project_root: &std::path::Path) -> args::ProjectArgs {
    args::ProjectArgs {
        project: project_root.to_path_buf(),
        cache_dir: None,
    }
}

fn output_slug(value: &str) -> String {
    let mut slug = String::new();
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            slug.push(ch);
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = slug.trim_matches('-');
    if slug.is_empty() {
        "anchor".to_string()
    } else {
        slug.to_string()
    }
}

fn dedupe_verification_targets(targets: &mut Vec<VerificationTargetOutput>) {
    let mut seen = HashSet::new();
    targets.retain(|target| {
        seen.insert((
            target.role.clone(),
            target.path.clone(),
            target.line,
            target.reason.clone(),
        ))
    });
}

fn drill_evidence_packet(
    question: Option<&str>,
    question_search: Option<&DrillCommandStatusOutput>,
    question_supplemental_searches: &[DrillCommandStatusOutput],
    anchors: &[DrillAnchorOutput],
    bridges: &[DrillBridgeOutput],
    verification_targets: &[VerificationTargetOutput],
    next_commands: &[String],
) -> EvidencePacketDto {
    let mut items = Vec::new();
    if let Some(status) = question_search {
        items.push(evidence_item_from_command(
            "question-search",
            EvidenceTypeDto::SearchHit,
            status,
            "medium",
            ClaimReadinessDto::Partial,
            vec![
                "natural-language question search is broad discovery evidence; use drill anchors and source verification before answering"
                    .to_string(),
            ],
        ));
    }
    for (index, status) in question_supplemental_searches.iter().enumerate() {
        items.push(evidence_item_from_command(
            &format!("question-supplemental-search-{}", index + 1),
            EvidenceTypeDto::SearchHit,
            status,
            if status.status == "ok" { "medium" } else { "low" },
            if status.status == "ok" {
                ClaimReadinessDto::Partial
            } else {
                ClaimReadinessDto::NeedsSourceRead
            },
            vec![
                "supplemental question search expands likely runtime/source-truth surfaces; verify source before final claims"
                    .to_string(),
            ],
        ));
    }

    for (index, anchor) in anchors.iter().enumerate() {
        let anchor_id = format!("anchor-{}", index + 1);
        match anchor.chosen_anchor.as_ref() {
            Some(hit) => items.push(evidence_item_from_hit(&anchor_id, anchor, hit)),
            None => items.push(EvidenceItemDto {
                id: anchor_id.clone(),
                evidence_type: EvidenceTypeDto::Negative,
                command: format!("search {}", anchor.anchor),
                status: "no_resolvable_anchor".to_string(),
                confidence: "low".to_string(),
                verification_status: ClaimReadinessDto::NeedsSourceRead,
                match_quality: None,
                source: None,
                artifacts: drill_command_artifacts(&anchor.commands),
                notes: vec![
                    "no typed/resolvable anchor was selected; do not make architecture claims from this anchor"
                        .to_string(),
                ],
            }),
        }

        for (command_index, status) in anchor.commands.iter().enumerate() {
            items.push(evidence_item_from_command(
                &format!("{anchor_id}-command-{}", command_index + 1),
                evidence_type_for_drill_command(&status.command),
                status,
                if status.status == "ok" {
                    "medium"
                } else {
                    "low"
                },
                readiness_for_drill_command(status),
                drill_command_notes(status),
            ));
        }
    }

    for (index, bridge) in bridges.iter().enumerate() {
        items.push(evidence_item_from_bridge(index + 1, bridge));
    }

    let source_truth_checks =
        source_truth_checks_from_drill_evidence(verification_targets, anchors, bridges);
    let readiness = drill_answer_readiness(&items, &source_truth_checks, next_commands);
    EvidencePacketDto {
        packet_version: 1,
        question: question.map(ToOwned::to_owned),
        items,
        readiness,
    }
}

fn evidence_item_from_hit(
    id: &str,
    anchor: &DrillAnchorOutput,
    hit: &SearchHitOutput,
) -> EvidenceItemDto {
    let is_repo_text = matches!(
        hit.origin,
        codestory_contracts::api::SearchHitOrigin::TextMatch
    ) || !hit.resolvable;
    EvidenceItemDto {
        id: id.to_string(),
        evidence_type: if is_repo_text {
            EvidenceTypeDto::RepoText
        } else {
            EvidenceTypeDto::SearchHit
        },
        command: format!("search {}", anchor.anchor),
        status: "selected_anchor".to_string(),
        confidence: if is_repo_text { "low" } else { "high" }.to_string(),
        verification_status: if is_repo_text {
            ClaimReadinessDto::NeedsSourceRead
        } else {
            ClaimReadinessDto::Anchored
        },
        match_quality: Some(hit.match_quality),
        source: hit
            .file_path
            .as_ref()
            .map(|path| EvidenceSourceLocationDto {
                path: path.clone(),
                line_start: hit.line,
                line_end: hit.line,
            }),
        artifacts: drill_command_artifacts(&anchor.commands),
        notes: evidence_hit_notes(anchor, hit),
    }
}

fn evidence_hit_notes(anchor: &DrillAnchorOutput, hit: &SearchHitOutput) -> Vec<String> {
    let mut notes = Vec::new();
    notes.push(format!("typed_hit_count={}", anchor.typed_hit_count));
    if let Some(summary) = anchor.consumer_summary.as_ref() {
        notes.push(format!(
            "visible_production_callers={} visible_consumers={} text_consumer_hints={}",
            summary.caller_count, summary.consumer_count, summary.text_hint_count
        ));
        for caller in summary.callers.iter().take(3) {
            let path = caller.file_path.as_deref().unwrap_or("<no-file>");
            let target = caller
                .target_name
                .as_deref()
                .map(|name| format!(" -> {name}"))
                .unwrap_or_default();
            notes.push(format!(
                "caller: {} [{:?}] {}{} via {:?}",
                caller.name, caller.kind, path, target, caller.edge_kind
            ));
        }
        for hint in summary.text_consumer_hints.iter().take(3) {
            let path = hint.file_path.as_deref().unwrap_or("<no-file>");
            notes.push(format!(
                "text-hint: {} [{:?}] {}:{}",
                hint.name,
                hint.kind,
                path,
                hint.line
                    .map(|line| line.to_string())
                    .unwrap_or_else(|| "?".to_string())
            ));
        }
        notes.extend(summary.notes.iter().take(2).cloned());
    }
    if matches!(
        hit.origin,
        codestory_contracts::api::SearchHitOrigin::TextMatch
    ) {
        notes.push(
            "repo-text hits are file/line hints only; choose a typed symbol before graph browsing"
                .to_string(),
        );
    }
    if hit.verification_targets.is_empty() {
        notes.push("no source occurrence metadata was available for this selected hit".to_string());
    }
    notes
}

fn evidence_item_from_command(
    id: &str,
    evidence_type: EvidenceTypeDto,
    status: &DrillCommandStatusOutput,
    confidence: &str,
    verification_status: ClaimReadinessDto,
    mut notes: Vec<String>,
) -> EvidenceItemDto {
    if let Some(error) = status.error.as_ref() {
        notes.push(format!("command error: {error}"));
    }
    EvidenceItemDto {
        id: id.to_string(),
        evidence_type: if status.status == "ok" {
            evidence_type
        } else {
            EvidenceTypeDto::Negative
        },
        command: status.command.clone(),
        status: status.status.clone(),
        confidence: confidence.to_string(),
        verification_status,
        match_quality: None,
        source: None,
        artifacts: status.artifact.iter().cloned().collect(),
        notes,
    }
}

fn evidence_item_from_bridge(index: usize, bridge: &DrillBridgeOutput) -> EvidenceItemDto {
    let evidence = &bridge.evidence;
    let verification_status = match evidence.status.as_str() {
        "graph_path" => ClaimReadinessDto::Supported,
        "reverse_graph_path" => ClaimReadinessDto::Partial,
        "source_truth_only" => ClaimReadinessDto::NeedsSourceRead,
        status if drill_bridge_status_is_partial(status) => ClaimReadinessDto::Partial,
        "no_bridge_found" | "unresolved_anchor" | "error" => ClaimReadinessDto::NeedsSourceRead,
        _ => ClaimReadinessDto::Inferred,
    };
    EvidenceItemDto {
        id: format!("bridge-{index}"),
        evidence_type: if bridge.command.status == "ok" {
            EvidenceTypeDto::Bridge
        } else {
            EvidenceTypeDto::Negative
        },
        command: bridge.command.command.clone(),
        status: evidence.status.clone(),
        confidence: evidence.confidence.clone(),
        verification_status,
        match_quality: None,
        source: None,
        artifacts: bridge.command.artifact.iter().cloned().collect(),
        notes: evidence.notes.clone(),
    }
}

fn evidence_type_for_drill_command(command: &str) -> EvidenceTypeDto {
    match command {
        "search" | "question_search" => EvidenceTypeDto::SearchHit,
        "symbol" => EvidenceTypeDto::SymbolContext,
        "trail" => EvidenceTypeDto::Trail,
        "snippet" => EvidenceTypeDto::Snippet,
        "explore" => EvidenceTypeDto::Explore,
        "bridge" => EvidenceTypeDto::Bridge,
        _ => EvidenceTypeDto::Negative,
    }
}

fn readiness_for_drill_command(status: &DrillCommandStatusOutput) -> ClaimReadinessDto {
    if status.status != "ok" {
        return ClaimReadinessDto::NeedsSourceRead;
    }
    match status.command.as_str() {
        "symbol" | "snippet" | "explore" => ClaimReadinessDto::Supported,
        "trail" => ClaimReadinessDto::Partial,
        "search" => ClaimReadinessDto::Anchored,
        "question_search" => ClaimReadinessDto::Partial,
        _ => ClaimReadinessDto::Partial,
    }
}

fn drill_command_notes(status: &DrillCommandStatusOutput) -> Vec<String> {
    match status.command.as_str() {
        "question_search" => {
            vec![
                "natural-language question search is broad discovery evidence; use drill anchors and source verification before answering".to_string(),
            ]
        }
        "search" => {
            vec!["search is anchor discovery, not final source-truth verification".to_string()]
        }
        "trail" => vec![
            "trail evidence may omit speculative edges and should be checked against snippets/source when architecture direction matters"
                .to_string(),
        ],
        "snippet" => vec!["snippet is source-backed local context for the selected target".to_string()],
        "explore" => vec!["explore packet broadens nearby files and related symbols".to_string()],
        _ => Vec::new(),
    }
}

fn drill_command_artifacts(commands: &[DrillCommandStatusOutput]) -> Vec<String> {
    commands
        .iter()
        .filter_map(|command| command.artifact.clone())
        .collect()
}

#[derive(Debug, Clone)]
struct SourceTruthCheckSeed {
    role: String,
    detail: String,
    path: String,
    line: Option<u32>,
    required: bool,
}

#[derive(Debug)]
struct SourceTruthCheckGroup {
    path: String,
    line: Option<u32>,
    required: bool,
    roles: BTreeSet<String>,
    details: Vec<String>,
    omitted_detail_count: usize,
}

fn source_truth_checks_from_drill_evidence(
    targets: &[VerificationTargetOutput],
    anchors: &[DrillAnchorOutput],
    bridges: &[DrillBridgeOutput],
) -> Vec<SourceTruthCheckDto> {
    let mut checks = Vec::new();
    let mut seen = HashSet::new();
    for target in targets {
        push_source_truth_check(
            &mut checks,
            &mut seen,
            source_truth_role_for_verification_reason(&target.reason),
            target.reason.clone(),
            target.path.clone(),
            Some(target.line),
            true,
        );
    }
    for anchor in anchors {
        push_consumer_source_truth_checks(&mut checks, &mut seen, anchor);
    }
    for bridge in bridges {
        push_bridge_source_truth_checks(&mut checks, &mut seen, &bridge.evidence);
    }
    rank_source_truth_checks(&mut checks);
    let mut checks = compact_source_truth_checks(checks);
    rank_source_truth_checks(&mut checks);

    checks
        .into_iter()
        .enumerate()
        .map(
            |(index, check): (usize, SourceTruthCheckSeed)| SourceTruthCheckDto {
                id: format!("source-truth-{}", index + 1),
                reason: check.detail,
                path: check.path,
                line: check.line,
                required: check.required,
            },
        )
        .collect()
}

fn rank_source_truth_checks(checks: &mut [SourceTruthCheckSeed]) {
    checks.sort_by_cached_key(|check| {
        (
            drill_file_rank_for_agent(Some(check.path.as_str())),
            check.path.clone(),
            check.line.unwrap_or_default(),
            check.role.clone(),
            check.detail.clone(),
        )
    });
}

fn push_source_truth_check(
    checks: &mut Vec<SourceTruthCheckSeed>,
    seen: &mut HashSet<(String, Option<u32>, String, String)>,
    role: impl Into<String>,
    detail: String,
    path: String,
    line: Option<u32>,
    required: bool,
) {
    if path.trim().is_empty() {
        return;
    }
    let role = role.into();
    if seen.insert((path.clone(), line, role.clone(), detail.clone())) {
        checks.push(SourceTruthCheckSeed {
            role,
            detail,
            path,
            line,
            required,
        });
    }
}

fn push_consumer_source_truth_checks(
    checks: &mut Vec<SourceTruthCheckSeed>,
    seen: &mut HashSet<(String, Option<u32>, String, String)>,
    anchor: &DrillAnchorOutput,
) {
    let Some(summary) = anchor.consumer_summary.as_ref() else {
        return;
    };

    for caller in summary.callers.iter().take(2) {
        if let Some(path) = caller.file_path.as_ref() {
            push_source_truth_check(
                checks,
                seen,
                "consumer",
                format!(
                    "consumer evidence for {}: caller {} reaches {} via {:?}",
                    anchor.anchor,
                    caller.name,
                    caller
                        .target_name
                        .as_deref()
                        .unwrap_or(anchor.anchor.as_str()),
                    caller.edge_kind
                ),
                path.clone(),
                None,
                true,
            );
        }
    }

    for consumer in summary.consumers.iter().take(3) {
        if let Some(path) = consumer.file_path.as_ref() {
            push_source_truth_check(
                checks,
                seen,
                "consumer",
                format!(
                    "consumer evidence for {}: {} uses {} via {:?}",
                    anchor.anchor,
                    consumer.name,
                    consumer
                        .target_name
                        .as_deref()
                        .unwrap_or(anchor.anchor.as_str()),
                    consumer.edge_kind
                ),
                path.clone(),
                None,
                true,
            );
        }
        if let Some(path) = consumer.target_file_path.as_ref() {
            push_source_truth_check(
                checks,
                seen,
                "related target",
                format!(
                    "consumer target for {}: verify related target {}",
                    anchor.anchor,
                    consumer
                        .target_name
                        .as_deref()
                        .unwrap_or(anchor.anchor.as_str())
                ),
                path.clone(),
                None,
                true,
            );
        }
    }

    for hint in summary.text_consumer_hints.iter().take(3) {
        if let Some(path) = hint.file_path.as_ref() {
            push_source_truth_check(
                checks,
                seen,
                "text hint",
                format!(
                    "text consumer hint for {}: {} matched repo text",
                    anchor.anchor, hint.name
                ),
                path.clone(),
                hint.line,
                true,
            );
        }
    }
}

fn push_bridge_source_truth_checks(
    checks: &mut Vec<SourceTruthCheckSeed>,
    seen: &mut HashSet<(String, Option<u32>, String, String)>,
    bridge: &DrillBridgeEvidenceOutput,
) {
    let prefix = format!("bridge {} -> {}", bridge.from_anchor, bridge.to_anchor);
    if let Some(from_node) = bridge.from_node.as_ref()
        && let Some(path) = from_node.file_path.as_ref()
    {
        push_source_truth_check(
            checks,
            seen,
            "bridge endpoint",
            format!("{prefix}: verify from endpoint {}", from_node.display_name),
            path.clone(),
            from_node.line,
            true,
        );
    }
    if let Some(to_node) = bridge.to_node.as_ref()
        && let Some(path) = to_node.file_path.as_ref()
    {
        push_source_truth_check(
            checks,
            seen,
            "bridge endpoint",
            format!("{prefix}: verify to endpoint {}", to_node.display_name),
            path.clone(),
            to_node.line,
            true,
        );
    }
    for path in bridge.shared_files.iter().take(3) {
        push_source_truth_check(
            checks,
            seen,
            "bridge shared file",
            format!(
                "{prefix}: inspect shared file for {} bridge evidence",
                bridge.status
            ),
            path.clone(),
            None,
            true,
        );
    }
    for path in bridge.evidence_files.iter().take(3) {
        push_source_truth_check(
            checks,
            seen,
            "bridge hint",
            format!(
                "{prefix}: inspect consumer/text evidence file for {} bridge evidence",
                bridge.status
            ),
            path.clone(),
            None,
            true,
        );
    }
}

fn source_truth_role_for_verification_reason(reason: &str) -> &'static str {
    let lower = reason.to_ascii_lowercase();
    if lower.contains("primary source occurrence") || lower.contains("selected") {
        "selected anchor"
    } else if lower.contains("snippet") || lower.contains("body") {
        "source body"
    } else {
        "source target"
    }
}

fn compact_source_truth_checks(checks: Vec<SourceTruthCheckSeed>) -> Vec<SourceTruthCheckSeed> {
    let mut groups = Vec::<SourceTruthCheckGroup>::new();
    let mut group_by_path = HashMap::<String, usize>::new();
    for check in checks {
        let index = if let Some(index) = group_by_path.get(&check.path) {
            *index
        } else {
            let index = groups.len();
            group_by_path.insert(check.path.clone(), index);
            groups.push(SourceTruthCheckGroup {
                path: check.path.clone(),
                line: check.line,
                required: false,
                roles: BTreeSet::new(),
                details: Vec::new(),
                omitted_detail_count: 0,
            });
            index
        };
        groups[index].push(check);
    }
    groups
        .into_iter()
        .map(SourceTruthCheckGroup::into_seed)
        .collect()
}

impl SourceTruthCheckGroup {
    fn push(&mut self, check: SourceTruthCheckSeed) {
        self.required |= check.required;
        self.roles.insert(check.role);
        self.line = match (self.line, check.line) {
            (Some(current), Some(next)) => Some(current.min(next)),
            (None, Some(next)) => Some(next),
            (current, None) => current,
        };
        if self.details.iter().any(|detail| detail == &check.detail) {
            return;
        }
        if self.details.len() < 4 {
            self.details.push(check.detail);
        } else {
            self.omitted_detail_count += 1;
        }
    }

    fn into_seed(self) -> SourceTruthCheckSeed {
        let roles = self.roles.into_iter().collect::<Vec<_>>().join(", ");
        let mut detail = format!("verify {roles} evidence");
        if !self.details.is_empty() {
            detail.push_str(": ");
            detail.push_str(&self.details.join("; "));
        }
        if self.omitted_detail_count > 0 {
            let _ = write!(
                detail,
                "; plus {} more signal{}",
                self.omitted_detail_count,
                if self.omitted_detail_count == 1 {
                    ""
                } else {
                    "s"
                }
            );
        }
        SourceTruthCheckSeed {
            role: roles,
            detail,
            path: self.path,
            line: self.line,
            required: self.required,
        }
    }
}

fn drill_answer_readiness(
    items: &[EvidenceItemDto],
    source_truth_checks: &[SourceTruthCheckDto],
    next_commands: &[String],
) -> AnswerReadinessReportDto {
    let pending_required_source_truth = source_truth_checks.iter().any(|check| check.required);
    let has_source_blocked_item = items.iter().any(|item| {
        matches!(
            item.verification_status,
            ClaimReadinessDto::NeedsSourceRead | ClaimReadinessDto::ContradictedBySource
        )
    });
    let has_partial_item = items
        .iter()
        .any(|item| matches!(item.verification_status, ClaimReadinessDto::Partial));
    let overall_status = if source_truth_checks.is_empty() || has_source_blocked_item {
        ClaimReadinessDto::NeedsSourceRead
    } else if has_partial_item {
        ClaimReadinessDto::Partial
    } else if pending_required_source_truth {
        ClaimReadinessDto::NeedsSourceRead
    } else {
        ClaimReadinessDto::Supported
    };

    let mut safe_to_say = Vec::new();
    let mut inferred_claims = Vec::new();
    let mut needs_verification = Vec::new();
    for item in items {
        match item.verification_status {
            ClaimReadinessDto::Anchored | ClaimReadinessDto::Supported => {
                safe_to_say.push(format!(
                    "{} evidence `{}` is available with {} confidence",
                    evidence_type_label(item.evidence_type),
                    item.id,
                    item.confidence
                ));
            }
            ClaimReadinessDto::Partial | ClaimReadinessDto::Inferred => {
                inferred_claims.push(format!(
                    "{} evidence `{}` is {} and must not be presented as verified architecture",
                    evidence_type_label(item.evidence_type),
                    item.id,
                    readiness_label(item.verification_status)
                ));
            }
            ClaimReadinessDto::NeedsSourceRead | ClaimReadinessDto::ContradictedBySource => {
                needs_verification.push(format!(
                    "{} evidence `{}` requires source-truth verification",
                    evidence_type_label(item.evidence_type),
                    item.id
                ));
            }
        }
    }
    if source_truth_checks.is_empty() {
        needs_verification.push(
            "no source-truth targets were emitted; verify candidate source files before finalizing"
                .to_string(),
        );
    } else if pending_required_source_truth {
        needs_verification.push(
            "required source-truth checks are pending; read the listed files before treating the packet as supported"
                .to_string(),
        );
    }

    AnswerReadinessReportDto {
        overall_status,
        safe_to_say,
        inferred_claims,
        needs_verification,
        next_commands: next_commands.to_vec(),
        source_truth_checks: source_truth_checks.to_vec(),
    }
}

fn evidence_type_label(evidence_type: EvidenceTypeDto) -> &'static str {
    match evidence_type {
        EvidenceTypeDto::SearchHit => "search",
        EvidenceTypeDto::SymbolContext => "symbol",
        EvidenceTypeDto::Trail => "trail",
        EvidenceTypeDto::Snippet => "snippet",
        EvidenceTypeDto::Explore => "explore",
        EvidenceTypeDto::Bridge => "bridge",
        EvidenceTypeDto::RepoText => "repo-text",
        EvidenceTypeDto::Negative => "negative",
    }
}

fn readiness_label(readiness: ClaimReadinessDto) -> &'static str {
    match readiness {
        ClaimReadinessDto::Anchored => "anchored",
        ClaimReadinessDto::Supported => "supported",
        ClaimReadinessDto::Partial => "partial",
        ClaimReadinessDto::Inferred => "inferred",
        ClaimReadinessDto::NeedsSourceRead => "needs_source_read",
        ClaimReadinessDto::ContradictedBySource => "contradicted_by_source",
    }
}

fn drill_next_commands(
    project_root: &std::path::Path,
    anchors: &[DrillAnchorOutput],
    bridges: &[DrillBridgeOutput],
    stale_freshness: bool,
) -> Vec<String> {
    let project = quote_command_path(project_root);
    let refresh = if stale_freshness {
        "incremental"
    } else {
        "none"
    };
    let mut commands = anchors
        .iter()
        .take(5)
        .flat_map(|anchor| {
            let query = quote_command_value(&anchor.anchor);
            let mut commands = vec![
                format!(
                    "codestory-cli search --project {project} --query {query} --refresh {refresh} --repo-text auto --why"
                ),
            ];
            if let Some(hit) = anchor.chosen_anchor.as_ref() {
                let id = quote_command_value(&hit.node_id);
                commands.push(format!(
                    "codestory-cli symbol --project {project} --id {id} --refresh {refresh}"
                ));
                commands.push(format!(
                    "codestory-cli snippet --project {project} --id {id} --function-body --context 40 --refresh {refresh}"
                ));
            } else {
                commands.push(format!(
                    "codestory-cli snippet --project {project} --query {query} --function-body --context 40 --refresh {refresh}"
                ));
            }
            commands
        })
        .collect::<Vec<_>>();

    if stale_freshness {
        commands.insert(
            0,
            format!("codestory-cli index --project {project} --refresh incremental"),
        );
    }

    for bridge in bridges.iter().take(5) {
        let evidence = &bridge.evidence;
        if evidence.next_commands.is_empty() {
            let bridge_query =
                quote_command_value(&format!("{} {}", evidence.from_anchor, evidence.to_anchor));
            commands.push(format!(
                "codestory-cli search --project {project} --query {bridge_query} --refresh {refresh} --why"
            ));
            if let Some(from_node) = evidence.from_node.as_ref() {
                let from_id = quote_command_value(&from_node.node_id);
                commands.push(format!(
                    "codestory-cli trail --project {project} --id {from_id} --refresh {refresh} --story --hide-speculative"
                ));
            } else {
                let from_query = quote_command_value(&evidence.from_anchor);
                commands.push(format!(
                    "codestory-cli trail --project {project} --query {from_query} --refresh {refresh} --story --hide-speculative"
                ));
            }
        } else {
            for command in &evidence.next_commands {
                commands.push(command.clone());
            }
        }
    }

    let mut seen = HashSet::new();
    commands
        .into_iter()
        .filter(|command| seen.insert(command.clone()))
        .collect()
}

fn search_output_from_results(
    runtime: &RuntimeContext,
    search_results: &codestory_contracts::api::SearchResultsDto,
    include_score_details: bool,
) -> SearchOutput {
    let occurrences = collect_search_hit_occurrences(
        runtime,
        search_results
            .indexed_symbol_hits
            .iter()
            .chain(search_results.suggestions.iter()),
    );
    build_search_output(SearchOutputParts {
        project_root: &runtime.project_root,
        query: &search_results.query,
        retrieval: &search_results.retrieval,
        retrieval_shadow: search_results.retrieval_shadow.as_ref(),
        freshness: search_results.freshness.as_ref(),
        symbol_hits: &search_results.indexed_symbol_hits,
        repo_text_hits: &search_results.repo_text_hits,
        repo_text_stats: search_results.repo_text_stats.as_ref(),
        query_assessment: search_results.query_assessment.as_ref(),
        search_plan: search_results.search_plan.as_ref(),
        suggestions: &search_results.suggestions,
        occurrences_by_node: &occurrences,
        limit_per_source: search_results.limit_per_source,
        repo_text: RepoTextOutputConfig {
            mode: from_api_repo_text_mode(search_results.repo_text_mode),
            enabled: search_results.repo_text_enabled,
        },
        explain: include_score_details,
    })
}

fn run_symbol(cmd: SymbolCommand) -> Result<()> {
    ensure_dot_only_for_trail(cmd.format, "symbol")?;
    preflight_output_file(cmd.output_file.as_deref())?;
    let runtime = RuntimeContext::new(&cmd.project)?;
    let opened = runtime.ensure_open(cmd.refresh)?;
    ensure_index_ready(&opened, "symbol")?;

    let file_filter = cmd.target.file_filter();
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
    if cmd.mermaid {
        return emit_text(render_symbol_mermaid(&context), cmd.output_file.as_deref());
    }
    let resolution = build_query_resolution_output_with_runtime(&runtime, &target);
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
    emit(cmd.format, &output, markdown, cmd.output_file.as_deref())
}

#[derive(Debug, Clone, Copy)]
enum SymbolWorkflowKind {
    Impact,
    TestMap,
}

impl SymbolWorkflowKind {
    fn label(self) -> &'static str {
        match self {
            Self::Impact => "impact",
            Self::TestMap => "test_map",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::Impact => "Symbol Impact",
            Self::TestMap => "Symbol Test Map",
        }
    }
}

#[derive(serde::Serialize)]
struct SymbolWorkflowNodeOutput {
    node_id: NodeId,
    display_name: String,
    kind: String,
    file_path: Option<String>,
    depth: u32,
}

#[derive(serde::Serialize)]
struct SymbolWorkflowRouteOutput {
    display_name: String,
    method: String,
    path: String,
    file_path: Option<String>,
    line: Option<u32>,
    confidence: String,
    reason: String,
}

#[derive(serde::Serialize)]
struct SymbolWorkflowTestOutput {
    path: String,
    reason: String,
    confidence: String,
    graph_depth: u32,
    impacted_symbol_count: u32,
}

#[derive(serde::Serialize)]
struct SymbolWorkflowCapsOutput {
    caller_depth: u32,
    caller_max_nodes: u32,
    affected_depth: u32,
    impacted_symbols_cap: u32,
    impacted_routes_cap: u32,
    affected_seed: String,
}

#[derive(serde::Serialize)]
struct SymbolWorkflowOutput<'a> {
    workflow: &'static str,
    project_root: String,
    resolution: QueryResolutionOutput,
    symbol: &'a codestory_contracts::api::SymbolContextDto,
    direct_callers: Vec<SymbolWorkflowNodeOutput>,
    transitive_callers: Vec<SymbolWorkflowNodeOutput>,
    impacted_files: Vec<String>,
    impacted_routes: Vec<SymbolWorkflowRouteOutput>,
    likely_tests: Vec<SymbolWorkflowTestOutput>,
    caps: SymbolWorkflowCapsOutput,
    unknowns: Vec<String>,
    next_commands: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    affected: Option<&'a codestory_contracts::api::AffectedAnalysisDto>,
    trail: &'a TrailContextDto,
}

fn run_symbol_workflow(kind: SymbolWorkflowKind, cmd: SymbolWorkflowCommand) -> Result<()> {
    ensure_dot_only_for_trail(cmd.format, kind.label())?;
    preflight_output_file(cmd.output_file.as_deref())?;
    let runtime = RuntimeContext::new(&cmd.project)?;
    let opened = runtime.ensure_open(cmd.refresh)?;
    ensure_index_ready(&opened, kind.label())?;

    let file_filter = cmd.target.file_filter();
    let target = resolve_target_or_emit_ambiguity(
        &runtime,
        cmd.target.selection()?,
        file_filter.as_deref(),
        cmd.format,
        cmd.output_file.as_deref(),
    )?;
    let symbol = runtime
        .browser
        .symbol_context(target.selected.node_id.clone())
        .map_err(map_api_error)?;

    let depth = cmd.depth.clamp(1, 8);
    let max_nodes = cmd.max_nodes.clamp(1, 200);
    let include_tests = cmd.include_tests || matches!(kind, SymbolWorkflowKind::TestMap);
    let trail = runtime
        .browser
        .trail_context(TrailConfigDto {
            root_id: target.selected.node_id.clone(),
            mode: TrailMode::AllReferencing,
            target_id: None,
            depth,
            direction: TrailDirection::Incoming,
            caller_scope: if include_tests {
                TrailCallerScope::IncludeTestsAndBenches
            } else {
                TrailCallerScope::ProductionOnly
            },
            edge_filter: Vec::new(),
            show_utility_calls: false,
            hide_speculative: true,
            story: false,
            node_filter: Vec::new(),
            max_nodes,
            layout_direction: codestory_contracts::api::LayoutDirection::Horizontal,
        })
        .map_err(map_api_error)?;

    let affected_seed = target
        .selected
        .file_path
        .clone()
        .or_else(|| symbol.node.file_path.clone())
        .map(|path| symbol_workflow_seed_path(&runtime.project_root, &path));
    let affected = if let Some(path) = affected_seed.as_ref() {
        Some(
            runtime
                .browser
                .affected_analysis(AffectedAnalysisRequest {
                    changed_paths: vec![path.clone()],
                    change_records: vec![AffectedChangeRecordDto {
                        path: path.clone(),
                        kind: AffectedChangeKindDto::Unknown,
                        status: "symbol_file".to_string(),
                        previous_path: None,
                    }],
                    depth: Some(depth),
                    filter: None,
                })
                .map_err(map_api_error)?,
        )
    } else {
        None
    };

    let direct_callers = symbol_workflow_direct_callers(&trail);
    let transitive_callers = symbol_workflow_transitive_callers(&trail, &direct_callers);
    let impacted_files = symbol_workflow_impacted_files(affected.as_ref());
    let impacted_routes = symbol_workflow_routes(affected.as_ref());
    let likely_tests = symbol_workflow_tests(affected.as_ref());
    let unknowns = symbol_workflow_unknowns(
        affected.as_ref(),
        &trail,
        &direct_callers,
        &transitive_callers,
        &likely_tests,
        affected_seed.as_deref(),
        max_nodes,
    );
    let next_commands = symbol_workflow_next_commands(
        &runtime.project_root,
        &target.selected.node_id,
        affected_seed.as_deref(),
        depth,
        max_nodes,
        kind,
        include_tests,
    );
    let resolution = build_query_resolution_output_with_runtime(&runtime, &target);
    let caps = SymbolWorkflowCapsOutput {
        caller_depth: depth,
        caller_max_nodes: max_nodes,
        affected_depth: depth,
        impacted_symbols_cap: 200,
        impacted_routes_cap: 100,
        affected_seed: affected_seed
            .clone()
            .unwrap_or_else(|| "none: selected symbol has no indexed file path".to_string()),
    };
    let output = SymbolWorkflowOutput {
        workflow: kind.label(),
        project_root: runtime.project_root.to_string_lossy().to_string(),
        resolution,
        symbol: &symbol,
        direct_callers,
        transitive_callers,
        impacted_files,
        impacted_routes,
        likely_tests,
        caps,
        unknowns,
        next_commands,
        affected: affected.as_ref(),
        trail: &trail,
    };
    let markdown = render_symbol_workflow_markdown(kind, &output);
    emit(cmd.format, &output, markdown, cmd.output_file.as_deref())
}

fn symbol_workflow_direct_callers(trail: &TrailContextDto) -> Vec<SymbolWorkflowNodeOutput> {
    let nodes = trail
        .trail
        .nodes
        .iter()
        .map(|node| (node.id.0.clone(), node))
        .collect::<HashMap<_, _>>();
    let mut seen = HashSet::new();
    let mut callers = trail
        .trail
        .edges
        .iter()
        .filter(|edge| {
            edge.kind == codestory_contracts::api::EdgeKind::CALL
                && edge.target == trail.focus.id
                && edge.source != trail.focus.id
        })
        .filter_map(|edge| {
            if !seen.insert(edge.source.0.clone()) {
                return None;
            }
            nodes
                .get(&edge.source.0)
                .map(|node| symbol_workflow_node_output(node))
        })
        .collect::<Vec<_>>();
    callers.sort_by(|left, right| {
        left.file_path
            .cmp(&right.file_path)
            .then(left.display_name.cmp(&right.display_name))
    });
    callers
}

fn symbol_workflow_transitive_callers(
    trail: &TrailContextDto,
    direct_callers: &[SymbolWorkflowNodeOutput],
) -> Vec<SymbolWorkflowNodeOutput> {
    let direct_ids = direct_callers
        .iter()
        .map(|caller| caller.node_id.0.clone())
        .collect::<HashSet<_>>();
    let mut callers = trail
        .trail
        .nodes
        .iter()
        .filter(|node| {
            node.id != trail.focus.id
                && node.depth > 1
                && !direct_ids.contains(&node.id.0)
                && symbol_workflow_has_call_path_to_focus(trail, &node.id)
        })
        .map(symbol_workflow_node_output)
        .collect::<Vec<_>>();
    callers.sort_by(|left, right| {
        left.depth
            .cmp(&right.depth)
            .then(left.file_path.cmp(&right.file_path))
            .then(left.display_name.cmp(&right.display_name))
    });
    callers.truncate(50);
    callers
}

fn symbol_workflow_has_call_path_to_focus(trail: &TrailContextDto, start: &NodeId) -> bool {
    let mut seen = HashSet::new();
    let mut stack = vec![start.clone()];
    while let Some(current) = stack.pop() {
        if !seen.insert(current.0.clone()) {
            continue;
        }
        for edge in trail.trail.edges.iter().filter(|edge| {
            edge.kind == codestory_contracts::api::EdgeKind::CALL && edge.source == current
        }) {
            if edge.target == trail.focus.id {
                return true;
            }
            stack.push(edge.target.clone());
        }
    }
    false
}

fn symbol_workflow_node_output(
    node: &codestory_contracts::api::GraphNodeDto,
) -> SymbolWorkflowNodeOutput {
    SymbolWorkflowNodeOutput {
        node_id: node.id.clone(),
        display_name: node
            .qualified_name
            .clone()
            .unwrap_or_else(|| node.label.clone()),
        kind: format!("{:?}", node.kind).to_ascii_lowercase(),
        file_path: node.file_path.clone(),
        depth: node.depth,
    }
}

fn symbol_workflow_seed_path(project_root: &Path, path: &str) -> String {
    let clean_path = path
        .strip_prefix(r"\\?\")
        .unwrap_or(path)
        .replace('\\', "/");
    let root_lossy = project_root.to_string_lossy();
    let root = root_lossy
        .strip_prefix(r"\\?\")
        .unwrap_or(&root_lossy)
        .replace('\\', "/");
    clean_path
        .strip_prefix(&format!("{root}/"))
        .unwrap_or(&clean_path)
        .to_string()
}

fn symbol_workflow_impacted_files(
    affected: Option<&codestory_contracts::api::AffectedAnalysisDto>,
) -> Vec<String> {
    let mut files = BTreeSet::new();
    if let Some(affected) = affected {
        files.extend(affected.matched_files.iter().map(|file| file.path.clone()));
        files.extend(
            affected
                .impacted_symbols
                .iter()
                .filter_map(|symbol| symbol.file_path.clone()),
        );
        files.extend(
            affected
                .impacted_routes
                .iter()
                .filter_map(|route| route.file_path.clone()),
        );
        files.extend(
            affected
                .impacted_routes
                .iter()
                .filter_map(|route| route.route.source_file.clone()),
        );
        files.extend(affected.impacted_tests.iter().map(|test| test.path.clone()));
    }
    files.into_iter().collect()
}

fn symbol_workflow_routes(
    affected: Option<&codestory_contracts::api::AffectedAnalysisDto>,
) -> Vec<SymbolWorkflowRouteOutput> {
    affected
        .into_iter()
        .flat_map(|affected| affected.impacted_routes.iter())
        .map(|route| SymbolWorkflowRouteOutput {
            display_name: route.display_name.clone(),
            method: route.route.method.clone(),
            path: route.route.path.clone(),
            file_path: route
                .file_path
                .clone()
                .or_else(|| route.route.source_file.clone()),
            line: route.line.or(route.route.line),
            confidence: route.confidence.clone(),
            reason: route.reason.clone(),
        })
        .collect()
}

fn symbol_workflow_tests(
    affected: Option<&codestory_contracts::api::AffectedAnalysisDto>,
) -> Vec<SymbolWorkflowTestOutput> {
    affected
        .into_iter()
        .flat_map(|affected| affected.impacted_tests.iter())
        .map(|test| SymbolWorkflowTestOutput {
            path: test.path.clone(),
            reason: test.reason.clone(),
            confidence: test.confidence.clone(),
            graph_depth: test.graph_depth,
            impacted_symbol_count: test.impacted_symbol_count,
        })
        .collect()
}

fn symbol_workflow_unknowns(
    affected: Option<&codestory_contracts::api::AffectedAnalysisDto>,
    trail: &TrailContextDto,
    direct_callers: &[SymbolWorkflowNodeOutput],
    transitive_callers: &[SymbolWorkflowNodeOutput],
    likely_tests: &[SymbolWorkflowTestOutput],
    affected_seed: Option<&str>,
    max_nodes: u32,
) -> Vec<String> {
    let mut unknowns = Vec::new();
    unknowns.push(
        "affected files/routes/tests are seeded from the selected symbol's file, not a symbol-level change slice"
            .to_string(),
    );
    if affected_seed.is_none() {
        unknowns.push(
            "selected symbol has no indexed file path; affected analysis was skipped".to_string(),
        );
    }
    if direct_callers.is_empty() {
        unknowns.push("no direct callers found in the incoming trail".to_string());
    }
    if transitive_callers.is_empty() {
        unknowns.push("no transitive callers found inside the caller depth cap".to_string());
    }
    if likely_tests.is_empty() {
        unknowns.push("no test-like file reached by the affected graph walk".to_string());
    }
    if trail.trail.truncated {
        unknowns.push(format!(
            "caller trail truncated at max_nodes={max_nodes}; rerun with a narrower symbol or higher cap"
        ));
    }
    if let Some(affected) = affected {
        unknowns.extend(affected.blind_spots.iter().cloned());
    }
    unknowns.sort();
    unknowns.dedup();
    unknowns
}

fn symbol_workflow_next_commands(
    project_root: &Path,
    node_id: &NodeId,
    affected_seed: Option<&str>,
    depth: u32,
    max_nodes: u32,
    kind: SymbolWorkflowKind,
    include_tests: bool,
) -> Vec<String> {
    let project = quote_command_path(project_root);
    let id = quote_command_value(&node_id.0);
    let caller_scope_flag = if include_tests {
        " --include-tests"
    } else {
        ""
    };
    let mut commands = vec![
        format!("codestory-cli symbol --project {project} --id {id}"),
        format!(
            "codestory-cli callers --project {project} --id {id} --depth {depth} --max-nodes {max_nodes}{caller_scope_flag}"
        ),
    ];
    if let Some(path) = affected_seed {
        commands.push(format!(
            "codestory-cli affected --project {project} {} --depth {depth}",
            quote_command_value(path)
        ));
    }
    let paired = match kind {
        SymbolWorkflowKind::Impact => "test-map",
        SymbolWorkflowKind::TestMap => "impact",
    };
    commands.push(format!(
        "codestory-cli {paired} --project {project} --id {id} --depth {depth} --max-nodes {max_nodes}{caller_scope_flag}"
    ));
    commands
}

fn render_symbol_workflow_markdown(
    kind: SymbolWorkflowKind,
    output: &SymbolWorkflowOutput<'_>,
) -> String {
    let mut markdown = String::new();
    let _ = writeln!(markdown, "# {}", kind.title());
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

    append_symbol_workflow_nodes(&mut markdown, "direct_callers", &output.direct_callers);
    append_symbol_workflow_nodes(
        &mut markdown,
        "transitive_callers",
        &output.transitive_callers,
    );
    append_symbol_workflow_strings(&mut markdown, "impacted_files", &output.impacted_files);

    let _ = writeln!(markdown, "impacted_routes:");
    if output.impacted_routes.is_empty() {
        let _ = writeln!(markdown, "- none");
    } else {
        for route in &output.impacted_routes {
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
        for test in &output.likely_tests {
            let _ = writeln!(
                markdown,
                "- {} confidence={} graph_depth={} impacted_symbols={}",
                test.path, test.confidence, test.graph_depth, test.impacted_symbol_count
            );
            let _ = writeln!(markdown, "  reason: {}", test.reason);
        }
    }

    append_symbol_workflow_strings(&mut markdown, "unknowns", &output.unknowns);
    append_symbol_workflow_strings(&mut markdown, "next_commands", &output.next_commands);
    markdown
}

fn append_symbol_workflow_nodes(
    markdown: &mut String,
    label: &str,
    nodes: &[SymbolWorkflowNodeOutput],
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

fn append_symbol_workflow_strings(markdown: &mut String, label: &str, items: &[String]) {
    let _ = writeln!(markdown, "{label}:");
    if items.is_empty() {
        let _ = writeln!(markdown, "- none");
        return;
    }
    for item in items {
        let _ = writeln!(markdown, "- {item}");
    }
}

fn run_trail(cmd: TrailCommand) -> Result<()> {
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
    if cmd.mermaid {
        return emit_text(render_trail_mermaid(&context), cmd.output_file.as_deref());
    }
    if cmd.format == args::OutputFormat::Dot {
        return emit_text(
            render_trail_dot(&runtime.project_root, &context),
            cmd.output_file.as_deref(),
        );
    }
    let notes = trail_guidance_notes(&context);
    let resolution = build_query_resolution_output_with_runtime(&runtime, &target);
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
    emit(cmd.format, &output, markdown, cmd.output_file.as_deref())
}

fn run_callers(mut cmd: TrailCommand) -> Result<()> {
    cmd.mode = CliTrailMode::Referencing;
    cmd.direction = Some(CliDirection::Incoming);
    run_trail(cmd)
}

fn run_callees(mut cmd: TrailCommand) -> Result<()> {
    cmd.mode = CliTrailMode::Referenced;
    cmd.direction = Some(CliDirection::Outgoing);
    run_trail(cmd)
}

fn run_trace(mut cmd: TrailCommand) -> Result<()> {
    if !cmd.mermaid && cmd.format != args::OutputFormat::Dot {
        cmd.story = true;
    }
    run_trail(cmd)
}

fn trail_guidance_notes(context: &codestory_contracts::api::TrailContextDto) -> Vec<String> {
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

fn prefer_function_body_target(
    project_root: &std::path::Path,
    mut target: runtime::ResolvedTarget,
) -> runtime::ResolvedTarget {
    if hit_looks_like_function_body(project_root, &target.selected) {
        return target;
    }
    if !matches!(target.selected.kind, NodeKind::FUNCTION | NodeKind::METHOD) {
        return target;
    }
    let Some((index, preferred)) = target
        .alternatives
        .iter()
        .enumerate()
        .find(|(_, hit)| {
            function_body_promotion_matches(&target.selected, hit)
                && hit_looks_like_function_body(project_root, hit)
        })
        .map(|(index, hit)| (index, hit.clone()))
    else {
        return target;
    };
    target.selected = preferred;
    let promoted = target.alternatives.remove(index);
    target.alternatives.insert(0, promoted);
    target
}

fn function_body_promotion_matches(selected: &SearchHit, candidate: &SearchHit) -> bool {
    if selected.display_name == candidate.display_name {
        return true;
    }
    terminal_display_name(&selected.display_name) == terminal_display_name(&candidate.display_name)
}

fn terminal_display_name(name: &str) -> &str {
    name.rsplit_once("::")
        .map(|(_, terminal)| terminal)
        .or_else(|| name.rsplit_once('.').map(|(_, terminal)| terminal))
        .unwrap_or(name)
}

fn hit_looks_like_function_body(project_root: &std::path::Path, hit: &SearchHit) -> bool {
    if !matches!(hit.kind, NodeKind::FUNCTION | NodeKind::METHOD) {
        return false;
    }
    let Some(path) = hit.file_path.as_deref() else {
        return false;
    };
    let Some(line) = hit.line else {
        return false;
    };
    let path = std::path::Path::new(path);
    let resolved = if path.is_absolute() {
        path.to_path_buf()
    } else {
        project_root.join(path)
    };
    let Ok(contents) = fs::read_to_string(&resolved) else {
        return false;
    };
    let line_index = line.saturating_sub(1) as usize;
    let window = contents
        .lines()
        .skip(line_index)
        .take(8)
        .collect::<Vec<_>>()
        .join("\n");
    let before_body = window.split('{').next().unwrap_or(window.as_str());
    window.contains('{') && !before_body.contains(';')
}

fn run_snippet(cmd: SnippetCommand) -> Result<()> {
    ensure_dot_only_for_trail(cmd.format, "snippet")?;
    preflight_output_file(cmd.output_file.as_deref())?;
    let runtime = RuntimeContext::new(&cmd.project)?;
    let opened = runtime.ensure_open(cmd.refresh)?;
    ensure_index_ready(&opened, "snippet")?;

    let file_filter = cmd.target.file_filter();
    let target = resolve_target_or_emit_ambiguity(
        &runtime,
        cmd.target.selection()?,
        file_filter.as_deref(),
        cmd.format,
        cmd.output_file.as_deref(),
    )?;
    let target = if cmd.function_body {
        prefer_function_body_target(&runtime.project_root, target)
    } else {
        target
    };
    let context = if cmd.function_body {
        runtime
            .browser
            .snippet_function_body_context(target.selected.node_id.clone(), cmd.context)
    } else {
        runtime
            .browser
            .snippet_context(target.selected.node_id.clone(), cmd.context)
    }
    .map_err(map_api_error)?;
    let colorize = cmd.format == args::OutputFormat::Markdown
        && cmd.output_file.is_none()
        && std::io::stdout().is_terminal();
    let resolution = build_query_resolution_output_with_runtime(&runtime, &target);
    let verification_targets = resolution.resolved.verification_targets.clone();
    let markdown = render_snippet_markdown(
        &runtime.project_root,
        &target,
        &context,
        colorize,
        &verification_targets,
    );
    let output = SnippetJsonOutput {
        resolution,
        snippet: &context,
        verification_targets,
    };
    emit(cmd.format, &output, markdown, cmd.output_file.as_deref())
}

fn run_query(cmd: QueryCommand) -> Result<()> {
    ensure_dot_only_for_trail(cmd.format, "query")?;
    preflight_output_file(cmd.output_file.as_deref())?;
    if let Some(sql) = cmd.sql.as_deref() {
        bail!(
            "CodeStory `query` uses the graph-query DSL, not SQL. \
             Use syntax like `search(query: 'AppController') | limit(5)` or \
             `trail(symbol: 'AppController') | filter(kind: function)`. \
             For raw symbol discovery, use `search --query {}`. \
             Unsupported SQL received: {}",
            quote_command_value(sql),
            sql
        );
    }
    let query = cmd
        .query
        .as_deref()
        .map(str::trim)
        .filter(|query| !query.is_empty())
        .context("Query cannot be empty.")?;
    let ast =
        codestory_runtime::parse_graph_query(query).map_err(|error| anyhow::anyhow!("{error}"))?;
    let runtime = RuntimeContext::new(&cmd.project)?;
    let opened = runtime.ensure_open(cmd.refresh)?;
    ensure_index_ready(&opened, "query")?;
    let items = runtime
        .browser
        .query(&ast)
        .map_err(map_api_error)?
        .iter()
        .map(|item| explore::browser_query_item_to_output(&runtime.project_root, item))
        .collect();
    let output = QueryOutput {
        query: query.to_string(),
        ast,
        items,
    };
    let markdown = render_query_markdown(&output);
    emit(cmd.format, &output, markdown, cmd.output_file.as_deref())
}

fn run_files(cmd: FilesCommand) -> Result<()> {
    ensure_dot_only_for_trail(cmd.format, "files")?;
    preflight_output_file(cmd.output_file.as_deref())?;
    let project = ProjectArgs {
        project: cmd.project.clone(),
        cache_dir: cmd.cache_dir.clone(),
    };
    let runtime = RuntimeContext::new(&project)?;
    let opened = runtime.ensure_open(cmd.refresh)?;
    ensure_index_ready(&opened, "files")?;
    let output = runtime
        .browser
        .indexed_files(IndexedFilesRequest {
            path_contains: cmd.path,
            language: cmd.language,
            role: cmd.role.map(Into::into),
            limit: Some(cmd.limit),
        })
        .map_err(map_api_error)?;
    let markdown = render_files_markdown(&output);
    emit(cmd.format, &output, markdown, cmd.output_file.as_deref())
}

fn run_affected(cmd: AffectedCommand) -> Result<()> {
    ensure_dot_only_for_trail(cmd.format, "affected")?;
    preflight_output_file(cmd.output_file.as_deref())?;
    let runtime = RuntimeContext::new(&cmd.project)?;
    let opened = runtime.ensure_open(cmd.refresh)?;
    ensure_index_ready(&opened, "affected")?;
    let change_records = affected_change_records(&cmd)?;
    let changed_paths = change_records
        .iter()
        .map(|record| record.path.clone())
        .collect::<Vec<_>>();
    let output = runtime
        .browser
        .affected_analysis(AffectedAnalysisRequest {
            changed_paths,
            change_records,
            depth: Some(cmd.depth),
            filter: cmd.filter,
        })
        .map_err(map_api_error)?;
    let markdown = render_affected_markdown(&output);
    emit(cmd.format, &output, markdown, cmd.output_file.as_deref())
}

fn affected_change_records(cmd: &AffectedCommand) -> Result<Vec<AffectedChangeRecordDto>> {
    let mut records = cmd
        .paths
        .iter()
        .map(|path| affected_path_record(path, AffectedChangeKindDto::Unknown, "path"))
        .collect::<Vec<_>>();
    if cmd.stdin {
        let mut input = String::new();
        std::io::stdin()
            .read_to_string(&mut input)
            .context("Failed to read changed paths from stdin")?;
        match cmd.stdin_format {
            AffectedStdinFormat::Path => records.extend(
                input
                    .lines()
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .map(|path| {
                        affected_path_record(path, AffectedChangeKindDto::Unknown, "stdin")
                    }),
            ),
            AffectedStdinFormat::NameStatus => {
                records.extend(parse_git_name_status_records(&input)?);
            }
        }
    }
    if !records.is_empty() {
        dedupe_affected_change_records(&mut records);
        return Ok(records);
    }
    let output = affected_git_change_output(cmd)?;
    if !output.status.success() {
        bail!(
            "git change discovery failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut records = match cmd.changes {
        AffectedChangeSource::Untracked => stdout
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(|path| affected_path_record(path, AffectedChangeKindDto::Untracked, "??"))
            .collect::<Vec<_>>(),
        AffectedChangeSource::Head
        | AffectedChangeSource::Staged
        | AffectedChangeSource::Unstaged => parse_git_name_status_records(&stdout)?,
    };
    dedupe_affected_change_records(&mut records);
    Ok(records)
}

fn affected_git_change_output(cmd: &AffectedCommand) -> Result<std::process::Output> {
    let mut command = std::process::Command::new("git");
    command.arg("-C").arg(&cmd.project.project);
    match cmd.changes {
        AffectedChangeSource::Head => {
            command.arg("diff").arg("--name-status").arg("HEAD");
        }
        AffectedChangeSource::Staged => {
            command.arg("diff").arg("--cached").arg("--name-status");
        }
        AffectedChangeSource::Unstaged => {
            command.arg("diff").arg("--name-status");
        }
        AffectedChangeSource::Untracked => {
            command
                .arg("ls-files")
                .arg("--others")
                .arg("--exclude-standard");
        }
    }
    command
        .output()
        .context("Failed to run git change discovery")
}

fn parse_git_name_status_records(input: &str) -> Result<Vec<AffectedChangeRecordDto>> {
    input
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(parse_git_name_status_record)
        .collect()
}

fn parse_git_name_status_record(line: &str) -> Result<AffectedChangeRecordDto> {
    let parts = line.split('\t').collect::<Vec<_>>();
    if parts.len() == 1 {
        return Ok(affected_path_record(
            parts[0],
            AffectedChangeKindDto::Unknown,
            "path",
        ));
    }
    let status = parts[0].trim();
    let kind = affected_change_kind_from_status(status);
    let (previous_path, path) = if matches!(
        kind,
        AffectedChangeKindDto::Renamed | AffectedChangeKindDto::Copied
    ) {
        let previous = parts
            .get(1)
            .map(|path| path.trim())
            .filter(|path| !path.is_empty())
            .context("git name-status rename/copy row is missing the previous path")?;
        let current = parts
            .get(2)
            .map(|path| path.trim())
            .filter(|path| !path.is_empty())
            .context("git name-status rename/copy row is missing the current path")?;
        (Some(previous.to_string()), current)
    } else {
        let path = parts
            .get(1)
            .map(|path| path.trim())
            .filter(|path| !path.is_empty())
            .context("git name-status row is missing the path")?;
        (None, path)
    };
    Ok(AffectedChangeRecordDto {
        path: path.to_string(),
        kind,
        status: status.to_string(),
        previous_path,
    })
}

fn affected_path_record(
    path: &str,
    kind: AffectedChangeKindDto,
    status: &str,
) -> AffectedChangeRecordDto {
    AffectedChangeRecordDto {
        path: path.trim().to_string(),
        kind,
        status: status.to_string(),
        previous_path: None,
    }
}

fn affected_change_kind_from_status(status: &str) -> AffectedChangeKindDto {
    match status.chars().next().unwrap_or_default() {
        'A' => AffectedChangeKindDto::Added,
        'M' | 'T' | 'U' => AffectedChangeKindDto::Modified,
        'D' => AffectedChangeKindDto::Deleted,
        'R' => AffectedChangeKindDto::Renamed,
        'C' => AffectedChangeKindDto::Copied,
        '?' => AffectedChangeKindDto::Untracked,
        _ => AffectedChangeKindDto::Unknown,
    }
}

fn dedupe_affected_change_records(records: &mut Vec<AffectedChangeRecordDto>) {
    records.retain(|record| !record.path.trim().is_empty());
    records.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then(left.previous_path.cmp(&right.previous_path))
            .then(left.status.cmp(&right.status))
    });
    records.dedup_by(|left, right| {
        left.path == right.path
            && left.previous_path == right.previous_path
            && left.status == right.status
    });
}

fn render_files_markdown(output: &codestory_contracts::api::IndexedFilesDto) -> String {
    let mut markdown = String::new();
    markdown.push_str("# indexed files\n\n");
    render_files_summary(&mut markdown, output);
    render_framework_route_coverage(&mut markdown, output);
    render_indexed_file_rows(&mut markdown, output);
    markdown
}

fn render_files_summary(markdown: &mut String, output: &codestory_contracts::api::IndexedFilesDto) {
    let status = if output.usable { "usable" } else { "empty" };
    let _ = writeln!(
        markdown,
        "- index: {status}; whole index files: {}; indexed: {}; incomplete: {}; error files: {}; filtered files: {}; visible rows: {}; truncated: {}",
        output.summary.file_count,
        output.summary.indexed_file_count,
        output.summary.incomplete_file_count,
        output.summary.error_file_count,
        output.summary.filtered_file_count,
        output.summary.visible_file_count,
        output.summary.truncated
    );
    if !output.summary.language_counts.is_empty() {
        let languages = output
            .summary
            .language_counts
            .iter()
            .map(|entry| {
                format!(
                    "{}={} [{}; {}]",
                    entry.language, entry.file_count, entry.support_mode, entry.evidence_tier
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(markdown, "- languages: {languages}");
        let claim_labels = output
            .summary
            .language_counts
            .iter()
            .map(|entry| format!("{}={}", entry.language, entry.claim_label))
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(markdown, "- language_support_claims: {claim_labels}");
    }
    if !output.summary.incomplete_reason_counts.is_empty() {
        let reasons = output
            .summary
            .incomplete_reason_counts
            .iter()
            .map(|entry| format!("{}={} ({})", entry.reason, entry.file_count, entry.detail))
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(markdown, "- incomplete_reasons: {reasons}");
    }
    for note in &output.summary.coverage_notes {
        let _ = writeln!(markdown, "- coverage: {note}");
    }
}

fn render_framework_route_coverage(
    markdown: &mut String,
    output: &codestory_contracts::api::IndexedFilesDto,
) {
    if !output.summary.framework_route_coverage.is_empty() {
        markdown.push_str("\nframework route coverage:\n");
        for entry in &output.summary.framework_route_coverage {
            let _ = writeln!(markdown, "{}", framework_route_coverage_row(entry));
        }
    }
}

fn framework_route_coverage_row(entry: &FrameworkRouteCoverageDto) -> String {
    format!(
        "- {} ({}) status={} coverage_evidence={} confidence_floor={} handler_link={} promotable={} unsupported={} known_gaps={}",
        entry.framework,
        entry.language,
        entry.status,
        entry.coverage_evidence,
        entry.confidence_floor,
        entry.handler_link_support,
        entry.promotable,
        joined_or_none_recorded(&entry.unsupported_patterns),
        joined_or_none_recorded(&entry.known_gaps)
    )
}

fn joined_or_none_recorded(values: &[String]) -> String {
    if values.is_empty() {
        "none recorded".to_string()
    } else {
        values.join("; ")
    }
}

fn render_indexed_file_rows(
    markdown: &mut String,
    output: &codestory_contracts::api::IndexedFilesDto,
) {
    markdown.push_str("\nfiles:\n");
    for file in &output.files {
        let markers = [
            (!file.indexed).then_some("not-indexed"),
            (!file.complete).then_some("incomplete"),
            (file.error_count > 0).then_some("errors"),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
        let marker = if markers.is_empty() {
            String::new()
        } else {
            format!(" [{}]", markers.join(", "))
        };
        let _ = writeln!(
            markdown,
            "- {} ({}, {:?}, {} lines){}",
            file.path, file.language, file.role, file.line_count, marker
        );
    }
    if output.summary.truncated {
        markdown.push_str("- ... truncated by limit\n");
    }
}

fn render_affected_markdown(output: &codestory_contracts::api::AffectedAnalysisDto) -> String {
    let mut markdown = String::new();
    markdown.push_str("# affected analysis\n\n");
    render_affected_summary(&mut markdown, output);
    render_affected_matched_files(&mut markdown, output);
    render_affected_routes(&mut markdown, output);
    render_affected_tests(&mut markdown, output);
    render_affected_symbols(&mut markdown, output);
    render_affected_footer(&mut markdown, output);
    markdown
}

fn render_affected_summary(
    markdown: &mut String,
    output: &codestory_contracts::api::AffectedAnalysisDto,
) {
    let _ = writeln!(
        markdown,
        "- matched files: {}; depth: {}; impacted symbols: {}; impacted routes: {}; impacted tests: {}",
        output.matched_file_count,
        output.depth,
        output.impacted_symbols.len(),
        output.impacted_routes.len(),
        output.impacted_tests.len()
    );
    if !output.changed_paths.is_empty() {
        markdown.push_str("- changed paths:\n");
        for path in &output.changed_paths {
            let _ = writeln!(markdown, "  - {path}");
        }
    }
    if !output.change_records.is_empty() {
        markdown.push_str("- change records:\n");
        for record in &output.change_records {
            let previous = record
                .previous_path
                .as_deref()
                .map(|path| format!(" previous={path}"))
                .unwrap_or_default();
            let _ = writeln!(
                markdown,
                "  - {:?} {} status={}{}",
                record.kind, record.path, record.status, previous
            );
        }
    }
    for note in &output.notes {
        let _ = writeln!(markdown, "- note: {note}");
    }
}

fn render_affected_matched_files(
    markdown: &mut String,
    output: &codestory_contracts::api::AffectedAnalysisDto,
) {
    if !output.matched_files.is_empty() {
        markdown.push_str("\nmatched files:\n");
        for file in &output.matched_files {
            let mut markers = Vec::new();
            if !file.complete {
                markers.push("incomplete".to_string());
            }
            if file.error_count > 0 {
                markers.push(format!("errors={}", file.error_count));
            }
            if let Some(kind) = file.change_kind.as_ref() {
                markers.push(format!("change={kind:?}"));
            }
            if let Some(status) = file.change_status.as_deref() {
                markers.push(format!("status={status}"));
            }
            if let Some(previous_path) = file.previous_path.as_deref() {
                markers.push(format!("previous={previous_path}"));
            }
            let marker = if markers.is_empty() {
                String::new()
            } else {
                format!(" ({})", markers.join(", "))
            };
            let _ = writeln!(markdown, "- {} [{:?}]{marker}", file.path, file.role);
        }
    }
    if !output.unmatched_paths.is_empty() {
        markdown.push_str("\nunmatched paths:\n");
        for path in &output.unmatched_paths {
            let mut markers = Vec::new();
            if let Some(kind) = path.change_kind.as_ref() {
                markers.push(format!("change={kind:?}"));
            }
            if let Some(status) = path.change_status.as_deref() {
                markers.push(format!("status={status}"));
            }
            if let Some(previous_path) = path.previous_path.as_deref() {
                markers.push(format!("previous={previous_path}"));
            }
            let marker = if markers.is_empty() {
                String::new()
            } else {
                format!(" ({})", markers.join(", "))
            };
            let _ = writeln!(markdown, "- {}{marker}: {}", path.path, path.reason);
        }
    }
}

fn render_affected_routes(
    markdown: &mut String,
    output: &codestory_contracts::api::AffectedAnalysisDto,
) {
    if !output.impacted_routes.is_empty() {
        markdown.push_str("\nimpacted routes:\n");
        for route in output.impacted_routes.iter().take(30) {
            let handler = route
                .route
                .handler
                .as_ref()
                .map(|handler| format!(" handler={}", handler.display_name))
                .unwrap_or_default();
            let framework = route
                .route
                .framework
                .as_deref()
                .map(|framework| format!(" framework={framework}"))
                .unwrap_or_default();
            let _ = writeln!(
                markdown,
                "- d{} {} {}{}{} [{}]: {}",
                route.graph_depth,
                route.route.method,
                route.route.path,
                framework,
                handler,
                route.confidence,
                route.reason
            );
        }
    }
}

fn render_affected_tests(
    markdown: &mut String,
    output: &codestory_contracts::api::AffectedAnalysisDto,
) {
    if !output.impacted_tests.is_empty() {
        markdown.push_str("\nlikely impacted tests:\n");
        for test in &output.impacted_tests {
            let _ = writeln!(
                markdown,
                "- d{} {} ({} symbols, {}): {}",
                test.graph_depth,
                test.path,
                test.impacted_symbol_count,
                test.confidence,
                test.reason
            );
        }
    }
}

fn render_affected_symbols(
    markdown: &mut String,
    output: &codestory_contracts::api::AffectedAnalysisDto,
) {
    markdown.push_str("\nimpacted symbols:\n");
    for symbol in output.impacted_symbols.iter().take(40) {
        let location = symbol
            .file_path
            .as_deref()
            .map(|path| match symbol.line {
                Some(line) => format!("{path}:{line}"),
                None => path.to_string(),
            })
            .unwrap_or_else(|| "unknown".to_string());
        let _ = writeln!(
            markdown,
            "- d{} {} [{:?}] at {} ({}, {}): {}",
            symbol.graph_depth,
            symbol.display_name,
            symbol.kind,
            location,
            symbol.node_id.0,
            symbol.confidence,
            symbol.reason
        );
    }
    if output.impacted_symbols.len() > 40 {
        let _ = writeln!(
            markdown,
            "- ... {} more symbols omitted",
            output.impacted_symbols.len() - 40
        );
    }
}

fn render_affected_footer(
    markdown: &mut String,
    output: &codestory_contracts::api::AffectedAnalysisDto,
) {
    if !output.blind_spots.is_empty() {
        markdown.push_str("\nblind spots:\n");
        for blind_spot in &output.blind_spots {
            let _ = writeln!(markdown, "- {blind_spot}");
        }
    }
    if !output.next_commands.is_empty() {
        markdown.push_str("\nnext_commands:\n");
        for command in &output.next_commands {
            let _ = writeln!(markdown, "- `{command}`");
        }
    }
}

fn run_serve(cmd: ServeCommand) -> Result<()> {
    let runtime = new_agent_surface_runtime(&cmd.project)?;
    let opened = runtime.ensure_open(cmd.refresh)?;
    ensure_index_ready(&opened, "serve")?;
    if cmd.stdio {
        return stdio_transport::run_stdio_server(runtime);
    }
    let listener = TcpListener::bind(&cmd.addr)
        .with_context(|| format!("Failed to bind server to {}", cmd.addr))?;
    eprintln!("codestory serve listening on http://{}", cmd.addr);
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                if let Err(error) = http_transport::handle_http_request(&runtime, stream) {
                    eprintln!("serve request failed: {error:#}");
                }
            }
            Err(error) => eprintln!("serve accept failed: {error}"),
        }
    }
    Ok(())
}

fn run_generate_completions(cmd: GenerateCompletionsCommand) -> Result<()> {
    let shell = match cmd.shell {
        CompletionShell::Bash => Shell::Bash,
        CompletionShell::Zsh => Shell::Zsh,
        CompletionShell::Fish => Shell::Fish,
        CompletionShell::Powershell => Shell::PowerShell,
    };
    let mut command = Cli::command();
    generate(shell, &mut command, "codestory-cli", &mut std::io::stdout());
    Ok(())
}

#[derive(serde::Serialize)]
struct CliErrorOutput {
    error: CliErrorBody,
}

#[derive(serde::Serialize)]
struct CliErrorBody {
    code: &'static str,
    failed_layer: &'static str,
    message: String,
    query: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    file_filter: Option<String>,
    alternatives: Vec<SearchHitOutput>,
    layer_notes: Vec<String>,
    next_commands: Vec<String>,
}

const CLI_ERROR_MARKDOWN_ALTERNATIVE_LIMIT: usize = 10;

fn resolve_target_or_emit_ambiguity(
    runtime: &RuntimeContext,
    target: args::TargetSelection,
    file_filter: Option<&str>,
    format: args::OutputFormat,
    output_file: Option<&std::path::Path>,
) -> Result<runtime::ResolvedTarget> {
    match resolve_target(runtime, target, file_filter) {
        Ok(target) => Ok(target),
        Err(error) => {
            if let Some(ambiguous) = error.downcast_ref::<AmbiguousTargetError>() {
                let output = build_ambiguous_target_error_output(&runtime.project_root, ambiguous);
                if output_file.is_some() || format == args::OutputFormat::Json {
                    emit(
                        format,
                        &output,
                        render_cli_error_markdown(&output),
                        output_file,
                    )?;
                }
            }
            Err(error)
        }
    }
}

fn render_cli_error_markdown(output: &CliErrorOutput) -> String {
    let mut markdown = String::new();
    let _ = writeln!(markdown, "# Command Error");
    let _ = writeln!(markdown, "code: {}", output.error.code);
    let _ = writeln!(markdown, "failed_layer: {}", output.error.failed_layer);
    let _ = writeln!(markdown, "message: {}", cli_error_markdown_message(output));
    let _ = writeln!(markdown, "query: `{}`", output.error.query);
    if let Some(file_filter) = output.error.file_filter.as_deref() {
        let _ = writeln!(markdown, "file_filter: `{file_filter}`");
    }
    if !output.error.next_commands.is_empty() {
        let _ = writeln!(markdown, "next_commands:");
        for command in &output.error.next_commands {
            let _ = writeln!(markdown, "- `{command}`");
        }
    }
    let _ = writeln!(
        markdown,
        "alternatives: {}",
        output.error.alternatives.len()
    );
    if output.error.alternatives.len() > CLI_ERROR_MARKDOWN_ALTERNATIVE_LIMIT {
        let _ = writeln!(
            markdown,
            "showing: {} of {}; use `--format json` or `search` to inspect all alternatives",
            CLI_ERROR_MARKDOWN_ALTERNATIVE_LIMIT,
            output.error.alternatives.len()
        );
    }
    for alternative in output
        .error
        .alternatives
        .iter()
        .take(CLI_ERROR_MARKDOWN_ALTERNATIVE_LIMIT)
    {
        let location = alternative
            .file_path
            .as_deref()
            .map(|path| {
                alternative
                    .line
                    .map(|line| format!(" {path}:{line}"))
                    .unwrap_or_else(|| format!(" {path}"))
            })
            .unwrap_or_default();
        let _ = writeln!(
            markdown,
            "- [{}] {} [{}]{} score={:.2} match={}",
            alternative.node_id,
            alternative.display_name,
            display::format_kind(alternative.kind),
            location,
            alternative.score,
            match alternative.match_quality {
                SearchMatchQualityDto::Exact => "exact",
                SearchMatchQualityDto::NormalizedExact => "normalized_exact",
                SearchMatchQualityDto::Prefix => "prefix",
                SearchMatchQualityDto::Fuzzy => "fuzzy",
                SearchMatchQualityDto::SemanticSuggestion => "semantic_suggestion",
                SearchMatchQualityDto::RepoText => "repo_text",
            }
        );
    }
    if !output.error.layer_notes.is_empty() {
        let _ = writeln!(markdown, "layer_notes:");
        for note in &output.error.layer_notes {
            let _ = writeln!(markdown, "- {note}");
        }
    }
    markdown
}

fn cli_error_markdown_message(output: &CliErrorOutput) -> &str {
    if output.error.code == "ambiguous_target" {
        output
            .error
            .message
            .lines()
            .next()
            .unwrap_or(&output.error.message)
    } else {
        &output.error.message
    }
}

fn build_ambiguous_target_error_output(
    project_root: &std::path::Path,
    ambiguous: &AmbiguousTargetError,
) -> CliErrorOutput {
    let alternatives = ambiguous
        .alternatives
        .iter()
        .enumerate()
        .map(|(index, hit)| {
            build_numbered_search_hit_output(project_root, hit, Some(&ambiguous.query), index + 1)
        })
        .collect::<Vec<_>>();
    let project = quote_command_path(project_root);
    let file_clause = ambiguous
        .file_filter
        .as_deref()
        .map(|file_filter| format!(" --file {}", quote_command_argument_value(file_filter)))
        .unwrap_or_default();
    let mut next_commands = vec![format!(
        "codestory-cli symbol --project {project} --query {}{} --choose 1",
        quote_command_argument_value(&ambiguous.query),
        file_clause
    )];
    if let Some(first) = ambiguous.alternatives.first() {
        next_commands.push(format!(
            "codestory-cli symbol --project {project} --id {}",
            first.node_id.0
        ));
        if let Some(path) = first.file_path.as_deref() {
            next_commands.push(format!(
                "codestory-cli symbol --project {project} --query {} --file {}",
                quote_command_argument_value(&ambiguous.query),
                quote_command_argument_value(&crate::display::relative_path(project_root, path))
            ));
        }
    }

    CliErrorOutput {
        error: CliErrorBody {
            code: "ambiguous_target",
            failed_layer: "query_resolution",
            message: ambiguous.message.clone(),
            query: ambiguous.query.clone(),
            file_filter: ambiguous
                .file_filter
                .as_deref()
                .map(crate::display::clean_path_string),
            alternatives,
            layer_notes: vec![
                format!(
                    "query_resolution: `{}` matched multiple equally ranked symbols",
                    ambiguous.query
                ),
                format!(
                    "search: inspect alternatives with `codestory-cli search --project {project} --query {}`, then rerun this command with --choose, --id, or --file",
                    quote_command_argument_value(&ambiguous.query)
                ),
            ],
            next_commands,
        },
    }
}

fn build_doctor_output(
    runtime: &RuntimeContext,
    summary: &codestory_contracts::api::ProjectSummary,
) -> DoctorOutput {
    let indexed = summary.stats.node_count > 0;
    let mut retrieval = summary.retrieval.clone();
    if let Some(retrieval) = retrieval.as_mut()
        && let Some(message) = retrieval.fallback_message.as_mut()
    {
        *message = redact_urls_in_text(message);
    }
    let project = display::clean_path_string(&summary.root);
    let storage_path = display::clean_path_string(&runtime.storage_path.to_string_lossy());
    let storage_exists = runtime.storage_path.exists();
    let sidecar_retrieval = doctor_sidecar_status(runtime);
    let readiness = build_summary_readiness(
        &project,
        &summary.stats,
        summary.freshness.as_ref(),
        &sidecar_retrieval,
    );
    let readiness_lanes = build_readiness_lanes_for_runtime(runtime, &readiness);
    let next_commands = readiness::compatibility_next_commands(&readiness);
    let mut checks = Vec::new();
    checks.push(doctor_check(
        "project",
        "ok",
        format!("Project root resolved to `{project}`."),
    ));
    checks.push(if storage_exists {
        doctor_check(
            "cache",
            "ok",
            format!("Cache database exists at `{storage_path}`."),
        )
    } else {
        doctor_check(
            "cache",
            "warn",
            "Cache database does not exist yet; run `codestory-cli index --refresh full`."
                .to_string(),
        )
    });
    checks.push(if indexed {
        doctor_check(
            "index",
            "ok",
            format!(
                "Indexed {} files, {} nodes, {} edges.",
                summary.stats.file_count, summary.stats.node_count, summary.stats.edge_count
            ),
        )
    } else {
        doctor_check(
            "index",
            "warn",
            "No indexed symbols are available yet.".to_string(),
        )
    });
    checks.push(doctor_sidecar_check(&sidecar_retrieval));
    if let Some(retrieval) = retrieval.as_ref() {
        checks.push(semantic_health_check(retrieval, &summary.stats));
        if retrieval.stored_embedding.is_some() {
            checks.push(semantic_contract_check(retrieval));
        }
    }
    let managed_status = managed_embeddings::inspect_status(&runtime.managed_embeddings_root);
    checks.push(doctor_check(
        "managed_embeddings",
        managed_doctor_status(&managed_status.state),
        managed_status.message,
    ));
    if let Some(freshness) = summary.freshness.as_ref() {
        checks.push(index_freshness_check(freshness));
    }

    let environment = [
        "CODESTORY_EMBED_PROFILE",
        "CODESTORY_EMBED_MODEL_ID",
        "CODESTORY_EMBED_BACKEND",
        "CODESTORY_EMBED_RUNTIME_MODE",
        "CODESTORY_EMBED_ONNX_MODEL",
        "CODESTORY_EMBED_ONNX_TOKENIZER",
        "CODESTORY_EMBED_ONNX_PROVIDER",
        "CODESTORY_EMBED_ONNX_BATCH_TOKENS",
        "CODESTORY_EMBED_ONNX_THREADS",
        "CODESTORY_EMBED_LLAMACPP_URL",
        "CODESTORY_EMBED_LLAMACPP_REQUEST_COUNT",
        "CODESTORY_STORED_VECTOR_ENCODING",
        "CODESTORY_HYBRID_RETRIEVAL_ENABLED",
        "CODESTORY_SEMANTIC_DOC_ALIAS_MODE",
    ]
    .into_iter()
    .map(|name| match std::env::var(name) {
        Ok(value) if !value.trim().is_empty() => {
            doctor_check(name, "ok", doctor_env_check_message(name, &value))
        }
        _ => doctor_check(name, "info", "not set; using runtime defaults".to_string()),
    })
    .collect::<Vec<_>>();

    DoctorOutput {
        project: project.clone(),
        storage_path,
        indexed,
        stats: summary.stats.clone(),
        retrieval_mode: sidecar_retrieval.retrieval_mode.clone(),
        degraded_reason: sidecar_retrieval.degraded_reason.clone(),
        sidecar_retrieval,
        retrieval,
        freshness: summary.freshness.clone(),
        readiness,
        readiness_lanes,
        checks,
        next_commands,
        environment,
    }
}

fn build_summary_readiness(
    project: &str,
    stats: &codestory_contracts::api::StorageStatsDto,
    freshness: Option<&IndexFreshnessDto>,
    sidecar: &DoctorSidecarStatusOutput,
) -> Vec<codestory_contracts::api::ReadinessVerdictDto> {
    readiness::build_readiness_verdicts(readiness::ReadinessInputs {
        project,
        stats,
        freshness,
        setup: None,
        sidecar: Some(readiness_sidecar_input(sidecar)),
    })
}

fn readiness_sidecar_input(
    sidecar: &DoctorSidecarStatusOutput,
) -> readiness::ReadinessSidecarInput<'_> {
    readiness::ReadinessSidecarInput {
        profile: sidecar.profile.as_deref(),
        run_id: sidecar.run_id.as_deref(),
        retrieval_mode: sidecar.retrieval_mode.as_str(),
        degraded_reason: sidecar.degraded_reason.as_deref(),
        manifest_generation: sidecar.manifest_generation.as_deref(),
        manifest_input_hash: sidecar.manifest_input_hash.as_deref(),
    }
}

fn doctor_sidecar_status(runtime: &RuntimeContext) -> DoctorSidecarStatusOutput {
    let sidecar = codestory_retrieval::sidecar_runtime_auto(&runtime.project_root);
    match codestory_retrieval::strict_sidecar_status_for_runtime(
        &runtime.project_root,
        Some(&runtime.storage_path),
        sidecar.clone(),
    ) {
        Ok(report) => {
            let status = doctor_sidecar_status_from_report(report, Some(&sidecar));
            let handoff_failure = (status.retrieval_mode == "full")
                .then(|| doctor_sidecar_profile_handoff_failure(runtime))
                .flatten();
            apply_sidecar_profile_handoff(status, handoff_failure)
        }
        Err(error) => doctor_sidecar_status_error(error, Some(&sidecar)),
    }
}

fn ready_sidecar_status(
    runtime: &RuntimeContext,
    repaired_sidecar: Option<codestory_retrieval::SidecarRuntimeConfig>,
) -> DoctorSidecarStatusOutput {
    if let Some(sidecar) = repaired_sidecar {
        return doctor_sidecar_status_for_runtime(runtime, sidecar);
    }
    doctor_sidecar_status(runtime)
}

fn doctor_sidecar_status_for_runtime(
    runtime: &RuntimeContext,
    sidecar: codestory_retrieval::SidecarRuntimeConfig,
) -> DoctorSidecarStatusOutput {
    match codestory_retrieval::strict_sidecar_status_for_runtime(
        &runtime.project_root,
        Some(&runtime.storage_path),
        sidecar.clone(),
    ) {
        Ok(report) => doctor_sidecar_status_from_report(report, Some(&sidecar)),
        Err(error) => doctor_sidecar_status_error(error, Some(&sidecar)),
    }
}

fn doctor_sidecar_status_from_report(
    report: codestory_retrieval::RetrievalStatusReport,
    runtime: Option<&codestory_retrieval::SidecarRuntimeConfig>,
) -> DoctorSidecarStatusOutput {
    let manifest_generation = report
        .manifest
        .as_ref()
        .and_then(|manifest| manifest.sidecar_generation.clone());
    let manifest_input_hash = report
        .manifest
        .as_ref()
        .and_then(|manifest| manifest.sidecar_input_hash.clone());
    let precise_semantic_import_status = report
        .manifest
        .as_ref()
        .and_then(|manifest| manifest.precise_semantic_import_status.clone());
    let precise_semantic_import_reason = report
        .manifest
        .as_ref()
        .and_then(|manifest| manifest.precise_semantic_import_reason.clone());
    let precise_semantic_import_revision = report
        .manifest
        .as_ref()
        .and_then(|manifest| manifest.precise_semantic_import_revision.clone());
    let precise_semantic_import_producer = report
        .manifest
        .as_ref()
        .and_then(|manifest| manifest.precise_semantic_import_producer.clone());
    DoctorSidecarStatusOutput {
        profile: runtime
            .map(|runtime| runtime.profile.as_str().to_string())
            .or_else(|| {
                report
                    .ownership
                    .as_ref()
                    .map(|ownership| ownership.profile.clone())
            }),
        run_id: runtime.and_then(|runtime| runtime.run_id.clone()),
        retrieval_mode: report.retrieval_mode,
        degraded_reason: report.degraded_reason,
        manifest_generation,
        manifest_input_hash,
        precise_semantic_import_status,
        precise_semantic_import_reason,
        precise_semantic_import_revision,
        precise_semantic_import_producer,
    }
}

fn doctor_sidecar_status_error(
    error: anyhow::Error,
    runtime: Option<&codestory_retrieval::SidecarRuntimeConfig>,
) -> DoctorSidecarStatusOutput {
    DoctorSidecarStatusOutput {
        profile: runtime.map(|runtime| runtime.profile.as_str().to_string()),
        run_id: runtime.and_then(|runtime| runtime.run_id.clone()),
        retrieval_mode: "unavailable".to_string(),
        degraded_reason: Some(format!("sidecar_status_error: {error}")),
        manifest_generation: None,
        manifest_input_hash: None,
        precise_semantic_import_status: None,
        precise_semantic_import_reason: None,
        precise_semantic_import_revision: None,
        precise_semantic_import_producer: None,
    }
}

pub(crate) fn build_readiness_lanes_for_runtime(
    runtime: &RuntimeContext,
    readiness: &[codestory_contracts::api::ReadinessVerdictDto],
) -> BTreeMap<String, ReadinessLaneOutput> {
    let project = display::clean_path_string(&runtime.project_root.to_string_lossy());
    let project_arg = display::quote_command_argument_value(&project);
    let local_runtime = codestory_retrieval::sidecar_runtime_for_project(
        &runtime.project_root,
        codestory_retrieval::SidecarProfile::Local,
    );
    let agent_runtime = agent_readiness_sidecar_runtime(&runtime.project_root);
    let local_status = doctor_sidecar_status_for_runtime(runtime, local_runtime);
    let agent_status = doctor_sidecar_status_for_runtime(runtime, agent_runtime);
    let agent_verdict = readiness
        .iter()
        .find(|verdict| verdict.goal == ReadinessGoalDto::AgentPacketSearch);
    let mut lanes = BTreeMap::new();
    lanes.insert(
        "local_default".to_string(),
        readiness_lane_output("local_default", &local_status, None, &project_arg),
    );
    lanes.insert(
        "agent_packet_search".to_string(),
        readiness_lane_output(
            "agent_packet_search",
            &agent_status,
            agent_verdict,
            &project_arg,
        ),
    );
    lanes
}

fn agent_readiness_sidecar_runtime(
    project_root: &Path,
) -> codestory_retrieval::SidecarRuntimeConfig {
    let active = codestory_retrieval::sidecar_runtime_auto(project_root);
    if active.profile == codestory_retrieval::SidecarProfile::Agent {
        return active;
    }
    codestory_retrieval::sidecar_runtime_for_project_with_run_id(
        project_root,
        codestory_retrieval::SidecarProfile::Agent,
        Some(AGENT_RUN_MISSING_ID),
    )
}

fn readiness_lane_output(
    lane: &str,
    sidecar: &DoctorSidecarStatusOutput,
    verdict: Option<&codestory_contracts::api::ReadinessVerdictDto>,
    project_arg: &str,
) -> ReadinessLaneOutput {
    let status = readiness_lane_status(sidecar, verdict);
    ReadinessLaneOutput {
        status,
        profile: sidecar
            .profile
            .clone()
            .unwrap_or_else(|| "unknown".to_string()),
        run_id: sidecar.run_id.clone(),
        sidecar_mode: sidecar.retrieval_mode.clone(),
        degraded_reason: sidecar.degraded_reason.clone(),
        next_command: lane_next_command(lane, sidecar, status, verdict, project_arg),
    }
}

fn readiness_lane_status(
    sidecar: &DoctorSidecarStatusOutput,
    verdict: Option<&codestory_contracts::api::ReadinessVerdictDto>,
) -> ReadinessStatusDto {
    let sidecar_status = if sidecar.retrieval_mode == "full" {
        ReadinessStatusDto::Ready
    } else {
        ReadinessStatusDto::RepairRetrieval
    };
    match verdict.map(|verdict| verdict.status) {
        Some(status @ (ReadinessStatusDto::RepairSetup | ReadinessStatusDto::RepairIndex)) => {
            status
        }
        Some(ReadinessStatusDto::CheckIndex) if sidecar_status == ReadinessStatusDto::Ready => {
            ReadinessStatusDto::CheckIndex
        }
        _ => sidecar_status,
    }
}

fn lane_next_command(
    lane: &str,
    sidecar: &DoctorSidecarStatusOutput,
    status: ReadinessStatusDto,
    verdict: Option<&codestory_contracts::api::ReadinessVerdictDto>,
    project_arg: &str,
) -> Option<String> {
    if status == ReadinessStatusDto::Ready {
        return Some(retrieval_status_command(sidecar, project_arg));
    }
    if let Some(command) = verdict.and_then(|verdict| verdict.minimum_next.first()) {
        return Some(command.clone());
    }
    match lane {
        "agent_packet_search" if sidecar.retrieval_mode != "full" => Some(format!(
            "codestory-cli ready --goal agent --repair --project {project_arg} --format json"
        )),
        "local_default" if sidecar.retrieval_mode != "full" => Some(format!(
            "codestory-cli retrieval index --project {project_arg} --profile local --refresh full --format json"
        )),
        _ => Some(retrieval_status_command(sidecar, project_arg)),
    }
}

fn retrieval_status_command(sidecar: &DoctorSidecarStatusOutput, project_arg: &str) -> String {
    let mut command = format!(
        "codestory-cli retrieval status --project {project_arg} --profile {}",
        sidecar.profile.as_deref().unwrap_or("local")
    );
    if let Some(run_id) = sidecar.run_id.as_deref() {
        command.push_str(" --run-id ");
        command.push_str(&display::quote_command_argument_value(run_id));
    }
    command.push_str(" --format json");
    command
}

fn apply_sidecar_profile_handoff(
    mut status: DoctorSidecarStatusOutput,
    handoff_failure: Option<String>,
) -> DoctorSidecarStatusOutput {
    if status.retrieval_mode == "full"
        && let Some(reason) = handoff_failure
    {
        status.retrieval_mode = "unavailable".to_string();
        status.degraded_reason = Some(reason);
    }
    status
}

fn doctor_sidecar_profile_handoff_failure(runtime: &RuntimeContext) -> Option<String> {
    let active = codestory_retrieval::sidecar_runtime_auto(&runtime.project_root);
    if active.profile == codestory_retrieval::SidecarProfile::Local {
        return None;
    }
    let local = codestory_retrieval::strict_sidecar_status_for_profile(
        &runtime.project_root,
        Some(&runtime.storage_path),
        codestory_retrieval::SidecarProfile::Local,
    );
    match local {
        Ok(report) if report.retrieval_mode == "full" => None,
        Ok(report) => Some(format!(
            "profile_handoff_mismatch: active profile={} namespace={} is full but local/default profile is mode={} reason={}",
            active.profile.as_str(),
            active.namespace,
            report.retrieval_mode,
            report.degraded_reason.as_deref().unwrap_or("unknown")
        )),
        Err(error) => Some(format!(
            "profile_handoff_mismatch: active profile={} namespace={} is full but local/default status failed: {error}",
            active.profile.as_str(),
            active.namespace
        )),
    }
}

fn doctor_env_check_message(name: &str, value: &str) -> String {
    let trimmed = value.trim();
    if name.ends_with("_URL") || trimmed.contains("://") {
        return format!(
            "set to `{}`",
            managed_embeddings::redact_url_for_display(trimmed)
        );
    }
    format!("set to `{trimmed}`")
}

fn redact_urls_in_text(text: &str) -> String {
    text.split_whitespace()
        .map(redact_url_token)
        .collect::<Vec<_>>()
        .join(" ")
}

fn redact_url_token(token: &str) -> String {
    let prefix_len = token
        .find("://")
        .and_then(|scheme_end| {
            token[..scheme_end]
                .rfind(|ch: char| !(ch.is_ascii_alphanumeric() || matches!(ch, '+' | '-' | '.')))
                .map(|index| index + 1)
                .or(Some(0))
        })
        .unwrap_or(token.len());
    if prefix_len == token.len() {
        return token.to_string();
    }

    let prefix = &token[..prefix_len];
    let url_and_suffix = &token[prefix_len..];
    let suffix_start = url_and_suffix
        .find([')', ']', '}', ',', ';', '`'])
        .unwrap_or(url_and_suffix.len());
    let (url, suffix) = url_and_suffix.split_at(suffix_start);
    format!(
        "{prefix}{}{suffix}",
        managed_embeddings::redact_url_for_display(url)
    )
}

fn semantic_health_check(
    retrieval: &codestory_contracts::api::RetrievalStateDto,
    stats: &codestory_contracts::api::StorageStatsDto,
) -> DoctorCheckOutput {
    if retrieval.semantic_ready {
        if stats.file_count > 0 && retrieval.semantic_doc_count < stats.file_count {
            return doctor_check(
                "semantic",
                "warn",
                format!(
                    "legacy semantic diagnostic partial: {} semantic docs for {} indexed files. Mandatory retrieval health is reported by sidecar_retrieval; run `codestory-cli retrieval index --refresh full` after repairing sidecars.",
                    retrieval.semantic_doc_count, stats.file_count
                ),
            );
        }

        return doctor_check(
            "semantic",
            "info",
            format!(
                "legacy semantic diagnostic ok: stored hybrid/semantic docs are available (docs={}); mandatory retrieval health is reported by sidecar_retrieval.",
                retrieval.semantic_doc_count
            ),
        );
    }

    let message = retrieval.fallback_message.clone().unwrap_or_else(|| {
        "Legacy semantic diagnostics are not ready; mandatory retrieval health is reported by sidecar_retrieval.".to_string()
    });
    let status =
        if retrieval.fallback_reason == Some(RetrievalFallbackReasonDto::MissingEmbeddingRuntime) {
            "warn"
        } else {
            "info"
        };
    doctor_check(
        "semantic",
        status,
        format!("legacy semantic diagnostic failed: {message}"),
    )
}

fn index_freshness_check(freshness: &IndexFreshnessDto) -> DoctorCheckOutput {
    match freshness.status {
        IndexFreshnessStatusDto::Fresh => doctor_check(
            "index_freshness",
            "ok",
            format!(
                "Indexed file inventory is fresh (checked={} duration_ms={}).",
                freshness.checked_file_count, freshness.duration_ms
            ),
        ),
        IndexFreshnessStatusDto::Stale => doctor_check(
            "index_freshness",
            "warn",
            format!(
                "Indexed file inventory is stale: changed={} new={} removed={} (checked={} duration_ms={}). Run `codestory-cli index --refresh incremental` to update the cache.",
                freshness.changed_file_count,
                freshness.new_file_count,
                freshness.removed_file_count,
                freshness.checked_file_count,
                freshness.duration_ms
            ),
        ),
        IndexFreshnessStatusDto::NotChecked => doctor_check(
            "index_freshness",
            "info",
            format!(
                "Index freshness was not checked: {}.",
                freshness.reason.as_deref().unwrap_or("no reason reported")
            ),
        ),
    }
}

fn semantic_contract_check(
    retrieval: &codestory_contracts::api::RetrievalStateDto,
) -> DoctorCheckOutput {
    let Some(stored) = retrieval.stored_embedding.as_ref() else {
        return doctor_check(
            "semantic_contract",
            "info",
            "Stored semantic doc metadata is unavailable.".to_string(),
        );
    };
    if stored.doc_count == 0 {
        return doctor_check(
            "semantic_contract",
            "info",
            "No stored semantic docs are available to compare with the current embedding config."
                .to_string(),
        );
    }

    let mut gaps = Vec::new();
    if stored.mixed_embedding_profiles {
        gaps.push("stored docs use mixed embedding profiles".to_string());
    }
    if stored.mixed_embedding_models {
        gaps.push("stored docs use mixed cache keys".to_string());
    }
    if stored.mixed_embedding_backends {
        gaps.push("stored docs use mixed embedding backends".to_string());
    }
    if stored.mixed_dimensions {
        gaps.push("stored docs use mixed embedding dimensions".to_string());
    }
    if stored.mixed_doc_versions {
        gaps.push("stored docs use mixed semantic doc versions".to_string());
    }
    if stored.mixed_doc_shapes {
        gaps.push("stored docs use mixed semantic doc shapes".to_string());
    }

    if let Some(current) = retrieval.current_embedding.as_ref() {
        compare_contract_field(
            &mut gaps,
            "embedding profile",
            stored.embedding_profile.as_deref(),
            Some(current.profile.as_str()),
        );
        compare_contract_field(
            &mut gaps,
            "embedding backend",
            stored.embedding_backend.as_deref(),
            Some(current.backend.as_str()),
        );
        compare_contract_field(
            &mut gaps,
            "cache key",
            stored.cache_key.as_deref(),
            Some(current.cache_key.as_str()),
        );
        compare_contract_field(
            &mut gaps,
            "semantic doc shape",
            stored.doc_shape.as_deref(),
            Some(current.doc_shape.as_str()),
        );
        if let (Some(stored_dim), Some(current_dim)) = (stored.dimension, current.dimension)
            && stored_dim != current_dim
        {
            gaps.push(format!(
                "embedding dimension mismatch: stored={stored_dim} current={current_dim}"
            ));
        }
    } else {
        gaps.push("current embedding config could not be resolved".to_string());
    }

    if gaps.is_empty() {
        doctor_check(
            "semantic_contract",
            "ok",
            format!(
                "semantic ok: stored semantic docs match the current embedding contract (docs={}).",
                stored.doc_count
            ),
        )
    } else if !retrieval.semantic_ready
        && retrieval.fallback_reason == Some(RetrievalFallbackReasonDto::MissingEmbeddingRuntime)
    {
        doctor_check(
            "semantic_contract",
            "info",
            format!(
                "semantic stale: {}. Resolve the embedding runtime first with `codestory-cli setup embeddings`; then run `codestory-cli retrieval index --refresh full` before trusting packet/search evidence.",
                gaps.join("; ")
            ),
        )
    } else {
        doctor_check(
            "semantic_contract",
            "warn",
            format!(
                "semantic stale: {}. Run `codestory-cli retrieval index --refresh full` before trusting packet/search evidence.",
                gaps.join("; ")
            ),
        )
    }
}

fn managed_doctor_status(state: &str) -> &'static str {
    match state {
        "managed_onnx_ready" | "external_llama_configured" | "disabled_by_config" => "ok",
        "missing_managed_assets" => "info",
        "external_llama_unreachable" | "managed_onnx_unusable" => "warn",
        _ => "info",
    }
}

fn compare_contract_field(
    gaps: &mut Vec<String>,
    label: &str,
    stored: Option<&str>,
    current: Option<&str>,
) {
    match (stored, current) {
        (Some(stored), Some(current)) if stored != current => {
            gaps.push(format!(
                "{label} mismatch: stored={stored} current={current}"
            ));
        }
        (None, Some(current)) => {
            gaps.push(format!(
                "{label} missing from stored docs; current={current}"
            ));
        }
        _ => {}
    }
}

fn doctor_check(
    name: impl Into<String>,
    status: impl Into<String>,
    message: impl Into<String>,
) -> DoctorCheckOutput {
    DoctorCheckOutput {
        name: name.into(),
        status: status.into(),
        message: message.into(),
    }
}

fn doctor_sidecar_check(sidecar: &DoctorSidecarStatusOutput) -> DoctorCheckOutput {
    if sidecar.retrieval_mode == "full" {
        return doctor_check(
            "sidecar_retrieval",
            "ok",
            "mandatory sidecar retrieval is full; packet/search evidence can use sidecar primary.",
        );
    }

    let reason = sidecar
        .degraded_reason
        .as_deref()
        .unwrap_or("no degraded_reason reported");
    doctor_check(
        "sidecar_retrieval",
        "warn",
        format!(
            "mandatory sidecar retrieval is not full (mode={} reason={reason}); repair sidecars before trusting packet/search evidence.",
            sidecar.retrieval_mode
        ),
    )
}

#[cfg(test)]
fn index_next_commands(
    project: &str,
    retrieval: Option<&codestory_contracts::api::RetrievalStateDto>,
    freshness: Option<&IndexFreshnessDto>,
    sidecar_is_full: bool,
) -> Vec<String> {
    let project = quote_command_path(std::path::Path::new(project));
    let mut commands = Vec::new();
    if let Some(freshness) = freshness {
        match freshness.status {
            IndexFreshnessStatusDto::Stale => {
                commands.push(format!(
                    "codestory-cli index --project {project} --refresh incremental"
                ));
                commands.push(format!(
                    "codestory-cli doctor --project {project} --format markdown"
                ));
                return commands;
            }
            IndexFreshnessStatusDto::NotChecked => {
                commands.push(format!(
                    "codestory-cli index --project {project} --refresh full"
                ));
                commands.push(format!(
                    "codestory-cli doctor --project {project} --format markdown"
                ));
                return commands;
            }
            IndexFreshnessStatusDto::Fresh => {}
        }
    }
    if !sidecar_is_full {
        commands.push(format!(
            "codestory-cli retrieval status --project {project}"
        ));
        commands.push(format!(
            "codestory-cli retrieval index --project {project} --refresh full"
        ));
        commands.push(format!(
            "codestory-cli doctor --project {project} --format markdown"
        ));
        return commands;
    }
    if let Some(retrieval) = retrieval.filter(|state| !state.semantic_ready)
        && retrieval.fallback_reason == Some(RetrievalFallbackReasonDto::MissingEmbeddingRuntime)
    {
        commands.push(format!(
            "codestory-cli setup embeddings --project {project}"
        ));
    }
    commands.push(format!("codestory-cli ground --project {project}"));
    commands.push(format!(
        "codestory-cli search --project {project} --query \"<symbol/file/literal/API path>\" --why"
    ));
    commands.push(format!(
        "codestory-cli context --project {project} --query \"<concrete target>\""
    ));
    commands
}

fn write_context_bundle<T: serde::Serialize>(
    bundle_dir: &std::path::Path,
    output: &T,
    graphs: &[GraphArtifactDto],
    markdown: &str,
) -> Result<()> {
    fs::create_dir_all(bundle_dir).with_context(|| {
        format!(
            "Failed to create bundle directory {}",
            display::clean_path_string(&bundle_dir.to_string_lossy())
        )
    })?;
    remove_stale_mermaid_artifacts(bundle_dir)?;
    let mut notes = Vec::new();
    let mut omitted_mermaid_artifacts = 0usize;
    let full_context_json =
        serde_json::to_string_pretty(output).context("Failed to serialize context JSON")?;
    let mut context_json = if markdown.len().saturating_add(full_context_json.len())
        > CONTEXT_BUNDLE_OUTPUT_BYTE_CAP
    {
        notes.push(
            "context.json was reduced to a valid manifest summary because the full context exceeded the bundle byte cap."
                .to_string(),
        );
        context_bundle_summary_json(output)?
    } else {
        full_context_json
    };
    if context_json.len() > CONTEXT_BUNDLE_OUTPUT_BYTE_CAP {
        notes.push(
            "context.json details were omitted because the summary still exceeded the bundle byte cap."
                .to_string(),
        );
        context_json = serde_json::to_string_pretty(&serde_json::json!({
            "truncated": true,
            "reason": "context bundle output hit its byte cap",
            "action": "Narrow the target or use JSON output without --bundle for the full in-memory response.",
        }))
        .context("Failed to serialize minimal context bundle summary JSON")?;
    }

    let mut markdown = if markdown.len() > CONTEXT_BUNDLE_MARKDOWN_SOFT_CAP {
        notes.push(format!(
            "context.md was truncated to {} bytes before writing.",
            CONTEXT_BUNDLE_MARKDOWN_SOFT_CAP
        ));
        truncate_utf8_with_suffix(
            markdown,
            CONTEXT_BUNDLE_MARKDOWN_SOFT_CAP,
            CONTEXT_BUNDLE_TRUNCATION_SUFFIX,
        )
    } else {
        markdown.to_string()
    };
    let remaining_markdown_bytes =
        CONTEXT_BUNDLE_OUTPUT_BYTE_CAP.saturating_sub(context_json.len());
    if markdown.len() > remaining_markdown_bytes {
        notes.push(format!(
            "context.md was truncated to fit the remaining {} bundle bytes.",
            remaining_markdown_bytes
        ));
        markdown = truncate_utf8_with_suffix(
            &markdown,
            remaining_markdown_bytes,
            CONTEXT_BUNDLE_TRUNCATION_SUFFIX,
        );
    }
    fs::write(bundle_dir.join("context.md"), &markdown).with_context(|| {
        format!(
            "Failed to write {}",
            display::clean_path_string(&bundle_dir.join("context.md").to_string_lossy())
        )
    })?;
    fs::write(bundle_dir.join("context.json"), &context_json).with_context(|| {
        format!(
            "Failed to write {}",
            display::clean_path_string(&bundle_dir.join("context.json").to_string_lossy())
        )
    })?;
    let mut written_bytes = markdown.len().saturating_add(context_json.len());
    for graph in graphs {
        if let GraphArtifactDto::Mermaid {
            id, mermaid_syntax, ..
        } = graph
        {
            let file_name = format!("{}.mmd", sanitize_artifact_name(id));
            let artifact_path = bundle_dir.join(&file_name);
            if written_bytes.saturating_add(mermaid_syntax.len()) > CONTEXT_BUNDLE_OUTPUT_BYTE_CAP {
                omitted_mermaid_artifacts = omitted_mermaid_artifacts.saturating_add(1);
                continue;
            }
            fs::write(&artifact_path, mermaid_syntax)?;
            written_bytes = written_bytes.saturating_add(mermaid_syntax.len());
        }
    }
    if omitted_mermaid_artifacts > 0 {
        notes.push(format!(
            "Omitted {omitted_mermaid_artifacts} Mermaid artifact(s) after reaching the bundle byte cap."
        ));
    }
    let manifest = serde_json::json!({
        "output_byte_cap": CONTEXT_BUNDLE_OUTPUT_BYTE_CAP,
        "written_bytes_excluding_manifest": written_bytes,
        "truncated": !notes.is_empty(),
        "omitted_mermaid_artifacts": omitted_mermaid_artifacts,
        "notes": notes,
    });
    fs::write(
        bundle_dir.join("bundle_manifest.json"),
        serde_json::to_string_pretty(&manifest).context("Failed to serialize bundle manifest")?,
    )
    .with_context(|| {
        format!(
            "Failed to write {}",
            display::clean_path_string(&bundle_dir.join("bundle_manifest.json").to_string_lossy())
        )
    })?;
    Ok(())
}

fn remove_stale_mermaid_artifacts(bundle_dir: &std::path::Path) -> Result<()> {
    for entry in fs::read_dir(bundle_dir).with_context(|| {
        format!(
            "Failed to inspect bundle directory {}",
            display::clean_path_string(&bundle_dir.to_string_lossy())
        )
    })? {
        let entry = entry.context("Failed to inspect bundle entry")?;
        let path = entry.path();
        if path.extension().is_some_and(|extension| extension == "mmd") {
            fs::remove_file(&path).with_context(|| {
                format!(
                    "Failed to remove stale {}",
                    display::clean_path_string(&path.to_string_lossy())
                )
            })?;
        }
    }
    Ok(())
}

fn context_bundle_summary_json<T: serde::Serialize>(output: &T) -> Result<String> {
    let value = serde_json::to_value(output).context("Failed to serialize context summary JSON")?;
    serde_json::to_string_pretty(&serde_json::json!({
        "truncated": true,
        "reason": "context bundle output hit its byte cap",
        "action": "Narrow the target or use JSON output without --bundle for the full in-memory response.",
        "target": value.get("target"),
        "resolution": value.get("resolution"),
        "context_summary": value.pointer("/context/summary"),
        "citation_count": value
            .pointer("/context/citations")
            .and_then(|citations| citations.as_array())
            .map(|citations| citations.len())
            .unwrap_or(0),
        "graph_count": value
            .pointer("/context/graphs")
            .and_then(|graphs| graphs.as_array())
            .map(|graphs| graphs.len())
            .unwrap_or(0),
    }))
    .context("Failed to serialize context bundle summary JSON")
}

fn truncate_utf8_with_suffix(value: &str, max_bytes: usize, suffix: &str) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let mut keep = max_bytes.saturating_sub(suffix.len());
    while keep > 0 && !value.is_char_boundary(keep) {
        keep -= 1;
    }
    let mut truncated = value[..keep].to_string();
    truncated.push_str(suffix);
    if truncated.len() > max_bytes {
        let mut hard_keep = max_bytes;
        while hard_keep > 0 && !truncated.is_char_boundary(hard_keep) {
            hard_keep -= 1;
        }
        truncated.truncate(hard_keep);
    }
    truncated
}

fn sanitize_artifact_name(value: &str) -> String {
    let mut out = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>();
    if out.is_empty() {
        out.push_str("artifact");
    }
    out
}

fn ensure_dot_only_for_trail(format: args::OutputFormat, command: &str) -> Result<()> {
    if format == args::OutputFormat::Dot {
        bail!("--format dot is only supported by `trail`; `{command}` supports markdown and json");
    }
    Ok(())
}

struct SearchOutputParts<'a> {
    project_root: &'a std::path::Path,
    query: &'a str,
    retrieval: &'a codestory_contracts::api::RetrievalStateDto,
    retrieval_shadow: Option<&'a RetrievalShadowDto>,
    freshness: Option<&'a IndexFreshnessDto>,
    symbol_hits: &'a [SearchHit],
    repo_text_hits: &'a [SearchHit],
    repo_text_stats: Option<&'a RepoTextScanStatsDto>,
    query_assessment: Option<&'a SearchQueryAssessmentDto>,
    search_plan: Option<&'a codestory_contracts::api::SearchPlanDto>,
    suggestions: &'a [SearchHit],
    occurrences_by_node: &'a HashMap<NodeId, Vec<SourceOccurrenceDto>>,
    limit_per_source: u32,
    repo_text: RepoTextOutputConfig,
    explain: bool,
}

fn build_search_output(parts: SearchOutputParts<'_>) -> SearchOutput {
    let indexed_symbol_hits = parts
        .symbol_hits
        .iter()
        .map(|hit| {
            build_search_hit_output(
                parts.project_root,
                hit,
                Some(parts.query),
                parts.explain,
                occurrences_for_hit(parts.occurrences_by_node, hit),
            )
        })
        .collect::<Vec<_>>();
    let mut duplicate_index = HashMap::new();
    for hit in &indexed_symbol_hits {
        if let Some(key) = search_hit_location_key(hit) {
            duplicate_index
                .entry(key)
                .or_insert_with(|| hit.node_id.clone());
        }
    }
    let repo_text_hits = parts
        .repo_text_hits
        .iter()
        .map(|hit| {
            let mut output = build_search_hit_output(
                parts.project_root,
                hit,
                Some(parts.query),
                parts.explain,
                &[],
            );
            if let Some(key) = search_hit_location_key(&output) {
                output.duplicate_of = duplicate_index.get(&key).cloned();
            }
            output
        })
        .collect::<Vec<_>>();
    let query_hints = search_query_hints(parts.query, &indexed_symbol_hits, &repo_text_hits);

    SearchOutput {
        query: parts.query.to_string(),
        retrieval: parts.retrieval.clone(),
        retrieval_shadow: parts.retrieval_shadow.cloned(),
        freshness: parts.freshness.cloned(),
        limit_per_source: parts.limit_per_source,
        repo_text_mode: parts.repo_text.mode,
        repo_text_enabled: parts.repo_text.enabled,
        query_assessment: parts.query_assessment.cloned(),
        search_plan: parts.search_plan.cloned(),
        explain: parts.explain,
        query_hints,
        suggestions: parts
            .suggestions
            .iter()
            .map(|hit| {
                build_search_hit_output(
                    parts.project_root,
                    hit,
                    Some(parts.query),
                    parts.explain,
                    occurrences_for_hit(parts.occurrences_by_node, hit),
                )
            })
            .collect(),
        indexed_symbol_hits,
        repo_text_hits,
        repo_text_stats: parts.repo_text_stats.cloned(),
    }
}

fn build_query_resolution_output(
    project_root: &std::path::Path,
    target: &runtime::ResolvedTarget,
) -> QueryResolutionOutput {
    QueryResolutionOutput {
        selector: target.selector,
        requested: target.requested.clone(),
        file_filter: target
            .file_filter
            .as_deref()
            .map(crate::display::clean_path_string),
        resolved: build_search_hit_output(
            project_root,
            &target.selected,
            Some(&target.requested),
            false,
            &[],
        ),
        alternatives: target
            .alternatives
            .iter()
            .skip(1)
            .map(|hit| {
                build_search_hit_output(project_root, hit, Some(&target.requested), false, &[])
            })
            .collect(),
    }
}

fn build_query_resolution_output_with_runtime(
    runtime: &RuntimeContext,
    target: &runtime::ResolvedTarget,
) -> QueryResolutionOutput {
    let occurrences = collect_search_hit_occurrences(
        runtime,
        std::iter::once(&target.selected).chain(target.alternatives.iter()),
    );
    QueryResolutionOutput {
        selector: target.selector,
        requested: target.requested.clone(),
        file_filter: target
            .file_filter
            .as_deref()
            .map(crate::display::clean_path_string),
        resolved: build_search_hit_output(
            &runtime.project_root,
            &target.selected,
            Some(&target.requested),
            false,
            occurrences_for_hit(&occurrences, &target.selected),
        ),
        alternatives: target
            .alternatives
            .iter()
            .skip(1)
            .map(|hit| {
                build_search_hit_output(
                    &runtime.project_root,
                    hit,
                    Some(&target.requested),
                    false,
                    occurrences_for_hit(&occurrences, hit),
                )
            })
            .collect(),
    }
}

fn build_search_hit_output(
    project_root: &std::path::Path,
    hit: &SearchHit,
    query: Option<&str>,
    explain: bool,
    occurrences: &[SourceOccurrenceDto],
) -> SearchHitOutput {
    let file_path = hit
        .file_path
        .as_deref()
        .map(|value| crate::display::relative_path(project_root, value));
    let score_breakdown = hit.score_breakdown.clone();
    let why = if explain {
        explain_search_hit(hit, score_breakdown.as_ref())
    } else {
        Vec::new()
    };
    let mut verification_targets =
        verification_targets_for_hit(project_root, &hit.display_name, occurrences);
    verification_targets.extend(implementation_counterpart_targets_for_hit(
        project_root,
        &hit.display_name,
        hit.file_path.as_deref(),
    ));
    verification_targets.extend(interface_implementation_targets_for_hit(
        project_root,
        &hit.display_name,
        hit.file_path.as_deref(),
    ));
    dedupe_verification_targets(&mut verification_targets);
    let primary_occurrence_kind =
        primary_occurrence(occurrences).map(|occurrence| occurrence.kind.clone());
    let symbol_role = primary_occurrence_kind
        .as_deref()
        .map(symbol_role_for_occurrence_kind)
        .map(str::to_string);
    let paired_refs = paired_occurrence_targets(
        project_root,
        &hit.display_name,
        primary_occurrence_kind.as_deref(),
        occurrences,
    );
    let resolution_hints = resolution_hints_for_hit(hit, &verification_targets, &paired_refs);
    SearchHitOutput {
        number: None,
        node_id: hit.node_id.0.clone(),
        node_ref: crate::output::node_ref(
            project_root,
            hit.file_path.as_deref(),
            hit.line,
            &hit.display_name,
        ),
        display_name: hit.display_name.clone(),
        kind: hit.kind,
        file_path,
        line: hit.line,
        score: hit.score,
        origin: hit.origin,
        match_quality: hit
            .match_quality
            .unwrap_or_else(|| search_match_quality(query, hit)),
        resolvable: hit.resolvable,
        score_breakdown,
        duplicate_of: None,
        excerpt: repo_text_excerpt(project_root, hit),
        primary_occurrence_kind,
        symbol_role,
        paired_refs,
        verification_targets,
        resolution_hints,
        why,
    }
}

fn build_numbered_search_hit_output(
    project_root: &std::path::Path,
    hit: &SearchHit,
    query: Option<&str>,
    number: usize,
) -> SearchHitOutput {
    let mut output = build_search_hit_output(project_root, hit, query, false, &[]);
    output.number = Some(number);
    output
}

fn search_match_quality(query: Option<&str>, hit: &SearchHit) -> SearchMatchQualityDto {
    if hit.is_text_match() {
        return SearchMatchQualityDto::RepoText;
    }
    let Some(query) = query.map(str::trim).filter(|query| !query.is_empty()) else {
        return SearchMatchQualityDto::SemanticSuggestion;
    };
    let query_normalized = codestory_runtime::normalize_symbol_query(query);
    let display_normalized = codestory_runtime::normalize_symbol_query(&hit.display_name);
    let terminal = codestory_runtime::terminal_symbol_segment(&hit.display_name);
    let leading = codestory_runtime::leading_symbol_segment(&hit.display_name);
    if hit.display_name == query {
        return SearchMatchQualityDto::Exact;
    }
    if display_normalized == query_normalized
        || terminal == query_normalized
        || leading == query_normalized
    {
        return SearchMatchQualityDto::NormalizedExact;
    }
    if display_normalized.starts_with(&query_normalized)
        || terminal.starts_with(&query_normalized)
        || leading.starts_with(&query_normalized)
    {
        return SearchMatchQualityDto::Prefix;
    }
    if hit
        .score_breakdown
        .as_ref()
        .is_some_and(|breakdown| breakdown.semantic > 0.0 && breakdown.lexical <= f32::EPSILON)
    {
        return SearchMatchQualityDto::SemanticSuggestion;
    }
    SearchMatchQualityDto::Fuzzy
}

fn collect_search_hit_occurrences<'a>(
    runtime: &RuntimeContext,
    hits: impl Iterator<Item = &'a SearchHit>,
) -> HashMap<NodeId, Vec<SourceOccurrenceDto>> {
    let mut seen = HashSet::new();
    let mut occurrences_by_node = HashMap::new();
    for hit in hits {
        if hit.is_text_match() || !hit.resolvable || !seen.insert(hit.node_id.clone()) {
            continue;
        }
        if let Ok(occurrences) = runtime.browser.node_occurrences(NodeOccurrencesRequest {
            id: hit.node_id.clone(),
        }) {
            occurrences_by_node.insert(hit.node_id.clone(), occurrences);
        }
    }
    occurrences_by_node
}

fn occurrences_for_hit<'a>(
    occurrences_by_node: &'a HashMap<NodeId, Vec<SourceOccurrenceDto>>,
    hit: &SearchHit,
) -> &'a [SourceOccurrenceDto] {
    occurrences_by_node
        .get(&hit.node_id)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

fn primary_occurrence(occurrences: &[SourceOccurrenceDto]) -> Option<&SourceOccurrenceDto> {
    occurrences.iter().max_by(|left, right| {
        occurrence_kind_rank(&left.kind)
            .cmp(&occurrence_kind_rank(&right.kind))
            .then_with(|| right.start_line.cmp(&left.start_line))
            .then_with(|| right.start_col.cmp(&left.start_col))
    })
}

fn occurrence_kind_rank(kind: &str) -> u8 {
    match kind {
        "definition" | "macro_definition" => 5,
        "declaration" => 4,
        "reference" | "macro_reference" => 2,
        _ => 1,
    }
}

fn symbol_role_for_occurrence_kind(kind: &str) -> &'static str {
    match kind {
        "definition" | "macro_definition" => "definition",
        "declaration" => "declaration",
        "reference" | "macro_reference" => "reference",
        _ => "unknown",
    }
}

fn verification_targets_for_hit(
    project_root: &std::path::Path,
    display_name: &str,
    occurrences: &[SourceOccurrenceDto],
) -> Vec<VerificationTargetOutput> {
    let Some(primary) = primary_occurrence(occurrences) else {
        return Vec::new();
    };
    let mut ordered = occurrences.iter().collect::<Vec<_>>();
    ordered.sort_by(|left, right| {
        occurrence_kind_rank(&right.kind)
            .cmp(&occurrence_kind_rank(&left.kind))
            .then_with(|| left.file_path.cmp(&right.file_path))
            .then_with(|| left.start_line.cmp(&right.start_line))
            .then_with(|| left.start_col.cmp(&right.start_col))
    });

    let mut targets = Vec::new();
    let mut seen = HashSet::new();
    for occurrence in ordered {
        let role = symbol_role_for_occurrence_kind(&occurrence.kind);
        let is_primary = same_source_occurrence(primary, occurrence);
        if !is_primary && !matches!(role, "definition" | "declaration") {
            continue;
        }
        let key = (
            role.to_string(),
            occurrence.file_path.clone(),
            occurrence.start_line,
        );
        if !seen.insert(key) {
            continue;
        }
        let reason = if is_primary {
            "primary source occurrence selected for this symbol"
        } else if role == "definition" {
            "paired definition/body location for a declaration-style hit"
        } else {
            "paired declaration location for a definition-style hit"
        };
        targets.push(verification_target_from_occurrence(
            project_root,
            display_name,
            occurrence,
            role,
            reason,
        ));
        if targets.len() >= 4 {
            break;
        }
    }
    targets
}

fn implementation_counterpart_targets_for_hit(
    project_root: &std::path::Path,
    display_name: &str,
    file_path: Option<&str>,
) -> Vec<VerificationTargetOutput> {
    let Some(file_path) = file_path else {
        return Vec::new();
    };
    if !display_name.contains("::") || !is_cxx_header_path(file_path) {
        return Vec::new();
    }
    let hit_path = std::path::Path::new(file_path);
    let absolute_header = if hit_path.is_absolute() {
        hit_path.to_path_buf()
    } else {
        project_root.join(hit_path)
    };
    let Some(stem) = absolute_header.file_stem().and_then(|stem| stem.to_str()) else {
        return Vec::new();
    };
    let Some(parent) = absolute_header.parent() else {
        return Vec::new();
    };
    [".cpp", ".cc", ".cxx", ".c"]
        .into_iter()
        .filter_map(|extension| {
            let candidate = parent.join(format!("{stem}{extension}"));
            let content = fs::read_to_string(&candidate).ok()?;
            let line_index = content
                .lines()
                .position(|line| line.contains(display_name))?;
            let path =
                crate::display::relative_path(project_root, candidate.to_string_lossy().as_ref());
            let line = (line_index + 1) as u32;
            Some(VerificationTargetOutput {
                role: "definition".to_string(),
                path: path.clone(),
                line,
                node_ref: Some(format!("{path}:{line}:{display_name}")),
                reason: "sibling implementation location for a C/C++ header hit".to_string(),
            })
        })
        .collect()
}

fn interface_implementation_targets_for_hit(
    project_root: &std::path::Path,
    display_name: &str,
    file_path: Option<&str>,
) -> Vec<VerificationTargetOutput> {
    let Some(file_path) = file_path else {
        return Vec::new();
    };
    if !is_cxx_header_path(file_path) {
        return Vec::new();
    }
    let Some((interface_name, member_name)) = split_qualified_member(display_name) else {
        return Vec::new();
    };
    let hit_path = std::path::Path::new(file_path);
    let absolute_header = if hit_path.is_absolute() {
        hit_path.to_path_buf()
    } else {
        project_root.join(hit_path)
    };
    let Ok(interface_content) = fs::read_to_string(&absolute_header) else {
        return Vec::new();
    };
    if !abstract_header_declares_member(&interface_content, interface_name, member_name) {
        return Vec::new();
    }
    let Some(parent) = absolute_header.parent() else {
        return Vec::new();
    };
    let Ok(entries) = fs::read_dir(parent) else {
        return Vec::new();
    };
    let mut headers = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path != &absolute_header && is_cxx_header_path(&path.to_string_lossy()))
        .collect::<Vec<_>>();
    headers.sort();

    let mut targets = Vec::new();
    for header in headers {
        let Some(class_name) = header.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        let Ok(header_content) = fs::read_to_string(&header) else {
            continue;
        };
        if !header_declares_public_base(&header_content, class_name, interface_name) {
            continue;
        }
        let declaration_line =
            line_containing(&header_content, &format!("class {class_name}")).unwrap_or(1);
        let header_path = crate::display::relative_path(project_root, &header.to_string_lossy());
        targets.push(VerificationTargetOutput {
            role: "declaration".to_string(),
            path: header_path.clone(),
            line: declaration_line,
            node_ref: Some(format!("{header_path}:{declaration_line}:{class_name}")),
            reason: "C/C++ implementation class declaration for an abstract interface hit"
                .to_string(),
        });

        for extension in [".cpp", ".cc", ".cxx", ".c"] {
            let implementation = parent.join(format!("{class_name}{extension}"));
            let Ok(implementation_content) = fs::read_to_string(&implementation) else {
                continue;
            };
            let definition_pattern = format!("{class_name}::{member_name}");
            let Some(definition_line) =
                line_containing(&implementation_content, &definition_pattern)
            else {
                continue;
            };
            let path =
                crate::display::relative_path(project_root, &implementation.to_string_lossy());
            targets.push(VerificationTargetOutput {
                role: "definition".to_string(),
                path: path.clone(),
                line: definition_line,
                node_ref: Some(format!("{path}:{definition_line}:{definition_pattern}")),
                reason: "C/C++ implementation method for an abstract interface hit".to_string(),
            });
            break;
        }
        if targets.len() >= 4 {
            break;
        }
    }
    targets
}

fn split_qualified_member(display_name: &str) -> Option<(&str, &str)> {
    let (owner, member) = display_name.rsplit_once("::")?;
    let owner = owner.rsplit("::").next()?.trim();
    let member = member
        .split_once('(')
        .map(|(prefix, _)| prefix)
        .unwrap_or(member)
        .trim();
    (!owner.is_empty() && !member.is_empty()).then_some((owner, member))
}

fn abstract_header_declares_member(content: &str, interface_name: &str, member_name: &str) -> bool {
    content.contains(&format!("class {interface_name}"))
        && content.contains(member_name)
        && content.contains("= 0")
}

fn header_declares_public_base(content: &str, class_name: &str, base_name: &str) -> bool {
    content.contains(&format!("class {class_name}"))
        && content.contains(&format!("public {base_name}"))
}

fn line_containing(content: &str, pattern: &str) -> Option<u32> {
    content
        .lines()
        .position(|line| line.contains(pattern))
        .map(|index| (index + 1) as u32)
}

fn is_cxx_header_path(path: &str) -> bool {
    let path = path.to_ascii_lowercase();
    path.ends_with(".h")
        || path.ends_with(".hpp")
        || path.ends_with(".hh")
        || path.ends_with(".hxx")
}

fn paired_occurrence_targets(
    project_root: &std::path::Path,
    display_name: &str,
    primary_kind: Option<&str>,
    occurrences: &[SourceOccurrenceDto],
) -> Vec<VerificationTargetOutput> {
    let primary_role = primary_kind.map(symbol_role_for_occurrence_kind);
    let wanted_role = match primary_role {
        Some("declaration") => Some("definition"),
        Some("definition") => Some("declaration"),
        _ => None,
    };
    let Some(wanted_role) = wanted_role else {
        return Vec::new();
    };

    occurrences
        .iter()
        .filter(|occurrence| symbol_role_for_occurrence_kind(&occurrence.kind) == wanted_role)
        .take(3)
        .map(|occurrence| {
            let reason = if wanted_role == "definition" {
                "paired definition/body location"
            } else {
                "paired declaration location"
            };
            verification_target_from_occurrence(
                project_root,
                display_name,
                occurrence,
                wanted_role,
                reason,
            )
        })
        .collect()
}

fn verification_target_from_occurrence(
    project_root: &std::path::Path,
    display_name: &str,
    occurrence: &SourceOccurrenceDto,
    role: &str,
    reason: &str,
) -> VerificationTargetOutput {
    let path = crate::display::relative_path(project_root, &occurrence.file_path);
    VerificationTargetOutput {
        role: role.to_string(),
        path: path.clone(),
        line: occurrence.start_line,
        node_ref: Some(format!("{path}:{}:{display_name}", occurrence.start_line)),
        reason: reason.to_string(),
    }
}

fn same_source_occurrence(left: &SourceOccurrenceDto, right: &SourceOccurrenceDto) -> bool {
    left.kind == right.kind
        && left.file_path == right.file_path
        && left.start_line == right.start_line
        && left.start_col == right.start_col
        && left.end_line == right.end_line
        && left.end_col == right.end_col
}

fn resolution_hints_for_hit(
    hit: &SearchHit,
    verification_targets: &[VerificationTargetOutput],
    paired_refs: &[VerificationTargetOutput],
) -> Vec<String> {
    let mut hints = Vec::new();
    if hit.kind == NodeKind::UNKNOWN {
        hints.push(
            "node kind is unknown; prefer a typed alternative for symbol/trail/snippet follow-up"
                .to_string(),
        );
    }
    if hit.is_text_match() {
        hints.push(
            "repo-text hit is a file/line hint only; choose an indexed symbol before graph browsing"
                .to_string(),
        );
        if hit
            .file_path
            .as_deref()
            .is_some_and(|path| path.ends_with(".svelte"))
        {
            hints.push(
                "Svelte files are currently surfaced through repo-text hints; typed graph edges may be unavailable for this file"
                    .to_string(),
            );
        }
    }
    if hit.resolvable && verification_targets.is_empty() {
        hints.push(
            "no source occurrence metadata was available for verification targeting".to_string(),
        );
    }
    if !paired_refs.is_empty() {
        hints.push("declaration/definition pair detected; open both files before trusting architecture claims".to_string());
    }
    hints
}

fn explain_search_hit(
    hit: &SearchHit,
    breakdown: Option<&RetrievalScoreBreakdownDto>,
) -> Vec<String> {
    let mut why = Vec::new();
    match breakdown {
        Some(breakdown) => why.push(format!(
            "ranked by hybrid score lexical={:.3} semantic={:.3} graph={:.3} total={:.3}",
            breakdown.lexical, breakdown.semantic, breakdown.graph, breakdown.total
        )),
        None if hit.is_text_match() => why.push(
            "repo-text diagnostic match; use the file/line hint for navigation, then resolve a typed symbol before using graph evidence"
                .to_string(),
        ),
        None => why.push(format!(
            "ranked by symbolic score {:.3} with origin {}",
            hit.score,
            hit.origin.as_str()
        )),
    }
    if hit.resolvable {
        why.push("can be passed to symbol, trail, snippet, or explore as a focus id".to_string());
    }
    why
}

fn search_query_hints(
    query: &str,
    indexed_hits: &[SearchHitOutput],
    repo_text_hits: &[SearchHitOutput],
) -> Vec<String> {
    if !indexed_hits.is_empty() {
        return Vec::new();
    }
    let mut hints = Vec::new();
    if repo_text_hits.is_empty() {
        hints.push(
            "No indexed symbol or repo-text hits; try a shorter symbol name, module path, or run index --refresh full."
                .to_string(),
        );
    } else {
        hints.push(
            "Only repo-text hits matched; try a concrete identifier from an excerpt to resolve a symbol."
                .to_string(),
        );
    }
    let terms = query
        .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))
        .filter(|term| term.len() >= 3)
        .take(4)
        .collect::<Vec<_>>();
    if !terms.is_empty() {
        hints.push(format!("Possible query terms: {}", terms.join(", ")));
    }
    hints
}

fn search_hit_location_key(hit: &SearchHitOutput) -> Option<(String, u32)> {
    Some((hit.file_path.clone()?, hit.line?))
}

#[cfg(test)]
fn hide_speculative_trail_edges(mut context: TrailContextDto) -> TrailContextDto {
    let original_edge_count = context.trail.edges.len();
    let retained_edges = context
        .trail
        .edges
        .into_iter()
        .filter(|edge| !is_speculative_trail_edge(edge))
        .collect::<Vec<_>>();

    let mut adjacency = HashMap::new();
    for edge in &retained_edges {
        adjacency
            .entry(edge.source.clone())
            .or_insert_with(Vec::new)
            .push(edge.target.clone());
        adjacency
            .entry(edge.target.clone())
            .or_insert_with(Vec::new)
            .push(edge.source.clone());
    }

    let mut reachable = HashSet::new();
    let mut queue = std::collections::VecDeque::new();
    reachable.insert(context.trail.center_id.clone());
    queue.push_back(context.trail.center_id.clone());
    while let Some(node_id) = queue.pop_front() {
        if let Some(next_nodes) = adjacency.get(&node_id) {
            for next in next_nodes {
                if reachable.insert(next.clone()) {
                    queue.push_back(next.clone());
                }
            }
        }
    }

    context
        .trail
        .nodes
        .retain(|node| reachable.contains(&node.id));
    context.trail.edges = retained_edges
        .into_iter()
        .filter(|edge| reachable.contains(&edge.source) && reachable.contains(&edge.target))
        .collect();
    let omitted_edges = original_edge_count.saturating_sub(context.trail.edges.len()) as u32;
    context.trail.omitted_edge_count = context
        .trail
        .omitted_edge_count
        .saturating_add(omitted_edges);

    if let Some(layout) = context.trail.canonical_layout.as_mut() {
        layout.nodes.retain(|node| reachable.contains(&node.id));
        layout.edges.retain(|edge| {
            !is_speculative_certainty_label(edge.certainty.as_deref())
                && reachable.contains(&edge.source)
                && reachable.contains(&edge.target)
        });
    }

    context
}

#[cfg(test)]
fn is_speculative_trail_edge(edge: &codestory_contracts::api::GraphEdgeDto) -> bool {
    if is_speculative_certainty_label(edge.certainty.as_deref()) {
        return true;
    }
    is_runtime_bridge_edge(edge.kind)
        && (is_probable_certainty_label(edge.certainty.as_deref())
            || edge.confidence.is_some_and(|confidence| {
                confidence < codestory_contracts::graph::ResolutionCertainty::CERTAIN_MIN
            }))
}

#[cfg(test)]
fn is_speculative_certainty_label(certainty: Option<&str>) -> bool {
    matches!(
        certainty.map(|value| value.to_ascii_lowercase()).as_deref(),
        Some("uncertain" | "speculative")
    )
}

#[cfg(test)]
fn is_probable_certainty_label(certainty: Option<&str>) -> bool {
    certainty
        .map(|value| value.eq_ignore_ascii_case("probable"))
        .unwrap_or(false)
}

#[cfg(test)]
fn is_runtime_bridge_edge(kind: codestory_contracts::api::EdgeKind) -> bool {
    matches!(
        kind,
        codestory_contracts::api::EdgeKind::CALL | codestory_contracts::api::EdgeKind::MACRO_USAGE
    )
}

fn repo_text_excerpt(project_root: &std::path::Path, hit: &SearchHit) -> Option<String> {
    if !hit.is_text_match() {
        return None;
    }
    let path = std::path::Path::new(hit.file_path.as_deref()?);
    let line = hit.line?;
    let resolved_path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        project_root.join(path)
    };
    let contents = fs::read_to_string(resolved_path).ok()?;
    let source_line = contents
        .lines()
        .nth(line.saturating_sub(1) as usize)?
        .trim();
    Some(compact_excerpt(source_line, 140))
}

fn compact_excerpt(line: &str, max_len: usize) -> String {
    let collapsed = line.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.len() <= max_len {
        return collapsed;
    }
    let clipped = collapsed
        .char_indices()
        .take_while(|(idx, _)| *idx < max_len.saturating_sub(1))
        .map(|(_, ch)| ch)
        .collect::<String>();
    format!("{clipped}…")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::RefreshMode;
    use crate::display::{clean_path_string, relative_path};
    use crate::query_resolution::compare_resolution_hits;
    use crate::runtime::{cache_root_for_project, fnv1a_hex, resolve_refresh_request};
    use codestory_contracts::api::{
        AgentAnswerDto, AgentCitationDto, AgentRetrievalPolicyModeDto, AgentRetrievalPresetDto,
        AgentRetrievalTraceDto, EdgeId, EdgeKind, GraphEdgeDto, GraphNodeDto, GraphResponse,
        IndexMode, IndexedFileDto, IndexedFileIncompleteReasonCountDto, IndexedFileRoleDto,
        IndexedFilesDto, IndexedFilesSummaryDto, IndexingPhaseTimings, NodeDetailsDto, NodeId,
        PacketBudgetDto, PacketBudgetLimitsDto, PacketBudgetUsageDto, PacketClaimDto,
        PacketPlanDto, PacketPlanQueryDto, PacketRetrievalTraceSummaryDto, PacketSufficiencyDto,
        ProjectSummary, RetrievalModeDto, RetrievalStateDto, SearchHit, SearchHitOrigin,
        SemanticModeDto, StorageStatsDto, TrailContextDto,
    };
    use std::fs;
    use std::path::{Path, PathBuf};
    use tempfile::tempdir;

    struct EnvVarSnapshot<'a> {
        values: Vec<(&'a str, Option<std::ffi::OsString>)>,
    }

    impl<'a> EnvVarSnapshot<'a> {
        fn clear(names: &'a [&'a str]) -> Self {
            let values = names
                .iter()
                .map(|name| (*name, std::env::var_os(name)))
                .collect();
            for name in names {
                unsafe {
                    std::env::remove_var(name);
                }
            }
            Self { values }
        }
    }

    impl Drop for EnvVarSnapshot<'_> {
        fn drop(&mut self) {
            for (name, value) in &self.values {
                unsafe {
                    match value {
                        Some(value) => std::env::set_var(name, value),
                        None => std::env::remove_var(name),
                    }
                }
            }
        }
    }

    #[test]
    fn classify_local_refresh_failure_state_detects_lock_contention() {
        let locked = anyhow::anyhow!("cache_busy: database is locked");
        assert_eq!(
            classify_local_refresh_failure_state(&locked),
            readiness::LocalRefreshState::SkippedLocked
        );

        let failed = anyhow::anyhow!("index refresh failed");
        assert_eq!(
            classify_local_refresh_failure_state(&failed),
            readiness::LocalRefreshState::Failed
        );
    }

    fn test_search_hit_defaults() -> SearchHit {
        SearchHit {
            node_id: NodeId(String::new()),
            display_name: String::new(),
            kind: NodeKind::UNKNOWN,
            file_path: None,
            line: None,
            score: 0.0,
            origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
            match_quality: None,
            resolvable: true,
            evidence_tier: None,
            evidence_producer: None,
            resolution_status: None,
            loss_reason: None,
            coverage_role: None,
            eligible_for_sufficiency: None,
            score_breakdown: None,
        }
    }

    #[test]
    fn ready_repair_embed_liveness_blocks_before_semantic_smoke() {
        let infrastructure = codestory_retrieval::InfrastructureHealth {
            zoekt_reachable: true,
            qdrant_reachable: true,
            embed_reachable: false,
            zoekt_detail: "http 200".into(),
            qdrant_detail: "http 200".into(),
            embed_detail:
                "llama.cpp embeddings unavailable: http://127.0.0.1:55280/v1/embeddings: Connection Failed"
                    .into(),
        };

        let error = ensure_ready_repair_embed_liveness(&infrastructure)
            .expect_err("unreachable embedding endpoint must stop ready repair");
        let message = format!("{error:#}");

        assert!(message.contains("embedding sidecar liveness failed"));
        assert!(message.contains("before mandatory Qdrant semantic smoke"));
        assert!(message.contains("http://127.0.0.1:55280/v1/embeddings"));
    }

    #[test]
    fn sidecar_profile_handoff_downgrades_full_readiness() {
        let status = DoctorSidecarStatusOutput {
            profile: Some("agent".to_string()),
            run_id: Some("run".to_string()),
            retrieval_mode: "full".to_string(),
            degraded_reason: None,
            manifest_generation: Some("generation".to_string()),
            manifest_input_hash: Some("hash".to_string()),
            precise_semantic_import_status: None,
            precise_semantic_import_reason: None,
            precise_semantic_import_revision: None,
            precise_semantic_import_producer: None,
        };

        let downgraded = apply_sidecar_profile_handoff(
            status,
            Some(
                "profile_handoff_mismatch: active profile=agent namespace=run is full but local/default profile is mode=unavailable reason=zoekt_stub"
                    .to_string(),
            ),
        );

        assert_eq!(downgraded.retrieval_mode, "unavailable");
        assert_eq!(
            downgraded.degraded_reason.as_deref(),
            Some(
                "profile_handoff_mismatch: active profile=agent namespace=run is full but local/default profile is mode=unavailable reason=zoekt_stub"
            )
        );
    }

    #[test]
    fn agent_readiness_runtime_does_not_collapse_to_local_without_agent_run() {
        let _env_lock = crate::config::config_env_test_lock();
        let _env_snapshot = EnvVarSnapshot::clear(&[
            "CODESTORY_RETRIEVAL_PROFILE",
            "CODESTORY_SIDECAR_PROFILE",
            "CODESTORY_AGENT_RUN_ID",
            "CODESTORY_SIDECAR_RUN_ID",
            "CODESTORY_AGENT",
            "CODESTORY_AGENT_RUN",
            "CI",
            "GITHUB_ACTIONS",
        ]);
        let temp = tempdir().expect("temp dir");
        let project = temp.path().join("repo");
        fs::create_dir_all(&project).expect("create project");

        let runtime = agent_readiness_sidecar_runtime(&project);

        assert_eq!(runtime.profile, codestory_retrieval::SidecarProfile::Agent);
        assert_eq!(runtime.run_id.as_deref(), Some(AGENT_RUN_MISSING_ID));
    }

    #[test]
    fn readiness_lane_keeps_agent_full_separate_from_local_handoff_mismatch() {
        let sidecar = DoctorSidecarStatusOutput {
            profile: Some("agent".to_string()),
            run_id: Some("run".to_string()),
            retrieval_mode: "full".to_string(),
            degraded_reason: None,
            manifest_generation: Some("generation".to_string()),
            manifest_input_hash: Some("hash".to_string()),
            precise_semantic_import_status: None,
            precise_semantic_import_reason: None,
            precise_semantic_import_revision: None,
            precise_semantic_import_producer: None,
        };
        let aggregate_verdict = codestory_contracts::api::ReadinessVerdictDto {
            goal: ReadinessGoalDto::AgentPacketSearch,
            status: ReadinessStatusDto::RepairRetrieval,
            summary: "profile_handoff_mismatch: local/default is unavailable".to_string(),
            minimum_next: vec![
                "codestory-cli ready --goal agent --repair --project C:/repo --format json"
                    .to_string(),
            ],
            full_repair: Vec::new(),
            setup: None,
            index: None,
            sidecar: None,
        };

        let lane = readiness_lane_output(
            "agent_packet_search",
            &sidecar,
            Some(&aggregate_verdict),
            "C:/repo",
        );

        assert_eq!(lane.status, ReadinessStatusDto::Ready);
        assert_eq!(lane.sidecar_mode, "full");
        assert_eq!(lane.profile, "agent");
        assert_eq!(lane.run_id.as_deref(), Some("run"));
        assert!(
            lane.next_command.as_deref().is_some_and(|command| command
                .contains("retrieval status")
                && command.contains("--profile agent")
                && command.contains("--run-id")
                && command.contains("--format json")),
            "ready agent lane should point at lane-scoped status proof: {lane:?}"
        );
    }

    #[test]
    fn packet_markdown_labels_use_public_wire_values() {
        assert_eq!(
            packet_budget_mode_label(PacketBudgetModeDto::Compact),
            "compact"
        );
        assert_eq!(
            packet_task_class_label(PacketTaskClassDto::ArchitectureExplanation),
            "architecture_explanation"
        );
        assert_eq!(
            packet_task_class_label(PacketTaskClassDto::BugLocalization),
            "bug_localization"
        );
    }

    #[test]
    fn index_next_commands_stop_at_check_index_when_freshness_not_checked() {
        let freshness = IndexFreshnessDto {
            status: IndexFreshnessStatusDto::NotChecked,
            changed_file_count: 0,
            new_file_count: 0,
            removed_file_count: 0,
            checked_file_count: 0,
            indexed_file_count: 1,
            duration_ms: 0,
            reason: Some("bounded inventory overflow".to_string()),
            samples: Vec::new(),
        };

        let commands = index_next_commands("C:/repo", None, Some(&freshness), true);
        let joined = commands.join("\n");

        assert!(
            joined.contains("codestory-cli index")
                && joined.contains("--refresh full")
                && joined.contains("codestory-cli doctor")
                && joined.contains("--format markdown"),
            "not-checked freshness should recommend index verification before proof commands: {joined}"
        );
        for blocked in ["ground", "search", "context"] {
            assert!(
                !joined.contains(&format!("codestory-cli {blocked} ")),
                "not-checked freshness should stop before `{blocked}` proof/navigation commands: {joined}"
            );
        }
    }

    #[test]
    fn files_markdown_reports_incomplete_reason_text() {
        let output = IndexedFilesDto {
            project_root: "C:/repo".to_string(),
            usable: true,
            summary: IndexedFilesSummaryDto {
                file_count: 1,
                indexed_file_count: 1,
                filtered_file_count: 1,
                visible_file_count: 1,
                incomplete_file_count: 1,
                error_file_count: 0,
                incomplete_reason_counts: vec![IndexedFileIncompleteReasonCountDto {
                    reason: "unknown".to_string(),
                    file_count: 1,
                    detail: "incomplete with no recorded file-level error; run a full reindex"
                        .to_string(),
                }],
                truncated: false,
                language_counts: Vec::new(),
                framework_route_coverage: Vec::new(),
                coverage_notes: Vec::new(),
            },
            files: vec![IndexedFileDto {
                path: "src/lib.rs".to_string(),
                language: "rust".to_string(),
                indexed: true,
                complete: false,
                line_count: 1,
                role: IndexedFileRoleDto::Source,
                error_count: 0,
            }],
        };

        let markdown = render_files_markdown(&output);

        assert!(
            markdown.contains("- incomplete_reasons: unknown=1"),
            "{markdown}"
        );
        assert!(
            markdown.contains("run a full reindex"),
            "incomplete counts need operator-actionable reason text: {markdown}"
        );
    }

    #[test]
    fn affected_name_status_parser_preserves_status_and_renames() {
        let records = parse_git_name_status_records(
            "M\tcrates/codestory-cli/src/main.rs\nD\tsrc/old.ts\nR100\tsrc/before.ts\tsrc/after.ts\nC75\tsrc/base.ts\tsrc/copy.ts\n",
        )
        .expect("parse name-status");

        assert_eq!(records[0].kind, AffectedChangeKindDto::Modified);
        assert_eq!(records[0].status, "M");
        assert_eq!(records[1].kind, AffectedChangeKindDto::Deleted);
        assert_eq!(records[2].kind, AffectedChangeKindDto::Renamed);
        assert_eq!(records[2].previous_path.as_deref(), Some("src/before.ts"));
        assert_eq!(records[2].path, "src/after.ts");
        assert_eq!(records[3].kind, AffectedChangeKindDto::Copied);
        assert_eq!(records[3].previous_path.as_deref(), Some("src/base.ts"));
    }

    fn sample_retrieval() -> RetrievalStateDto {
        RetrievalStateDto {
            mode: RetrievalModeDto::Hybrid,
            hybrid_configured: true,
            semantic_ready: true,
            semantic_mode: SemanticModeDto::Enabled,
            semantic_doc_count: 42,
            embedding_model: Some("sentence-transformers/all-MiniLM-L6-v2-local".to_string()),
            current_embedding: None,
            stored_embedding: None,
            fallback_reason: None,
            fallback_message: None,
        }
    }

    fn sample_agent_answer_with_graph(graph: GraphArtifactDto) -> AgentAnswerDto {
        AgentAnswerDto {
            answer_id: "context-test".to_string(),
            prompt: "capped_bundle".to_string(),
            summary: "Bundle summary".to_string(),
            freshness: None,
            sections: Vec::new(),
            citations: Vec::new(),
            subgraph_ids: vec!["big-mermaid".to_string()],
            retrieval_version: "test".to_string(),
            graphs: vec![graph],
            retrieval_trace: AgentRetrievalTraceDto {
                request_id: "context-test".to_string(),
                resolved_profile: AgentRetrievalPresetDto::Architecture,
                policy_mode: AgentRetrievalPolicyModeDto::LatencyFirst,
                total_latency_ms: 1,
                sla_target_ms: None,
                sla_missed: false,
                semantic_fallback_count: 0,
                semantic_fallbacks: Vec::new(),
                annotations: Vec::new(),
                steps: Vec::new(),
                packet_sidecar_diagnostics: Vec::new(),
                retrieval_shadow: None,
            },
        }
    }

    fn sample_task_brief_packet() -> AgentPacketDto {
        let source = sample_task_brief_citation(
            "run_`packet_$env:SECRET$('x')",
            NodeKind::FUNCTION,
            "crates/codestory-cli/src/`main_$env:SECRET$('x').rs",
            1053,
        );
        let test = sample_task_brief_citation(
            "packet_tool_returns_budgeted_sufficiency_contract",
            NodeKind::FUNCTION,
            "crates/codestory-cli/tests/stdio`$env:SECRET$('x')_protocol_contracts.rs",
            2909,
        );
        AgentPacketDto {
            packet_id: "packet-task-brief".to_string(),
            question: "Add `$env:SECRET $(Get-ChildItem) 'literal' task brief".to_string(),
            task_class: Some(PacketTaskClassDto::EditPlanning),
            plan: PacketPlanDto {
                task_class: PacketTaskClassDto::EditPlanning,
                inferred_task_class: false,
                queries: vec![PacketPlanQueryDto {
                    query: "task brief packet surface".to_string(),
                    purpose: "find packet entry points".to_string(),
                }],
                trace: Vec::new(),
            },
            answer: AgentAnswerDto {
                answer_id: "answer-task-brief".to_string(),
                prompt: "Add `$env:SECRET $(Get-ChildItem) 'literal' task brief".to_string(),
                summary: "Use the packet command path.".to_string(),
                freshness: None,
                sections: Vec::new(),
                citations: vec![source.clone(), test.clone()],
                subgraph_ids: Vec::new(),
                retrieval_version: "sidecar".to_string(),
                graphs: Vec::new(),
                retrieval_trace: AgentRetrievalTraceDto {
                    request_id: "trace-task-brief".to_string(),
                    resolved_profile: AgentRetrievalPresetDto::Architecture,
                    policy_mode: AgentRetrievalPolicyModeDto::LatencyFirst,
                    total_latency_ms: 1,
                    sla_target_ms: None,
                    sla_missed: false,
                    semantic_fallback_count: 0,
                    semantic_fallbacks: Vec::new(),
                    annotations: Vec::new(),
                    steps: Vec::new(),
                    packet_sidecar_diagnostics: Vec::new(),
                    retrieval_shadow: None,
                },
            },
            budget: PacketBudgetDto {
                requested: PacketBudgetModeDto::Compact,
                limits: PacketBudgetLimitsDto {
                    max_anchors: 8,
                    max_files: 8,
                    max_snippets: 4,
                    max_trail_edges: 12,
                    max_output_bytes: 32_000,
                },
                used: PacketBudgetUsageDto {
                    anchors: 2,
                    files: 2,
                    snippets: 0,
                    trail_edges: 0,
                    output_bytes: 1024,
                },
                truncated: false,
                omitted_sections: Vec::new(),
                next_deeper_command: None,
            },
            sufficiency: PacketSufficiencyDto {
                status: PacketSufficiencyStatusDto::Partial,
                covered_claims: vec![PacketClaimDto {
                    claim: "Packet command is the starting point.".to_string(),
                    proof_status: None,
                    required_evidence_role: None,
                    citations: vec![source, test],
                    coverage_role: None,
                    eligible_for_sufficiency: None,
                }],
                open_next: Vec::new(),
                avoid_opening: Vec::new(),
                avoid_opening_paths: Vec::new(),
                gaps: vec!["verify `changed` files after editing".to_string()],
                follow_up_commands: vec![
                    "codestory-cli ready --goal agent --repair --project . --format json"
                        .to_string(),
                ],
                coverage_report: None,
            },
            retrieval_trace_summary: PacketRetrievalTraceSummaryDto {
                retrieval_trace: AgentRetrievalTraceDto {
                    request_id: "trace-task-brief".to_string(),
                    resolved_profile: AgentRetrievalPresetDto::Architecture,
                    policy_mode: AgentRetrievalPolicyModeDto::LatencyFirst,
                    total_latency_ms: 1,
                    sla_target_ms: None,
                    sla_missed: false,
                    semantic_fallback_count: 0,
                    semantic_fallbacks: Vec::new(),
                    annotations: Vec::new(),
                    steps: Vec::new(),
                    packet_sidecar_diagnostics: Vec::new(),
                    retrieval_shadow: None,
                },
                source_read_steps: 0,
                search_steps: 1,
                trail_steps: 0,
            },
        }
    }

    fn sample_task_brief_citation(
        display_name: &str,
        kind: NodeKind,
        file_path: &str,
        line: u32,
    ) -> AgentCitationDto {
        AgentCitationDto {
            node_id: NodeId(format!("{file_path}:{line}")),
            display_name: display_name.to_string(),
            kind,
            file_path: Some(file_path.to_string()),
            line: Some(line),
            score: 1.0,
            origin: SearchHitOrigin::IndexedSymbol,
            resolvable: true,
            subgraph_id: None,
            evidence_edge_ids: Vec::new(),
            retrieval_score_breakdown: None,
            evidence_tier: None,
            evidence_producer: None,
            resolution_status: None,
            loss_reason: None,
            coverage_role: None,
            eligible_for_sufficiency: None,
        }
    }

    fn summary_with_files(file_count: u32) -> ProjectSummary {
        ProjectSummary {
            root: "C:/repo".to_string(),
            stats: StorageStatsDto {
                node_count: file_count.saturating_mul(10),
                edge_count: 0,
                file_count,
                error_count: 0,
                fatal_error_count: 0,
            },
            members: Vec::new(),
            retrieval: None,
            freshness: None,
        }
    }

    fn sample_phase_timings() -> IndexingPhaseTimings {
        IndexingPhaseTimings {
            parse_index_ms: 10,
            projection_flush_ms: 20,
            edge_resolution_ms: 30,
            error_flush_ms: 4,
            cleanup_ms: 5,
            cache_refresh_ms: Some(6),
            search_projection_rebuild_ms: Some(61),
            search_symbol_index_ms: Some(62),
            runtime_cache_publish_ms: Some(63),
            semantic_doc_build_ms: Some(7),
            semantic_embedding_ms: Some(8),
            semantic_db_upsert_ms: Some(9),
            semantic_reload_ms: Some(10),
            semantic_prune_ms: Some(64),
            semantic_docs_reused: Some(11),
            semantic_docs_embedded: Some(12),
            semantic_docs_pending: Some(13),
            semantic_docs_stale: Some(14),
            symbol_search_docs_written: Some(15),
            semantic_dense_docs_skipped: Some(16),
            semantic_dense_public_api: Some(17),
            semantic_dense_entrypoint: Some(18),
            semantic_dense_documented_nontrivial: Some(19),
            semantic_dense_central_graph_node: Some(20),
            semantic_dense_component_report: Some(21),
            semantic_dense_unstructured_doc: Some(22),
            deferred_indexes_ms: Some(7),
            summary_snapshot_ms: Some(8),
            detail_snapshot_ms: Some(9),
            publish_ms: Some(10),
            setup_existing_projection_ids_ms: Some(11),
            setup_seed_symbol_table_ms: Some(12),
            flush_files_ms: Some(13),
            flush_nodes_ms: Some(14),
            flush_edges_ms: Some(15),
            flush_occurrences_ms: Some(16),
            flush_component_access_ms: Some(17),
            flush_callable_projection_ms: Some(18),
            unresolved_calls_start: 19,
            unresolved_imports_start: 20,
            resolved_calls: 21,
            resolved_imports: 22,
            unresolved_calls_end: 23,
            unresolved_imports_end: 24,
            resolution_override_count_ms: Some(25),
            resolution_unresolved_counts_ms: Some(26),
            resolution_calls_ms: Some(27),
            resolution_imports_ms: Some(28),
            resolution_cleanup_ms: Some(29),
            resolution_call_candidate_index_ms: Some(30),
            resolution_import_candidate_index_ms: Some(31),
            resolution_call_semantic_index_ms: Some(32),
            resolution_import_semantic_index_ms: Some(33),
            resolution_call_semantic_candidates_ms: Some(34),
            resolution_import_semantic_candidates_ms: Some(35),
            resolution_call_semantic_requests: Some(36),
            resolution_call_semantic_unique_requests: Some(37),
            resolution_call_semantic_skipped_requests: Some(38),
            resolution_import_semantic_requests: Some(39),
            resolution_import_semantic_unique_requests: Some(40),
            resolution_import_semantic_skipped_requests: Some(41),
            resolution_call_compute_ms: Some(42),
            resolution_import_compute_ms: Some(43),
            resolution_call_apply_ms: Some(44),
            resolution_import_apply_ms: Some(45),
            resolution_override_resolution_ms: Some(46),
            resolved_calls_same_file: Some(47),
            resolved_calls_same_module: Some(48),
            resolved_calls_global_unique: Some(49),
            resolved_calls_semantic: Some(50),
            resolved_imports_same_file: Some(51),
            resolved_imports_same_module: Some(52),
            resolved_imports_global_unique: Some(53),
            resolved_imports_fuzzy: Some(54),
            resolved_imports_semantic: Some(55),
        }
    }

    fn sample_drill_runtime_timings() -> DrillRuntimeTimingsOutput {
        DrillRuntimeTimingsOutput {
            total_ms: 42,
            setup_ms: 3,
            question_search_ms: 5,
            anchor_resolution_ms: 13,
            supplemental_search_ms: 7,
            bridge_evidence_ms: 11,
            evidence_assembly_ms: 3,
        }
    }

    fn sample_node_details(id: &str, display_name: &str) -> NodeDetailsDto {
        NodeDetailsDto {
            id: NodeId(id.to_string()),
            kind: codestory_contracts::api::NodeKind::FUNCTION,
            display_name: display_name.to_string(),
            serialized_name: display_name.to_string(),
            qualified_name: None,
            canonical_id: None,
            file_path: None,
            start_line: None,
            start_col: None,
            end_line: None,
            end_col: None,
            member_access: None,
            route_endpoint: None,
        }
    }

    fn sample_graph_node(id: &str, label: &str) -> GraphNodeDto {
        GraphNodeDto {
            id: NodeId(id.to_string()),
            label: label.to_string(),
            kind: codestory_contracts::api::NodeKind::FUNCTION,
            depth: 0,
            label_policy: None,
            badge_visible_members: None,
            badge_total_members: None,
            merged_symbol_examples: Vec::new(),
            file_path: None,
            qualified_name: None,
            member_access: None,
        }
    }

    fn sample_graph_edge(
        id: &str,
        source: &str,
        target: &str,
        certainty: Option<&str>,
    ) -> GraphEdgeDto {
        sample_graph_edge_with_kind(id, source, target, EdgeKind::CALL, certainty)
    }

    fn sample_graph_edge_with_kind(
        id: &str,
        source: &str,
        target: &str,
        kind: EdgeKind,
        certainty: Option<&str>,
    ) -> GraphEdgeDto {
        GraphEdgeDto {
            id: EdgeId(id.to_string()),
            source: NodeId(source.to_string()),
            target: NodeId(target.to_string()),
            kind,
            confidence: None,
            certainty: certainty.map(ToOwned::to_owned),
            callsite_identity: None,
            candidate_targets: Vec::new(),
        }
    }

    fn sample_search_hit_output(id: &str, name: &str) -> SearchHitOutput {
        SearchHitOutput {
            number: None,
            node_id: id.to_string(),
            node_ref: Some(format!("src/lib.rs:1:{name}")),
            display_name: name.to_string(),
            kind: NodeKind::FUNCTION,
            file_path: Some("src/lib.rs".to_string()),
            line: Some(1),
            score: 1.0,
            origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
            match_quality: SearchMatchQualityDto::Exact,
            resolvable: true,
            score_breakdown: None,
            duplicate_of: None,
            excerpt: None,
            primary_occurrence_kind: None,
            symbol_role: None,
            paired_refs: Vec::new(),
            verification_targets: Vec::new(),
            resolution_hints: Vec::new(),
            why: Vec::new(),
        }
    }

    fn sample_drill_anchor(anchor: &str, node_id: &str) -> DrillAnchorOutput {
        DrillAnchorOutput {
            anchor: anchor.to_string(),
            typed_hit_count: 1,
            chosen_anchor: Some(sample_search_hit_output(node_id, anchor)),
            verification_targets: Vec::new(),
            consumer_summary: None,
            timings: DrillAnchorTimingsOutput::default(),
            commands: Vec::new(),
        }
    }

    fn sample_drill_anchor_with_file(anchor: &str, node_id: &str, path: &str) -> DrillAnchorOutput {
        let mut output = sample_drill_anchor(anchor, node_id);
        if let Some(hit) = output.chosen_anchor.as_mut() {
            hit.node_ref = Some(format!("{path}:1:{anchor}"));
            hit.file_path = Some(path.to_string());
        }
        output
    }

    fn add_text_hint(anchor: &mut DrillAnchorOutput, path: &str) {
        anchor.consumer_summary = Some(DrillAnchorConsumerSummaryOutput {
            caller_count: 0,
            consumer_count: 0,
            text_hint_count: 1,
            truncated: false,
            omitted_edge_count: 0,
            callers: Vec::new(),
            consumers: Vec::new(),
            text_consumer_hints: vec![DrillAnchorTextConsumerHintOutput {
                name: format!("{} usage", anchor.anchor),
                kind: NodeKind::FUNCTION,
                file_path: Some(path.to_string()),
                line: Some(12),
                score: 1.0,
            }],
            notes: Vec::new(),
        });
    }

    fn sample_bridge_trail(truncated: bool) -> TrailContextDto {
        TrailContextDto {
            focus: sample_node_details("a", "A"),
            trail: GraphResponse {
                center_id: NodeId("a".to_string()),
                nodes: vec![sample_graph_node("a", "A"), sample_graph_node("b", "B")],
                edges: vec![sample_graph_edge("e1", "a", "b", Some("certain"))],
                truncated,
                omitted_edge_count: if truncated { 1 } else { 0 },
                canonical_layout: None,
            },
            story: None,
        }
    }

    #[test]
    fn symbol_workflow_transitive_callers_render_only_call_paths() {
        let focus = sample_graph_node("focus", "Focus");
        let mut direct = sample_graph_node("direct", "DirectCaller");
        let mut transitive = sample_graph_node("transitive", "TransitiveCaller");
        let mut reference = sample_graph_node("reference", "ReferenceOnly");
        direct.depth = 1;
        transitive.depth = 2;
        reference.depth = 2;
        let trail = TrailContextDto {
            focus: sample_node_details("focus", "Focus"),
            trail: GraphResponse {
                center_id: NodeId("focus".to_string()),
                nodes: vec![focus, direct, transitive, reference],
                edges: vec![
                    sample_graph_edge("direct-focus", "direct", "focus", Some("certain")),
                    sample_graph_edge("transitive-direct", "transitive", "direct", Some("certain")),
                    sample_graph_edge_with_kind(
                        "reference-direct",
                        "reference",
                        "direct",
                        EdgeKind::USAGE,
                        Some("certain"),
                    ),
                ],
                truncated: false,
                omitted_edge_count: 0,
                canonical_layout: None,
            },
            story: None,
        };
        let symbol = codestory_contracts::api::SymbolContextDto {
            node: sample_node_details("focus", "Focus"),
            summary: None,
            children: Vec::new(),
            related_hits: Vec::new(),
            edge_digest: Vec::new(),
        };
        let direct_callers = symbol_workflow_direct_callers(&trail);
        let transitive_callers = symbol_workflow_transitive_callers(&trail, &direct_callers);
        let output = SymbolWorkflowOutput {
            workflow: SymbolWorkflowKind::Impact.label(),
            project_root: "C:/repo".to_string(),
            resolution: QueryResolutionOutput {
                selector: QuerySelectorOutput::Id,
                requested: "focus".to_string(),
                file_filter: None,
                resolved: sample_search_hit_output("focus", "Focus"),
                alternatives: Vec::new(),
            },
            symbol: &symbol,
            direct_callers,
            transitive_callers,
            impacted_files: Vec::new(),
            impacted_routes: Vec::new(),
            likely_tests: Vec::new(),
            caps: SymbolWorkflowCapsOutput {
                caller_depth: 3,
                caller_max_nodes: 20,
                affected_depth: 3,
                impacted_symbols_cap: 200,
                impacted_routes_cap: 100,
                affected_seed: "none".to_string(),
            },
            unknowns: Vec::new(),
            next_commands: Vec::new(),
            affected: None,
            trail: &trail,
        };

        let markdown = render_symbol_workflow_markdown(SymbolWorkflowKind::Impact, &output);

        assert!(markdown.contains("transitive_callers:"));
        assert!(markdown.contains("TransitiveCaller"));
        assert!(
            !markdown.contains("ReferenceOnly"),
            "non-CALL depth-2 nodes must not render as transitive callers:\n{markdown}"
        );
    }

    #[test]
    fn symbol_workflow_next_commands_preserve_include_tests_scope() {
        let commands = symbol_workflow_next_commands(
            Path::new("C:/repo"),
            &NodeId("focus".to_string()),
            None,
            3,
            20,
            SymbolWorkflowKind::Impact,
            true,
        );

        assert!(
            commands
                .iter()
                .any(|command| command.contains("callers") && command.contains("--include-tests")),
            "callers next command should preserve include-tests scope: {commands:#?}"
        );
        assert!(
            commands
                .iter()
                .any(|command| command.contains("test-map") && command.contains("--include-tests")),
            "paired workflow next command should preserve include-tests scope: {commands:#?}"
        );
    }

    fn sample_drill_output_with_source_truth_files(paths: Vec<&str>) -> DrillOutput {
        let from = sample_drill_anchor("WorkspaceIndexer", "a");
        let to = sample_drill_anchor("SearchService", "b");
        let mut bridge = DrillBridgeOutput {
            evidence: fallback_drill_bridge(
                Path::new("C:/repo"),
                &from,
                &to,
                from.chosen_anchor.clone().expect("from"),
                to.chosen_anchor.clone().expect("to"),
                &sample_bridge_trail(false),
                Vec::new(),
                false,
            ),
            command: DrillCommandStatusOutput {
                command: "bridge".to_string(),
                status: "ok".to_string(),
                duration_ms: 2,
                artifact: Some("bridge.md".to_string()),
                error: None,
            },
        };
        bridge.evidence.evidence_files = paths.iter().map(|path| (*path).to_string()).collect();
        let source_truth_checks = paths
            .into_iter()
            .enumerate()
            .map(|(index, path)| SourceTruthCheckDto {
                id: format!("source-truth-{}", index + 1),
                reason: "verify surface ordering".to_string(),
                path: path.to_string(),
                line: None,
                required: true,
            })
            .collect::<Vec<_>>();
        DrillOutput {
            project: "C:/repo".to_string(),
            label: Some("sample".to_string()),
            question: Some("How does indexing work?".to_string()),
            output_dir: "target/drill/sample".to_string(),
            mechanical: DrillMechanicalOutput {
                before_files: 1,
                before_nodes: 10,
                before_edges: 2,
                before_errors: 0,
                after_files: 2,
                after_nodes: 20,
                after_edges: 4,
                after_errors: 0,
                refresh: "full".to_string(),
                retrieval: Some(sample_retrieval()),
                sidecar_retrieval_mode: Some("full".to_string()),
                freshness: None,
                phase_timings: Some(sample_phase_timings()),
                drill_timings: sample_drill_runtime_timings(),
            },
            question_search: None,
            question_supplemental_searches: Vec::new(),
            anchors: vec![from, to],
            bridges: vec![bridge],
            execution_boundaries: drill_execution_boundaries(),
            verification_targets: Vec::new(),
            evidence_packet: EvidencePacketDto {
                packet_version: 1,
                question: Some("How does indexing work?".to_string()),
                items: Vec::new(),
                readiness: AnswerReadinessReportDto {
                    overall_status: ClaimReadinessDto::NeedsSourceRead,
                    safe_to_say: Vec::new(),
                    inferred_claims: Vec::new(),
                    needs_verification: Vec::new(),
                    next_commands: Vec::new(),
                    source_truth_checks,
                },
            },
            answer_quality_contract: drill_answer_quality_contract(),
            claim_ledger_template: DrillClaimLedgerOutput {
                template_version: 1,
                instructions: Vec::new(),
                claims: Vec::new(),
                scoring: DrillClaimLedgerScoringOutput {
                    status: "pending_source_verification".to_string(),
                    pending_claim_count: 0,
                    correct: 0,
                    partial: 0,
                    misleading: 0,
                    unsupported: 0,
                    material_revision_count: 0,
                    score_formula: "manual".to_string(),
                },
            },
            verification_checklist: drill_verification_checklist(),
            next_commands: Vec::new(),
        }
    }

    #[test]
    fn drill_bridge_search_hints_keep_middle_source_files() {
        fn search_hit(
            name: &str,
            path: &str,
            origin: codestory_contracts::api::SearchHitOrigin,
        ) -> SearchHit {
            SearchHit {
                node_id: NodeId(format!("{path}:{name}")),
                display_name: name.to_string(),
                kind: NodeKind::FUNCTION,
                file_path: Some(path.to_string()),
                line: Some(1),
                score: 1.0,
                origin,
                match_quality: None,
                resolvable: true,
                score_breakdown: None,
                ..test_search_hit_defaults()
            }
        }

        let endpoint_files = vec![
            "lib/axios.js".to_string(),
            "lib/core/dispatchRequest.js".to_string(),
        ];
        let repo_text_hits = vec![
            search_hit(
                "Axios.js",
                "lib/core/Axios.js",
                codestory_contracts::api::SearchHitOrigin::TextMatch,
            ),
            search_hit(
                "axios.js",
                "lib/axios.js",
                codestory_contracts::api::SearchHitOrigin::TextMatch,
            ),
            search_hit(
                "bundle.js",
                "dist/bundle.js",
                codestory_contracts::api::SearchHitOrigin::TextMatch,
            ),
        ];
        let indexed_symbol_hits = vec![
            search_hit(
                "Axios",
                "lib/core/Axios.js",
                codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
            ),
            search_hit(
                "InterceptorManager",
                "lib/core/InterceptorManager.js",
                codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
            ),
        ];

        let files = drill_bridge_search_hint_files_from_hits(
            Path::new("C:/repo"),
            &endpoint_files,
            &repo_text_hits,
            &indexed_symbol_hits,
        );

        assert_eq!(
            files,
            vec![
                "lib/core/Axios.js".to_string(),
                "lib/core/InterceptorManager.js".to_string()
            ]
        );
    }

    #[test]
    fn drill_import_hub_helpers_resolve_relative_js_imports() {
        let temp = tempdir().expect("temp dir");
        let lib_dir = temp.path().join("lib");
        let core_dir = lib_dir.join("core");
        fs::create_dir_all(&core_dir).expect("create dirs");
        let endpoint = lib_dir.join("axios.js");
        let axios_core = core_dir.join("Axios.js");
        fs::write(
            &endpoint,
            "import Axios from './core/Axios.js';\nimport './polyfill.js';\n",
        )
        .expect("write endpoint");
        fs::write(
            &axios_core,
            "import dispatchRequest from './dispatchRequest.js';\nclass Axios {}\n",
        )
        .expect("write candidate");
        let outside = temp.path().with_file_name(format!(
            "{}-outside.js",
            temp.path().file_name().unwrap().to_string_lossy()
        ));
        fs::write(&outside, "class Outside {}\n").expect("write outside file");

        let source = fs::read_to_string(&endpoint).expect("read endpoint");
        let specifiers = drill_js_relative_import_specifiers(&source);

        assert_eq!(
            specifiers,
            vec!["./core/Axios.js".to_string(), "./polyfill.js".to_string()]
        );
        assert_eq!(
            drill_resolve_relative_import(temp.path(), &endpoint, "./core/Axios.js"),
            Some(fs::canonicalize(&axios_core).expect("canonical axios core"))
        );
        assert_eq!(
            drill_relative_source_path(temp.path(), &axios_core.to_string_lossy()),
            None
        );
        assert_eq!(
            drill_relative_source_path(temp.path(), "../outside.js"),
            None
        );
        assert_eq!(
            drill_resolve_relative_import(
                temp.path(),
                &endpoint,
                &format!("../{}", outside.file_name().unwrap().to_string_lossy())
            ),
            None
        );
        assert_eq!(
            drill_resolve_relative_import(temp.path(), &endpoint, &outside.to_string_lossy()),
            None
        );
        assert!(drill_file_contains_terms(
            temp.path(),
            "lib/core/Axios.js",
            &["dispatchRequest", "Axios"]
        ));
        assert!(!drill_file_contains_terms(
            temp.path(),
            "lib/core/Axios.js",
            &["createInstance", "dispatchRequest"]
        ));
    }

    #[test]
    fn drill_bridge_constructors_preserve_status_contract() {
        let from = sample_drill_anchor("FromAnchor", "a");
        let to = sample_drill_anchor("ToAnchor", "b");
        let project_root = Path::new("C:/repo");
        let complete_trail = sample_bridge_trail(false);
        let truncated_trail = sample_bridge_trail(true);

        let forward = graph_path_drill_bridge(
            project_root,
            &from,
            &to,
            from.chosen_anchor.clone().expect("from"),
            to.chosen_anchor.clone().expect("to"),
            &complete_trail,
            false,
        );
        assert_eq!(forward.status, "graph_path");
        assert_eq!(forward.strategy, "to_target_symbol_forward");
        assert_eq!(forward.confidence, "high");
        assert_eq!(forward.graph_path.as_ref().expect("path").mode, "forward");
        assert_eq!(forward.endpoint_files, vec!["src/lib.rs"]);

        let reverse = reverse_graph_path_drill_bridge(
            project_root,
            &from,
            &to,
            from.chosen_anchor.clone().expect("from"),
            to.chosen_anchor.clone().expect("to"),
            &truncated_trail,
            false,
        );
        assert_eq!(reverse.status, "reverse_graph_path");
        assert_eq!(reverse.strategy, "to_target_symbol_reverse");
        assert_eq!(reverse.confidence, "low");
        assert_eq!(reverse.graph_path.as_ref().expect("path").mode, "reverse");

        let shared_file = fallback_drill_bridge(
            project_root,
            &from,
            &to,
            from.chosen_anchor.clone().expect("from"),
            to.chosen_anchor.clone().expect("to"),
            &complete_trail,
            vec!["src/lib.rs".to_string()],
            false,
        );
        assert_eq!(shared_file.status, "graph_shared_file");
        assert_eq!(
            shared_file.strategy,
            "to_target_symbol_then_graph_shared_files"
        );
        assert_eq!(shared_file.confidence, "medium");

        let mut hinted_from = sample_drill_anchor("FromAnchor", "a");
        hinted_from.consumer_summary = Some(DrillAnchorConsumerSummaryOutput {
            caller_count: 0,
            consumer_count: 0,
            text_hint_count: 1,
            truncated: false,
            omitted_edge_count: 0,
            callers: Vec::new(),
            consumers: Vec::new(),
            text_consumer_hints: vec![DrillAnchorTextConsumerHintOutput {
                name: "FromAnchorUser".to_string(),
                kind: NodeKind::FUNCTION,
                file_path: Some("src/from-user.rs".to_string()),
                line: Some(12),
                score: 1.0,
            }],
            notes: Vec::new(),
        });
        let hint_only = fallback_drill_bridge(
            project_root,
            &hinted_from,
            &to,
            hinted_from.chosen_anchor.clone().expect("from"),
            to.chosen_anchor.clone().expect("to"),
            &complete_trail,
            Vec::new(),
            false,
        );
        assert_eq!(hint_only.status, "evidence_hint_only");
        assert_eq!(
            hint_only.strategy,
            "to_target_symbol_then_consumer_text_hints"
        );
        assert_eq!(hint_only.endpoint_files, vec!["src/lib.rs"]);
        assert_eq!(hint_only.evidence_files, vec!["src/from-user.rs"]);
        assert!(
            hint_only
                .next_commands
                .iter()
                .any(|command| command.contains("search --project")
                    && command.contains("FromAnchor ToAnchor")),
            "hint-only bridges should keep a bridge search follow-up: {hint_only:#?}"
        );
        assert!(
            hint_only
                .next_commands
                .iter()
                .any(|command| command.contains("trail --project") && command.contains("--id")),
            "hint-only bridges should keep id-based trail follow-up: {hint_only:#?}"
        );
        assert!(
            hint_only
                .next_commands
                .iter()
                .any(|command| command.contains("snippet --project")
                    && command.contains("--id")
                    && command.contains("--function-body")),
            "hint-only bridges should keep id-based function-body snippets: {hint_only:#?}"
        );
        assert!(
            hint_only
                .next_commands
                .iter()
                .any(|command| command.contains("src/from-user.rs")),
            "hint-only bridges should search the text-hint evidence file: {hint_only:#?}"
        );
        let hint_markdown = render_drill_bridge_markdown(&hint_only);
        assert!(hint_markdown.contains("next_commands:"));
        assert!(hint_markdown.contains("src/from-user.rs"));

        let shared_with_hints = fallback_drill_bridge(
            project_root,
            &hinted_from,
            &to,
            hinted_from.chosen_anchor.clone().expect("from"),
            to.chosen_anchor.clone().expect("to"),
            &complete_trail,
            vec!["src/shared.rs".to_string()],
            false,
        );
        assert_eq!(shared_with_hints.status, "graph_shared_file");
        assert_eq!(shared_with_hints.shared_files, vec!["src/shared.rs"]);
        assert_eq!(shared_with_hints.evidence_files, vec!["src/from-user.rs"]);

        let missing = fallback_drill_bridge(
            project_root,
            &from,
            &to,
            from.chosen_anchor.clone().expect("from"),
            to.chosen_anchor.clone().expect("to"),
            &complete_trail,
            Vec::new(),
            false,
        );
        assert_eq!(missing.status, "no_bridge_found");

        let error = drill_bridge_error(
            project_root,
            &from,
            &to,
            from.chosen_anchor.clone().expect("from"),
            to.chosen_anchor.clone().expect("to"),
            "internal",
            "failed",
            false,
        );
        assert_eq!(error.status, "error");
        assert_eq!(error.strategy, "to_target_symbol_forward");
        assert!(error.notes[0].contains("internal: failed"));
        assert!(error.graph_path.is_none());
    }

    #[test]
    fn drill_fallback_bridge_promotes_typed_source_truth_candidates() {
        let project_root = Path::new("C:/repo");
        let complete_trail = sample_bridge_trail(false);
        assert!(drill_path_is_framework_route_or_page(
            "src/app/(frontend)/posts/[slug]/comments/route.ts"
        ));
        assert!(!drill_path_is_framework_route_or_page(
            "src/lib/app/Application.cpp"
        ));

        let posts = sample_drill_anchor_with_file("Posts", "posts", "src/collections/Posts.ts");
        let mut comment_auth =
            sample_drill_anchor_with_file("getCommentAuth", "auth", "src/lib/comment-auth.ts");
        add_text_hint(
            &mut comment_auth,
            "src/app/(frontend)/posts/[slug]/comments/route.ts",
        );

        let payload_bridge = fallback_drill_bridge(
            project_root,
            &posts,
            &comment_auth,
            posts.chosen_anchor.clone().expect("from"),
            comment_auth.chosen_anchor.clone().expect("to"),
            &complete_trail,
            Vec::new(),
            false,
        );
        assert_eq!(payload_bridge.status, "data_collection_usage");
        assert_eq!(
            payload_bridge.strategy,
            "payload_collection_usage_source_targets"
        );
        assert_eq!(payload_bridge.confidence, "medium");
        assert_eq!(payload_bridge.evidence_kind, "data_collection_usage");
        assert!(drill_bridge_status_is_partial(&payload_bridge.status));

        let payload_item = evidence_item_from_bridge(
            1,
            &DrillBridgeOutput {
                evidence: payload_bridge,
                command: DrillCommandStatusOutput {
                    command: "bridge".to_string(),
                    status: "ok".to_string(),
                    duration_ms: 2,
                    artifact: Some("bridge.md".to_string()),
                    error: None,
                },
            },
        );
        assert_eq!(payload_item.verification_status, ClaimReadinessDto::Partial);

        let source_group = sample_drill_anchor_with_file(
            "SourceGroupCxxCdb",
            "source-group",
            "src/lib_cxx/project/SourceGroupCxxCdb.h",
        );
        let mut storage = sample_drill_anchor_with_file(
            "StorageAccess",
            "storage",
            "src/lib/data/storage/StorageAccess.h",
        );
        add_text_hint(&mut storage, "src/lib/data/storage/StorageAccess.cpp");

        let native_bridge = fallback_drill_bridge(
            project_root,
            &source_group,
            &storage,
            source_group.chosen_anchor.clone().expect("from"),
            storage.chosen_anchor.clone().expect("to"),
            &complete_trail,
            Vec::new(),
            false,
        );
        assert_eq!(native_bridge.status, "source_truth_only");
        assert_eq!(
            native_bridge.strategy,
            "native_related_source_truth_targets"
        );
        assert_eq!(native_bridge.evidence_kind, "source_truth_only");
        assert!(drill_bridge_status_is_partial(&native_bridge.status));
        assert!(!drill_bridge_status_is_unresolved(&native_bridge.status));

        let native_item = evidence_item_from_bridge(
            2,
            &DrillBridgeOutput {
                evidence: native_bridge,
                command: DrillCommandStatusOutput {
                    command: "bridge".to_string(),
                    status: "ok".to_string(),
                    duration_ms: 2,
                    artifact: Some("bridge.md".to_string()),
                    error: None,
                },
            },
        );
        assert_eq!(
            native_item.verification_status,
            ClaimReadinessDto::NeedsSourceRead
        );
    }

    #[test]
    fn no_path_bridge_graphs_do_not_report_truncated_when_nothing_was_omitted() {
        let mut no_path = sample_bridge_trail(true);
        no_path.trail.nodes = vec![sample_graph_node("a", "A")];
        no_path.trail.edges.clear();
        no_path.trail.omitted_edge_count = 0;

        let graph =
            drill_bridge_graph_path_output(Path::new("C:/repo"), "forward_no_path", &no_path);

        assert_eq!(graph.edge_count, 0);
        assert_eq!(graph.omitted_edge_count, 0);
        assert!(
            !graph.truncated,
            "zero-edge no-path bridge payloads should not look like clipped path evidence"
        );

        no_path.trail.omitted_edge_count = 2;
        let omitted_graph =
            drill_bridge_graph_path_output(Path::new("C:/repo"), "forward_no_path", &no_path);

        assert!(omitted_graph.truncated);
        assert_eq!(omitted_graph.omitted_edge_count, 2);
    }

    #[test]
    fn drill_bridge_evidence_kind_distinguishes_framework_and_data_paths() {
        let mut payload = sample_bridge_trail(false);
        payload.trail.edges[0].callsite_identity = Some("payload:create:comments:7:18".to_string());
        assert_eq!(
            drill_bridge_evidence_kind_for_trail(&payload),
            "data_collection_usage"
        );

        let mut route = sample_bridge_trail(false);
        route.trail.nodes[0].label =
            "POST /api/comments (nextjs route; confidence=file_convention)".to_string();
        assert_eq!(
            drill_bridge_evidence_kind_for_trail(&route),
            "framework_route"
        );

        let mut component = sample_bridge_trail(false);
        component.trail.nodes[1].label = "RootRuntimeHome".to_string();
        component.trail.nodes[1].file_path = Some("src/components/RootRuntimeHome.tsx".to_string());
        assert_eq!(
            drill_bridge_evidence_kind_for_trail(&component),
            "component_usage"
        );
    }

    #[test]
    fn drill_native_related_queries_cover_sourcetrail_anchor_methods() {
        let source_group = SearchHit {
            node_id: NodeId("source-group".to_string()),
            display_name: "SourceGroupCxxCdb".to_string(),
            kind: codestory_contracts::api::NodeKind::CLASS,
            file_path: Some("src/lib_cxx/project/SourceGroupCxxCdb.h".to_string()),
            line: Some(12),
            score: 0.9,
            origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
            match_quality: None,
            resolvable: true,
            score_breakdown: None,
            ..test_search_hit_defaults()
        };
        let queries = drill_native_related_queries("SourceGroupCxxCdb", &source_group);
        assert!(
            queries.iter().any(|(relation, query)| {
                relation == "related_native_role:commands"
                    && query == "source group indexer commands"
            }),
            "source-group anchors should expand to role-based command queries: {queries:#?}"
        );

        let indexer = SearchHit {
            node_id: NodeId("indexer-java".to_string()),
            display_name: "IndexerJava".to_string(),
            ..source_group
        };
        let queries = drill_native_related_queries("IndexerJava", &indexer);
        assert!(
            queries.iter().any(|(relation, query)| {
                relation == "related_native_role:java_index" && query == "java indexer do index"
            }),
            "Java indexer anchors should expand to role-based parser dispatch queries: {queries:#?}"
        );

        let method_hit = SearchHit {
            node_id: NodeId("method".to_string()),
            display_name: "sourcetrail::SourceGroupCxxCdb::getIndexerCommands".to_string(),
            kind: codestory_contracts::api::NodeKind::METHOD,
            file_path: Some("src/lib_cxx/project/SourceGroupCxxCdb.cpp".to_string()),
            line: Some(44),
            score: 0.8,
            origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
            match_quality: None,
            resolvable: true,
            score_breakdown: None,
            ..test_search_hit_defaults()
        };
        assert!(drill_native_related_query_matches(
            &method_hit,
            "source group indexer commands"
        ));
    }

    #[test]
    fn bridge_evidence_files_rank_runtime_paths_before_auxiliary_files() {
        let mut files = vec![
            "crates/codestory-bench/benches/ask_latency.rs".to_string(),
            "scripts/import-wordpress-rich-content.ts".to_string(),
            "tests/int/social-feed.int.spec.ts".to_string(),
            "crates/codestory-runtime/src/services.rs".to_string(),
            "src/lib/comment-auth.ts".to_string(),
        ];

        rank_drill_bridge_evidence_files(&mut files);

        assert_eq!(files[0], "crates/codestory-runtime/src/services.rs");
        assert_eq!(files[1], "src/lib/comment-auth.ts");
        assert!(files[4].starts_with("scripts/"));
    }

    #[test]
    fn source_truth_files_rank_runtime_paths_before_auxiliary_files() {
        let mut output = sample_drill_output_with_source_truth_files(vec![
            "scripts/migrate-wordpress-rich-content.ts",
            "crates/codestory-bench/benches/ask_latency.rs",
            "crates/codestory-runtime/src/lib.rs",
            "tests/int/social-feed.int.spec.ts",
            "src/lib/comment-auth.ts",
        ]);

        let summary = drill_summary(&output);

        assert_eq!(
            summary.source_truth.target_files,
            vec![
                "crates/codestory-runtime/src/lib.rs",
                "src/lib/comment-auth.ts",
                "tests/int/social-feed.int.spec.ts",
                "crates/codestory-bench/benches/ask_latency.rs",
                "scripts/migrate-wordpress-rich-content.ts",
            ]
        );

        output.claim_ledger_template =
            drill_claim_ledger_template(&output.anchors, &output.bridges);
        let bridge_claim = output
            .claim_ledger_template
            .claims
            .iter()
            .find(|claim| claim.id == "bridge-1")
            .expect("bridge claim");
        let anchor_claim = output
            .claim_ledger_template
            .claims
            .iter()
            .find(|claim| claim.id == "anchor-1")
            .expect("anchor claim");
        assert!(
            anchor_claim
                .claim
                .contains("requires source-truth verification before the final answer")
        );
        assert!(
            !anchor_claim
                .claim
                .contains("CodeStory evidence is sufficient")
        );
        assert!(
            bridge_claim
                .claim
                .contains("requires source-truth verification before the final answer")
        );
        assert!(
            !bridge_claim
                .claim
                .contains("CodeStory evidence is sufficient")
        );
        assert!(
            bridge_claim
                .source_truth_files
                .first()
                .is_some_and(|path| path.contains("/src/") || path.starts_with("src/")),
            "bridge claim should send agents to runtime/source files before auxiliary files: {bridge_claim:#?}"
        );
    }

    #[test]
    fn source_truth_files_rank_public_surfaces_before_admin_and_generated_files() {
        let output = sample_drill_output_with_source_truth_files(vec![
            "src/components/admin/DashboardWidgets.tsx",
            "src/payload-types.ts",
            "tests/int/social-feed.int.spec.ts",
            "src/collections/SocialEntries.ts",
            "src/lib/content-data/social-entry-content.ts",
            "src/components/RootRuntimeHome.tsx",
            "src/app/(frontend)/page.tsx",
            "src/app/api/comments/route.ts",
            "src/lib/comment-auth.ts",
        ]);

        let summary = drill_summary(&output);

        assert_eq!(
            summary.source_truth.target_files,
            vec![
                "src/app/(frontend)/page.tsx",
                "src/app/api/comments/route.ts",
                "src/components/RootRuntimeHome.tsx",
                "src/lib/content-data/social-entry-content.ts",
                "src/lib/comment-auth.ts",
                "src/collections/SocialEntries.ts",
                "src/components/admin/DashboardWidgets.tsx",
                "tests/int/social-feed.int.spec.ts",
                "src/payload-types.ts",
            ]
        );
    }

    #[test]
    fn source_truth_checks_group_repeated_files_without_dropping_roles() {
        let mut from = sample_drill_anchor("WorkspaceIndexer", "a");
        from.consumer_summary = Some(DrillAnchorConsumerSummaryOutput {
            caller_count: 1,
            consumer_count: 1,
            text_hint_count: 1,
            truncated: false,
            omitted_edge_count: 0,
            callers: Vec::new(),
            consumers: vec![DrillAnchorConsumerOutput {
                name: "SearchService".to_string(),
                kind: NodeKind::STRUCT,
                file_path: Some("src/lib.rs".to_string()),
                qualified_name: None,
                target_name: Some("WorkspaceIndexer".to_string()),
                target_kind: Some(NodeKind::FUNCTION),
                target_file_path: Some("src/lib.rs".to_string()),
                target_relation: Some("test".to_string()),
                edge_kind: codestory_contracts::api::EdgeKind::CALL,
                confidence: Some(0.95),
                certainty: Some("certain".to_string()),
            }],
            text_consumer_hints: vec![DrillAnchorTextConsumerHintOutput {
                name: "WorkspaceIndexer text hint".to_string(),
                kind: NodeKind::FUNCTION,
                file_path: Some("src/lib.rs".to_string()),
                line: Some(42),
                score: 12.0,
            }],
            notes: Vec::new(),
        });
        let to = sample_drill_anchor("SearchService", "b");
        let mut bridge = fallback_drill_bridge(
            Path::new("C:/repo"),
            &from,
            &to,
            from.chosen_anchor.clone().expect("from"),
            to.chosen_anchor.clone().expect("to"),
            &sample_bridge_trail(false),
            Vec::new(),
            false,
        );
        bridge.evidence_files = vec!["src/lib.rs".to_string()];
        let bridge = DrillBridgeOutput {
            evidence: bridge,
            command: DrillCommandStatusOutput {
                command: "bridge".to_string(),
                status: "ok".to_string(),
                duration_ms: 2,
                artifact: Some("bridge.md".to_string()),
                error: None,
            },
        };
        let targets = vec![VerificationTargetOutput {
            role: "definition".to_string(),
            path: "src/lib.rs".to_string(),
            line: 7,
            node_ref: None,
            reason: "primary source occurrence selected for this symbol".to_string(),
        }];

        let checks = source_truth_checks_from_drill_evidence(&targets, &[from, to], &[bridge]);

        assert_eq!(
            checks.len(),
            1,
            "checks should collapse by file: {checks:#?}"
        );
        let check = &checks[0];
        assert_eq!(check.path, "src/lib.rs");
        assert!(check.required);
        assert!(check.reason.contains("selected anchor"), "{check:#?}");
        assert!(check.reason.contains("consumer"), "{check:#?}");
        assert!(check.reason.contains("related target"), "{check:#?}");
        assert!(check.reason.contains("text hint"), "{check:#?}");
        assert!(check.reason.contains("bridge endpoint"), "{check:#?}");
        assert!(check.reason.contains("bridge hint"), "{check:#?}");
    }

    #[test]
    fn fnv1a_hash_is_stable() {
        assert_eq!(fnv1a_hex(b"abc"), "e71fa2190541574b");
    }

    #[test]
    fn auto_refresh_uses_full_for_empty_index() {
        assert_eq!(
            resolve_refresh_request(RefreshMode::Auto, &summary_with_files(0)),
            Some(IndexMode::Full)
        );
    }

    #[test]
    fn auto_refresh_uses_incremental_for_existing_index() {
        assert_eq!(
            resolve_refresh_request(RefreshMode::Auto, &summary_with_files(3)),
            Some(IndexMode::Incremental)
        );
    }

    #[test]
    fn render_index_markdown_includes_rich_timing_breakdown_when_available() {
        let summary = summary_with_files(3);
        let timings = sample_phase_timings();
        let retrieval = sample_retrieval();
        let output = IndexOutput {
            project: &summary.root,
            storage_path: "C:/repo/.cache/index.sqlite",
            refresh: "full",
            summary: &summary,
            retrieval: &retrieval,
            phase_timings: Some(&timings),
            summary_generation: None,
            readiness: Vec::new(),
            next_commands: Vec::new(),
        };

        let markdown = render_index_markdown(&output);

        assert!(
            markdown.contains("cache_ms: search_projection=61 search_index=62 runtime_publish=63")
        );
        assert!(
            markdown
                .contains("semantic_ms: doc_build=7 embedding=8 db_upsert=9 reload=10 prune=64")
        );
        assert!(markdown.contains("semantic_docs: reused=11 embedded=12 pending=13 stale=14"));
        assert!(markdown.contains(
            "staged_publish_ms: deferred_indexes=7 summary_snapshot=8 detail_snapshot=9 publish=10"
        ));
        assert!(markdown.contains("setup_ms: existing_projection_ids=11 seed_symbol_table=12"));
        assert!(
            markdown.contains(
                "flush_breakdown_ms: files=13 nodes=14 edges=15 occurrences=16 component_access=17 callable_projection=18"
            )
        );
        assert!(markdown.contains(
            "resolution_ms: override_count=25 unresolved_counts=26 calls=27 imports=28 cleanup=29"
        ));
        assert!(markdown.contains(
            "resolution_indexes_ms: call_candidate=30 import_candidate=31 call_semantic=32 import_semantic=33"
        ));
        assert!(markdown.contains(
            "resolution_detail_ms: call_semantic_candidates=34 import_semantic_candidates=35 call_compute=42 import_compute=43 call_apply=44 import_apply=45 overrides=46"
        ));
        assert!(markdown.contains(
            "resolution_semantic_requests: call_rows=36 call_unique=37 call_skipped=38 import_rows=39 import_unique=40 import_skipped=41"
        ));
    }

    #[test]
    fn build_search_output_preserves_separate_provenance_groups() {
        let root = Path::new("C:/repo");
        let symbol_hits = vec![SearchHit {
            node_id: NodeId("1".to_string()),
            display_name: "indexed_symbol".to_string(),
            kind: codestory_contracts::api::NodeKind::FUNCTION,
            file_path: Some("src/lib.rs".to_string()),
            line: Some(10),
            score: 0.9,
            origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
            match_quality: None,
            resolvable: true,
            score_breakdown: None,
            ..test_search_hit_defaults()
        }];
        let repo_text_hits = vec![SearchHit {
            node_id: NodeId("repo-text".to_string()),
            display_name: "README.md".to_string(),
            kind: codestory_contracts::api::NodeKind::FILE,
            file_path: Some("README.md".to_string()),
            line: Some(3),
            score: 500.0,
            origin: codestory_contracts::api::SearchHitOrigin::TextMatch,
            match_quality: Some(codestory_contracts::api::SearchMatchQualityDto::RepoText),
            resolvable: false,
            score_breakdown: None,
            ..test_search_hit_defaults()
        }];

        let output = build_search_output(SearchOutputParts {
            project_root: root,
            query: "needle",
            retrieval: &sample_retrieval(),
            retrieval_shadow: None,
            freshness: None,
            symbol_hits: &symbol_hits,
            repo_text_hits: &repo_text_hits,
            repo_text_stats: None,
            query_assessment: None,
            search_plan: None,
            suggestions: &[],
            occurrences_by_node: &HashMap::new(),
            limit_per_source: 5,
            repo_text: RepoTextOutputConfig {
                mode: RepoTextMode::Auto,
                enabled: true,
            },
            explain: false,
        });

        assert_eq!(output.repo_text_mode, RepoTextMode::Auto);
        assert!(output.repo_text_enabled);
        assert_eq!(output.indexed_symbol_hits.len(), 1);
        assert_eq!(output.repo_text_hits.len(), 1);
        assert_eq!(output.indexed_symbol_hits[0].display_name, "indexed_symbol");
        assert_eq!(output.repo_text_hits[0].display_name, "README.md");
        assert_eq!(
            output.repo_text_hits[0].origin,
            codestory_contracts::api::SearchHitOrigin::TextMatch
        );
    }

    #[test]
    fn build_search_output_marks_repo_text_why_as_diagnostic_navigation() {
        let root = Path::new("C:/repo");
        let repo_text_hits = vec![SearchHit {
            node_id: NodeId("repo-text".to_string()),
            display_name: "README.md".to_string(),
            kind: codestory_contracts::api::NodeKind::FILE,
            file_path: Some("README.md".to_string()),
            line: Some(3),
            score: 500.0,
            origin: codestory_contracts::api::SearchHitOrigin::TextMatch,
            match_quality: Some(codestory_contracts::api::SearchMatchQualityDto::RepoText),
            resolvable: false,
            score_breakdown: None,
            ..test_search_hit_defaults()
        }];

        let output = build_search_output(SearchOutputParts {
            project_root: root,
            query: "needle",
            retrieval: &sample_retrieval(),
            retrieval_shadow: None,
            freshness: None,
            symbol_hits: &[],
            repo_text_hits: &repo_text_hits,
            repo_text_stats: None,
            query_assessment: None,
            search_plan: None,
            suggestions: &[],
            occurrences_by_node: &HashMap::new(),
            limit_per_source: 5,
            repo_text: RepoTextOutputConfig {
                mode: RepoTextMode::Auto,
                enabled: true,
            },
            explain: true,
        });

        let why = output.repo_text_hits[0].why.join("\n");
        assert!(
            why.contains("repo-text diagnostic match"),
            "repo-text why should be a diagnostic/navigation hint: {why}"
        );
        assert!(
            !why.contains("this hit is evidence"),
            "repo-text why must not present text as evidence: {why}"
        );
    }

    #[test]
    fn write_context_bundle_caps_disk_artifacts_and_writes_manifest() {
        let temp = tempdir().expect("bundle dir");
        fs::write(
            temp.path().join("big-mermaid.mmd"),
            "stale oversized artifact",
        )
        .expect("write stale artifact");
        fs::write(
            temp.path().join("previously-omitted.mmd"),
            "stale upstream-omitted artifact",
        )
        .expect("write stale upstream-omitted artifact");
        let answer = sample_agent_answer_with_graph(GraphArtifactDto::Mermaid {
            id: "big-mermaid".to_string(),
            title: "Big Mermaid".to_string(),
            diagram: "graph TD".to_string(),
            mermaid_syntax: format!(
                "graph TD\nA[{}]\n",
                "x".repeat(CONTEXT_BUNDLE_OUTPUT_BYTE_CAP + 1024)
            ),
        });
        let output = serde_json::json!({
            "target": {"selector": "id", "requested": "big-mermaid"},
            "context": crate::output::context_packet_json(&answer),
        });

        write_context_bundle(temp.path(), &output, &answer.graphs, "short context")
            .expect("write capped bundle");

        let manifest: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(temp.path().join("bundle_manifest.json"))
                .expect("read bundle manifest"),
        )
        .expect("parse bundle manifest");
        let context_json: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(temp.path().join("context.json")).expect("read context json"),
        )
        .expect("parse context json");

        assert_eq!(manifest["truncated"], serde_json::Value::Bool(true));
        assert_eq!(
            manifest["omitted_mermaid_artifacts"].as_u64(),
            Some(1),
            "{manifest}"
        );
        assert!(
            manifest["written_bytes_excluding_manifest"]
                .as_u64()
                .is_some_and(|bytes| bytes <= CONTEXT_BUNDLE_OUTPUT_BYTE_CAP as u64),
            "{manifest}"
        );
        assert_eq!(context_json["truncated"], serde_json::Value::Bool(true));
        assert!(
            !temp.path().join("big-mermaid.mmd").exists(),
            "oversized Mermaid artifact should be omitted"
        );
        assert!(
            !temp.path().join("previously-omitted.mmd").exists(),
            "stale Mermaid artifacts from prior runs should be removed"
        );
    }

    #[test]
    fn http_search_repo_text_param_accepts_cli_modes() {
        assert_eq!(
            search_repo_text_mode_param("auto"),
            Some(SearchRepoTextMode::Auto)
        );
        assert_eq!(
            search_repo_text_mode_param("off"),
            Some(SearchRepoTextMode::Off)
        );
        assert_eq!(
            search_repo_text_mode_param("0"),
            Some(SearchRepoTextMode::Off)
        );
        assert_eq!(
            search_repo_text_mode_param("on"),
            Some(SearchRepoTextMode::On)
        );
        assert_eq!(search_repo_text_mode_param("bogus"), None);
    }

    #[test]
    fn build_search_output_adds_stable_node_ref_when_location_is_known() {
        let root = Path::new("C:/repo");
        let symbol_hits = vec![SearchHit {
            node_id: NodeId("1".to_string()),
            display_name: "ResolutionPass".to_string(),
            kind: codestory_contracts::api::NodeKind::STRUCT,
            file_path: Some("C:/repo/src/resolution/mod.rs".to_string()),
            line: Some(42),
            score: 0.9,
            origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
            match_quality: None,
            resolvable: true,
            score_breakdown: None,
            ..test_search_hit_defaults()
        }];

        let output = build_search_output(SearchOutputParts {
            project_root: root,
            query: "ResolutionPass",
            retrieval: &sample_retrieval(),
            retrieval_shadow: None,
            freshness: None,
            symbol_hits: &symbol_hits,
            repo_text_hits: &[],
            repo_text_stats: None,
            query_assessment: None,
            search_plan: None,
            suggestions: &[],
            occurrences_by_node: &HashMap::new(),
            limit_per_source: 5,
            repo_text: RepoTextOutputConfig {
                mode: RepoTextMode::Auto,
                enabled: false,
            },
            explain: false,
        });

        assert_eq!(
            output.indexed_symbol_hits[0].node_ref.as_deref(),
            Some("src/resolution/mod.rs:42:ResolutionPass")
        );
    }

    #[test]
    fn build_search_output_adds_occurrence_quality_and_verification_targets() {
        let root = Path::new("C:/repo");
        let symbol_hits = vec![SearchHit {
            node_id: NodeId("1".to_string()),
            display_name: "StorageAccess".to_string(),
            kind: codestory_contracts::api::NodeKind::CLASS,
            file_path: Some("C:/repo/src/lib/StorageAccess.h".to_string()),
            line: Some(12),
            score: 0.9,
            origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
            match_quality: None,
            resolvable: true,
            score_breakdown: None,
            ..test_search_hit_defaults()
        }];
        let mut occurrences = HashMap::new();
        occurrences.insert(
            NodeId("1".to_string()),
            vec![
                SourceOccurrenceDto {
                    element_id: "1".to_string(),
                    kind: "declaration".to_string(),
                    file_path: "C:/repo/src/lib/StorageAccess.h".to_string(),
                    start_line: 12,
                    start_col: 1,
                    end_line: 12,
                    end_col: 20,
                },
                SourceOccurrenceDto {
                    element_id: "1".to_string(),
                    kind: "definition".to_string(),
                    file_path: "C:/repo/src/lib/StorageAccess.cpp".to_string(),
                    start_line: 44,
                    start_col: 1,
                    end_line: 60,
                    end_col: 1,
                },
            ],
        );

        let output = build_search_output(SearchOutputParts {
            project_root: root,
            query: "StorageAccess",
            retrieval: &sample_retrieval(),
            retrieval_shadow: None,
            freshness: None,
            symbol_hits: &symbol_hits,
            repo_text_hits: &[],
            repo_text_stats: None,
            query_assessment: None,
            search_plan: None,
            suggestions: &[],
            occurrences_by_node: &occurrences,
            limit_per_source: 5,
            repo_text: RepoTextOutputConfig {
                mode: RepoTextMode::Auto,
                enabled: false,
            },
            explain: false,
        });

        let hit = &output.indexed_symbol_hits[0];
        assert_eq!(hit.primary_occurrence_kind.as_deref(), Some("definition"));
        assert_eq!(hit.symbol_role.as_deref(), Some("definition"));
        assert!(hit.verification_targets.iter().any(|target| target.path
            == "src/lib/StorageAccess.cpp"
            && target.role == "definition"));
        assert!(
            hit.paired_refs.iter().any(|target| {
                target.path == "src/lib/StorageAccess.h" && target.role == "declaration"
            }),
            "definition hits should point back to the paired declaration"
        );
    }

    #[test]
    fn header_member_hits_include_sibling_implementation_verification_target() {
        let temp = tempdir().expect("tempdir");
        let src_dir = temp.path().join("src/lib/project");
        fs::create_dir_all(&src_dir).expect("mkdir");
        fs::write(
            src_dir.join("Project.h"),
            "class Project { void buildIndex(); };\n",
        )
        .expect("write header");
        fs::write(
            src_dir.join("Project.cpp"),
            "#include \"Project.h\"\n\nvoid Project::buildIndex() {}\n",
        )
        .expect("write impl");

        let targets = implementation_counterpart_targets_for_hit(
            temp.path(),
            "Project::buildIndex",
            Some("src/lib/project/Project.h"),
        );

        assert!(
            targets.iter().any(|target| {
                target.path == "src/lib/project/Project.cpp"
                    && target.line == 3
                    && target.role == "definition"
            }),
            "expected sibling implementation target in {targets:#?}"
        );
    }

    #[test]
    fn abstract_interface_hits_include_concrete_implementation_verification_targets() {
        let temp = tempdir().expect("tempdir");
        let src_dir = temp.path().join("src/lib/data/storage");
        fs::create_dir_all(&src_dir).expect("mkdir");
        fs::write(
            src_dir.join("StorageAccess.h"),
            "class StorageAccess {\npublic:\n virtual TextAccess getFileContent() const = 0;\n};\n",
        )
        .expect("write interface");
        fs::write(
            src_dir.join("PersistentStorage.h"),
            "class PersistentStorage\n    : public StorageAccess\n{\n};\n",
        )
        .expect("write implementation header");
        fs::write(
            src_dir.join("PersistentStorage.cpp"),
            "#include \"PersistentStorage.h\"\n\nTextAccess PersistentStorage::getFileContent() const {}\n",
        )
        .expect("write implementation");
        fs::write(
            src_dir.join("StorageCache.cpp"),
            "TextAccess StorageCache::getFileContent() const {}\n",
        )
        .expect("write unrelated implementation");

        let targets = interface_implementation_targets_for_hit(
            temp.path(),
            "StorageAccess::getFileContent",
            Some("src/lib/data/storage/StorageAccess.h"),
        );

        assert!(
            targets.iter().any(|target| {
                target.path == "src/lib/data/storage/PersistentStorage.h"
                    && target.line == 1
                    && target.role == "declaration"
            }),
            "expected concrete implementation class declaration in {targets:#?}"
        );
        assert!(
            targets.iter().any(|target| {
                target.path == "src/lib/data/storage/PersistentStorage.cpp"
                    && target.line == 3
                    && target.role == "definition"
            }),
            "expected concrete implementation method definition in {targets:#?}"
        );
        assert!(
            !targets
                .iter()
                .any(|target| target.path == "src/lib/data/storage/StorageCache.cpp"),
            "unrelated same-name methods should not be verification targets: {targets:#?}"
        );
    }

    #[test]
    fn build_search_output_marks_repo_text_duplicates_of_indexed_symbols() {
        let root = Path::new("C:/repo");
        let symbol_hits = vec![SearchHit {
            node_id: NodeId("symbol-1".to_string()),
            display_name: "build_snapshot_digest".to_string(),
            kind: codestory_contracts::api::NodeKind::FUNCTION,
            file_path: Some("C:/repo/src/lib.rs".to_string()),
            line: Some(7),
            score: 0.9,
            origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
            match_quality: None,
            resolvable: true,
            score_breakdown: None,
            ..test_search_hit_defaults()
        }];
        let repo_text_hits = vec![SearchHit {
            node_id: NodeId("text-1".to_string()),
            display_name: "src/lib.rs".to_string(),
            kind: codestory_contracts::api::NodeKind::FILE,
            file_path: Some("C:/repo/src/lib.rs".to_string()),
            line: Some(7),
            score: 500.0,
            origin: codestory_contracts::api::SearchHitOrigin::TextMatch,
            match_quality: Some(codestory_contracts::api::SearchMatchQualityDto::RepoText),
            resolvable: false,
            score_breakdown: None,
            ..test_search_hit_defaults()
        }];

        let output = build_search_output(SearchOutputParts {
            project_root: root,
            query: "snapshot digest",
            retrieval: &sample_retrieval(),
            retrieval_shadow: None,
            freshness: None,
            symbol_hits: &symbol_hits,
            repo_text_hits: &repo_text_hits,
            repo_text_stats: None,
            query_assessment: None,
            search_plan: None,
            suggestions: &[],
            occurrences_by_node: &HashMap::new(),
            limit_per_source: 5,
            repo_text: RepoTextOutputConfig {
                mode: RepoTextMode::Auto,
                enabled: true,
            },
            explain: false,
        });

        assert_eq!(
            output.repo_text_hits[0].duplicate_of.as_deref(),
            Some("symbol-1")
        );
    }

    #[test]
    fn task_brief_output_contract_maps_packet_evidence_to_owner_workflow() {
        let packet = sample_task_brief_packet();
        let brief = build_task_brief_output(Path::new("C:/repo"), &packet);

        assert_eq!(brief.task_brief_version, 1);
        assert_eq!(brief.status, "needs_attention");
        assert_eq!(brief.source_packet_id, "packet-task-brief");
        assert_eq!(brief.source_packet_sufficiency, "partial");
        assert_eq!(
            brief.first_files[0].path,
            "crates/codestory-cli/src/`main_$env:SECRET$('x').rs"
        );
        assert_eq!(
            brief.relevant_symbols[0].name,
            "run_`packet_$env:SECRET$('x')"
        );
        assert_eq!(
            brief.likely_tests[0].path,
            "crates/codestory-cli/tests/stdio`$env:SECRET$('x')_protocol_contracts.rs"
        );
        assert!(
            brief
                .impacted_surfaces
                .contains(&"crates/codestory-cli".to_string())
        );
        assert!(
            brief
                .risks_unknowns
                .contains(&"verify `changed` files after editing".to_string())
        );
        for expected in [
            "codestory-cli packet",
            "codestory-cli snippet",
            "codestory-cli trail",
            "codestory-cli affected",
        ] {
            assert!(
                brief
                    .follow_up_codestory_commands
                    .iter()
                    .any(|command| command.contains(expected)),
                "brief should include {expected}: {brief:#?}"
            );
        }
        assert_eq!(brief.future_sections, ["scout", "where", "onboard"]);

        let packet_command = brief
            .follow_up_codestory_commands
            .iter()
            .find(|command| command.contains("codestory-cli packet"))
            .expect("packet follow-up command");
        assert!(
            packet_command.contains(&format!(
                "--question {}",
                quote_command_value(&packet.question)
            )),
            "packet follow-up should quote prompt safely: {packet_command}"
        );
        let snippet_command = brief
            .follow_up_codestory_commands
            .iter()
            .find(|command| command.contains("codestory-cli snippet"))
            .expect("snippet follow-up command");
        assert!(
            snippet_command.contains(&quote_command_value(&brief.first_files[0].path)),
            "snippet follow-up should quote path safely: {snippet_command}"
        );
        let trail_command = brief
            .follow_up_codestory_commands
            .iter()
            .find(|command| command.contains("codestory-cli trail"))
            .expect("trail follow-up command");
        assert!(
            trail_command.contains(&quote_command_value(&brief.relevant_symbols[0].name)),
            "trail follow-up should quote symbol safely: {trail_command}"
        );

        let json = serde_json::to_value(&brief).expect("brief should serialize");
        for key in [
            "task_brief_version",
            "prompt",
            "status",
            "first_files",
            "relevant_symbols",
            "likely_tests",
            "impacted_surfaces",
            "risks_unknowns",
            "follow_up_codestory_commands",
            "future_sections",
        ] {
            assert!(json.get(key).is_some(), "brief JSON should include {key}");
        }

        let markdown = render_task_brief_markdown(&brief);
        assert!(
            markdown.contains("prompt: `Add '$env:SECRET $(Get-ChildItem) 'literal' task brief`"),
            "brief markdown should replace prompt backticks inside inline code: {markdown}"
        );
        assert!(
            markdown.contains("`crates/codestory-cli/src/'main_$env:SECRET$('x').rs`"),
            "brief markdown should replace path backticks inside inline code: {markdown}"
        );
        assert!(
            markdown.contains("`run_'packet_$env:SECRET$('x')`"),
            "brief markdown should replace symbol backticks inside inline code: {markdown}"
        );
        assert!(
            markdown.contains("- verify 'changed' files after editing"),
            "brief markdown should replace risk backticks in bullets: {markdown}"
        );
        assert!(
            markdown.contains("- command:\n    codestory-cli packet"),
            "brief markdown should render commands as indented code blocks: {markdown}"
        );
        assert!(
            !markdown.contains("- `codestory-cli"),
            "brief markdown should not render follow-up commands as inline code: {markdown}"
        );
        assert!(
            !markdown.contains("```"),
            "brief markdown should not use fences that embedded backticks can split: {markdown}"
        );
        for heading in [
            "# Task Brief",
            "## First Files",
            "## Relevant Symbols",
            "## Likely Tests",
            "## Impacted Surfaces",
            "## Risks And Unknowns",
            "## Follow Up CodeStory Commands",
            "## Future Sections",
        ] {
            assert!(
                markdown.contains(heading),
                "brief markdown should include {heading}: {markdown}"
            );
        }
    }

    #[test]
    fn all_existing_commands_accept_output_file() {
        let commands = [
            vec!["codestory-cli", "index", "--output-file", "out.md"],
            vec!["codestory-cli", "ground", "--output-file", "out.md"],
            vec![
                "codestory-cli",
                "search",
                "--query",
                "needle",
                "--output-file",
                "out.md",
            ],
            vec![
                "codestory-cli",
                "symbol",
                "--query",
                "Foo",
                "--output-file",
                "out.md",
            ],
            vec![
                "codestory-cli",
                "trail",
                "--query",
                "Foo",
                "--hide-speculative",
                "--format",
                "dot",
                "--output-file",
                "out.md",
            ],
            vec![
                "codestory-cli",
                "snippet",
                "--query",
                "Foo",
                "--output-file",
                "out.md",
            ],
            vec![
                "codestory-cli",
                "task",
                "brief",
                "--prompt",
                "Implement issue 507",
                "--output-file",
                "out.md",
            ],
            vec![
                "codestory-cli",
                "query",
                "search(query: 'Foo') | limit(1)",
                "--output-file",
                "out.md",
            ],
            vec!["codestory-cli", "doctor", "--output-file", "out.md"],
            vec![
                "codestory-cli",
                "explore",
                "--query",
                "Foo",
                "--no-tui",
                "--output-file",
                "out.md",
            ],
            vec![
                "codestory-cli",
                "bookmark",
                "add",
                "--id",
                "1",
                "--output-file",
                "out.md",
            ],
            vec![
                "codestory-cli",
                "bookmark",
                "list",
                "--output-file",
                "out.md",
            ],
            vec![
                "codestory-cli",
                "bookmark",
                "remove",
                "1",
                "--output-file",
                "out.md",
            ],
        ];

        for command in commands {
            Cli::try_parse_from(command).expect("command should parse --output-file");
        }
    }

    #[test]
    fn explore_tui_keyboard_state_reaches_every_pane() {
        let mut state = ExploreTuiState::new(6);
        for expected in 1..6 {
            assert!(!state.apply(ExploreTuiAction::NextPane));
            assert_eq!(state.selected, expected);
        }
        assert!(!state.apply(ExploreTuiAction::NextPane));
        assert_eq!(state.selected, 0);

        assert!(!state.apply(ExploreTuiAction::PreviousPane));
        assert_eq!(state.selected, 5);
        assert!(!state.apply(ExploreTuiAction::ScrollDown(12)));
        assert_eq!(state.scroll[5], 12);
        assert!(!state.apply(ExploreTuiAction::ScrollUp(5)));
        assert_eq!(state.scroll[5], 7);
        assert!(!state.apply(ExploreTuiAction::Home));
        assert_eq!(state.scroll[5], 0);
        assert!(state.apply(ExploreTuiAction::Quit));
    }

    #[test]
    fn explore_tui_key_mapping_covers_keyboard_only_controls() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        assert_eq!(
            explore_tui_action(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
            ExploreTuiAction::NextPane
        );
        assert_eq!(
            explore_tui_action(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT)),
            ExploreTuiAction::PreviousPane
        );
        assert_eq!(
            explore_tui_action(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE)),
            ExploreTuiAction::ScrollDown(1)
        );
        assert_eq!(
            explore_tui_action(KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE)),
            ExploreTuiAction::ScrollUp(10)
        );
        assert_eq!(
            explore_tui_action(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE)),
            ExploreTuiAction::Quit
        );
        assert_eq!(
            explore_tui_action(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            ExploreTuiAction::Quit
        );
    }

    #[test]
    fn build_search_output_includes_why_when_requested() {
        let root = Path::new("C:/repo");
        let symbol_hits = vec![SearchHit {
            node_id: NodeId("1".to_string()),
            display_name: "ranked_symbol".to_string(),
            kind: codestory_contracts::api::NodeKind::FUNCTION,
            file_path: Some("src/lib.rs".to_string()),
            line: Some(10),
            score: 0.9,
            origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
            match_quality: None,
            resolvable: true,
            score_breakdown: Some(codestory_contracts::api::RetrievalScoreBreakdownDto {
                lexical: 0.7,
                semantic: 0.2,
                graph: 0.1,
                total: 0.9,
                tier_cap: None,
                boosts: Vec::new(),
                dampening: Vec::new(),
                final_rank_reason: None,
                provenance: Vec::new(),
            }),
            ..test_search_hit_defaults()
        }];

        let output = build_search_output(SearchOutputParts {
            project_root: root,
            query: "ranked",
            retrieval: &sample_retrieval(),
            retrieval_shadow: None,
            freshness: None,
            symbol_hits: &symbol_hits,
            repo_text_hits: &[],
            repo_text_stats: None,
            query_assessment: None,
            search_plan: None,
            suggestions: &[],
            occurrences_by_node: &HashMap::new(),
            limit_per_source: 5,
            repo_text: RepoTextOutputConfig {
                mode: RepoTextMode::Off,
                enabled: false,
            },
            explain: true,
        });

        assert!(output.explain);
        assert_eq!(
            output.indexed_symbol_hits[0]
                .score_breakdown
                .as_ref()
                .map(|score| score.total),
            Some(0.9)
        );
        assert!(
            output.indexed_symbol_hits[0]
                .why
                .iter()
                .any(|why| why.contains("lexical=0.700"))
        );
    }

    #[test]
    fn drill_anchor_helpers_normalize_and_sanitize_inputs() {
        assert_eq!(
            drill_targeting::normalized_drill_anchors(&[
                "WorkspaceIndexer, SearchService".to_string(),
                "WorkspaceIndexer".to_string(),
                " TrailResult ".to_string(),
            ]),
            vec!["WorkspaceIndexer", "SearchService", "TrailResult"]
        );
        assert_eq!(
            output_slug("getElsewhereFeed() / posts"),
            "getElsewhereFeed-posts"
        );
    }

    #[test]
    fn drill_next_commands_push_body_aware_snippets_after_search() {
        let anchors = vec![sample_drill_anchor(
            "WorkspaceIndexer",
            "workspace-indexer-id",
        )];
        let commands = drill_next_commands(Path::new("C:/repo"), &anchors, &[], false);

        assert!(
            commands
                .iter()
                .any(|command| command.contains("codestory-cli search")
                    && command.contains("--query")
                    && command.contains("WorkspaceIndexer")),
            "drill should preserve the exact-anchor search handoff: {commands:#?}"
        );
        assert!(
            commands
                .iter()
                .any(|command| command.contains("codestory-cli snippet")
                    && command.contains("--id")
                    && command.contains("workspace-indexer-id")
                    && command.contains("--function-body")
                    && command.contains("--context 40")),
            "drill should advertise body-aware snippets for decisive operations: {commands:#?}"
        );
        assert!(
            !commands
                .iter()
                .any(|command| command.contains("codestory-cli snippet")
                    && command.contains("--query")
                    && command.contains("WorkspaceIndexer")),
            "drill should use the selected anchor id for unambiguous snippet follow-ups: {commands:#?}"
        );
    }

    #[test]
    fn command_quoting_single_quotes_shell_sensitive_values() {
        #[cfg(windows)]
        assert_eq!(
            quote_command_value("Inspect $env:SECRET and $(Get-ChildItem) and 'literal'"),
            "'Inspect $env:SECRET and $(Get-ChildItem) and ''literal'''"
        );
        #[cfg(not(windows))]
        assert_eq!(
            quote_command_value("Inspect $env:SECRET and $(Get-ChildItem) and 'literal'"),
            r"'Inspect $env:SECRET and $(Get-ChildItem) and '\''literal'\'''"
        );
        assert_eq!(
            quote_command_path(Path::new("C:/repo/$hidden")),
            "'C:/repo/$hidden'"
        );
        assert_eq!(
            quote_command_path(Path::new("C:/repo/quoted\"path")),
            "'C:/repo/quoted\"path'"
        );
        assert_eq!(quote_command_path(Path::new("C:/repo")), "\"C:/repo\"");
    }

    #[test]
    fn drill_summary_compacts_statuses_for_report_synthesis() {
        let resolved_anchor = sample_drill_anchor("WorkspaceIndexer", "a");
        let unresolved_anchor = DrillAnchorOutput {
            anchor: "MissingAnchor".to_string(),
            typed_hit_count: 0,
            chosen_anchor: None,
            verification_targets: Vec::new(),
            consumer_summary: None,
            timings: DrillAnchorTimingsOutput {
                total_ms: 17,
                search_ms: 17,
                resolution_ms: 0,
                consumer_summary_ms: 0,
                command_artifacts_ms: 17,
            },
            commands: vec![DrillCommandStatusOutput {
                command: "search".to_string(),
                status: "error".to_string(),
                duration_ms: 17,
                artifact: None,
                error: Some("not found".to_string()),
            }],
        };
        let bridge = DrillBridgeOutput {
            evidence: unresolved_drill_bridge(
                &resolved_anchor,
                &unresolved_anchor,
                "to anchor was not resolved",
            ),
            command: DrillCommandStatusOutput {
                command: "bridge".to_string(),
                status: "ok".to_string(),
                duration_ms: 2,
                artifact: Some("bridge-1.md".to_string()),
                error: None,
            },
        };
        let output = DrillOutput {
            project: "C:/repo".to_string(),
            label: Some("sample".to_string()),
            question: Some("How does indexing work?".to_string()),
            output_dir: "target/drill/sample".to_string(),
            mechanical: DrillMechanicalOutput {
                before_files: 1,
                before_nodes: 10,
                before_edges: 2,
                before_errors: 1,
                after_files: 2,
                after_nodes: 20,
                after_edges: 4,
                after_errors: 0,
                refresh: "full".to_string(),
                retrieval: Some(sample_retrieval()),
                sidecar_retrieval_mode: Some("full".to_string()),
                freshness: None,
                phase_timings: Some(sample_phase_timings()),
                drill_timings: sample_drill_runtime_timings(),
            },
            question_search: None,
            question_supplemental_searches: Vec::new(),
            anchors: vec![resolved_anchor, unresolved_anchor],
            bridges: vec![bridge],
            execution_boundaries: drill_execution_boundaries(),
            verification_targets: Vec::new(),
            evidence_packet: EvidencePacketDto {
                packet_version: 1,
                question: Some("How does indexing work?".to_string()),
                items: Vec::new(),
                readiness: AnswerReadinessReportDto {
                    overall_status: ClaimReadinessDto::NeedsSourceRead,
                    safe_to_say: vec!["anchor evidence is available".to_string()],
                    inferred_claims: vec!["bridge is unresolved".to_string()],
                    needs_verification: vec!["read the source".to_string()],
                    next_commands: vec![
                        "codestory-cli snippet --query WorkspaceIndexer".to_string(),
                    ],
                    source_truth_checks: vec![SourceTruthCheckDto {
                        id: "source-truth-1".to_string(),
                        reason: "primary source occurrence".to_string(),
                        path: "src/indexer.rs".to_string(),
                        line: Some(12),
                        required: true,
                    }],
                },
            },
            answer_quality_contract: drill_answer_quality_contract(),
            claim_ledger_template: DrillClaimLedgerOutput {
                template_version: 1,
                instructions: Vec::new(),
                claims: vec![DrillClaimLedgerEntryOutput {
                    id: "claim-1".to_string(),
                    claim: "anchor claim".to_string(),
                    expected_evidence: Vec::new(),
                    source_truth_files: vec!["src/indexer.rs".to_string()],
                    pre_verification_confidence: "medium".to_string(),
                    classification: None,
                    changed_after_source_read: None,
                    correction_note: None,
                }],
                scoring: DrillClaimLedgerScoringOutput {
                    status: "pending_source_verification".to_string(),
                    pending_claim_count: 1,
                    correct: 0,
                    partial: 0,
                    misleading: 0,
                    unsupported: 0,
                    material_revision_count: 0,
                    score_formula: "manual".to_string(),
                },
            },
            verification_checklist: drill_verification_checklist(),
            next_commands: Vec::new(),
        };

        let markdown = render_drill_markdown(&output);
        assert!(
            markdown.contains(
                "drill_timings_ms: total=42 setup=3 question_search=5 anchors=13 supplemental_search=7 bridges=11 evidence_assembly=3"
            ),
            "drill report markdown should expose diagnostic runtime timings: {markdown}"
        );
        assert!(
            markdown.contains("search [error duration_ms=17"),
            "drill report markdown should expose per-command timings: {markdown}"
        );

        let summary = drill_summary(&output);

        assert_eq!(summary.mechanical.error_delta, -1);
        assert!(summary.mechanical.index_ready);
        assert_eq!(summary.anchors.requested, 2);
        assert_eq!(summary.anchors.resolved, 1);
        assert_eq!(summary.anchors.unresolved, 1);
        assert_eq!(summary.anchors.failed_command_count, 1);
        let missing_anchor = summary
            .anchors
            .statuses
            .iter()
            .find(|anchor| anchor.anchor == "MissingAnchor")
            .expect("missing anchor status");
        assert_eq!(missing_anchor.command_duration_ms, 17);
        assert_eq!(missing_anchor.slowest_command.as_deref(), Some("search"));
        assert_eq!(missing_anchor.slowest_command_ms, 17);
        assert_eq!(summary.bridges.total, 1);
        assert_eq!(summary.bridges.unresolved_or_error, 1);
        assert_eq!(summary.mechanical.drill_timings.total_ms, 42);
        assert_eq!(summary.mechanical.drill_timings.bridge_evidence_ms, 11);
        assert!(summary.source_truth.required);
        assert_eq!(summary.source_truth.target_files, vec!["src/indexer.rs"]);
        assert_eq!(
            summary.open_gaps.overall_status,
            ClaimReadinessDto::NeedsSourceRead
        );
        assert!(summary.open_gaps.open_gap_friendly);
        assert_eq!(summary.open_gaps.status, "open_gaps_explicit");

        let from = sample_drill_anchor("FromAnchor", "from");
        let to = sample_drill_anchor("ToAnchor", "to");
        let mut failed_bridge_output = output.clone();
        failed_bridge_output.anchors = vec![from.clone(), to.clone()];
        failed_bridge_output.bridges = vec![DrillBridgeOutput {
            evidence: drill_bridge_error(
                Path::new("C:/repo"),
                &from,
                &to,
                from.chosen_anchor.clone().expect("from"),
                to.chosen_anchor.clone().expect("to"),
                "internal",
                "failed",
                false,
            ),
            command: DrillCommandStatusOutput {
                command: "bridge".to_string(),
                status: "error".to_string(),
                duration_ms: 3,
                artifact: None,
                error: Some("failed".to_string()),
            },
        }];
        let failed_summary = drill_summary(&failed_bridge_output);
        assert!(
            failed_summary
                .verdict
                .next_action
                .contains("repair or rerun"),
            "failed bridge commands should not be presented as ordinary verification work: {failed_summary:#?}"
        );
    }

    #[test]
    fn drill_suite_cases_load_manifest_order_and_anchors() {
        let temp = tempdir().expect("manifest dir");
        let manifest_path = temp.path().join("agent-drill-cases.json");
        fs::write(
            &manifest_path,
            serde_json::json!({
                "suite": "generic-agent-drill",
                "cases": [
                    {
                        "slug": "alpha repo",
                        "project": "alpha-project",
                        "question": "Explain how alpha works.",
                        "anchors": [" AlphaRoot ", "", "AlphaStore"]
                    },
                    {
                        "slug": "beta",
                        "project": "beta-project",
                        "question": "Explain how beta works.",
                        "anchors": ["BetaRoot"]
                    }
                ]
            })
            .to_string(),
        )
        .expect("write manifest");
        let owner_root = PathBuf::from("C:/owner");
        let (suite, cases) =
            drill_suite_cases_from_manifest(&manifest_path, &owner_root).expect("suite cases");

        assert_eq!(suite, "generic-agent-drill");
        assert_eq!(
            cases
                .iter()
                .map(|case| case.slug.as_str())
                .collect::<Vec<_>>(),
            vec!["alpha-repo", "beta"]
        );
        assert_eq!(cases[0].project_root, temp.path().join("alpha-project"));
        assert_eq!(cases[1].project_root, temp.path().join("beta-project"));
        assert_eq!(
            cases[0].anchors,
            vec!["AlphaRoot".to_string(), "AlphaStore".to_string()]
        );
        assert!(cases[1].question.contains("beta"));
    }

    #[test]
    fn drill_jobs_normalize_to_safe_bounded_workers() {
        assert_eq!(normalize_drill_jobs_with_limit(0, 4), 1);
        assert_eq!(normalize_drill_jobs_with_limit(4, 16), 4);
        assert_eq!(normalize_drill_jobs_with_limit(99, 16), MAX_DRILL_JOBS);
        assert_eq!(normalize_drill_jobs_with_limit(8, 2), 2);
        assert_eq!(drill_read_only_jobs(4, RefreshMode::Full), 1);
        assert_eq!(drill_read_only_jobs(4, RefreshMode::Incremental), 1);
        assert_eq!(drill_read_only_jobs(4, RefreshMode::None), 4);
        assert_eq!(drill_anchor_jobs(4, RefreshMode::None, 5), 4);
        assert_eq!(drill_anchor_jobs(4, RefreshMode::None, 1), 1);
        assert_eq!(drill_anchor_jobs(4, RefreshMode::Full, 5), 1);
        assert_eq!(drill_anchor_jobs(99, RefreshMode::None, 2), 2);
        assert_eq!(drill_suite_case_jobs(4, RefreshMode::Full, 3), 1);
        assert_eq!(drill_suite_case_jobs(4, RefreshMode::None, 3), 3);
        assert_eq!(drill_suite_case_jobs(4, RefreshMode::None, 1), 1);
    }

    #[test]
    fn drill_bridge_neighborhood_file_cache_loads_each_node_once() {
        let cache = DrillBridgeNeighborhoodFileCache::default();
        let load_count = std::sync::atomic::AtomicUsize::new(0);

        let first = cache.files_for_key("node-a", || {
            load_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            HashSet::from(["src/a.rs".to_string()])
        });
        let second = cache.files_for_key("node-a", || {
            load_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            HashSet::from(["src/other.rs".to_string()])
        });
        let third = cache.files_for_key("node-b", || {
            load_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            HashSet::from(["src/b.rs".to_string()])
        });

        assert_eq!(load_count.load(std::sync::atomic::Ordering::SeqCst), 2);
        assert_eq!(first, HashSet::from(["src/a.rs".to_string()]));
        assert_eq!(
            second,
            HashSet::from(["src/a.rs".to_string()]),
            "repeat fallback bridges should reuse the first neighborhood file result"
        );
        assert_eq!(third, HashSet::from(["src/b.rs".to_string()]));
    }

    #[test]
    fn drill_bridge_pairs_preserve_deterministic_anchor_pair_order() {
        let anchors = vec![
            sample_drill_anchor("Alpha", "a"),
            sample_drill_anchor("Beta", "b"),
            sample_drill_anchor("Gamma", "c"),
            sample_drill_anchor("Delta", "d"),
        ];

        assert_eq!(
            drill_bridge_pairs(&anchors),
            vec![(0, 1), (0, 2), (0, 3), (1, 2), (1, 3), (2, 3)]
        );
    }

    #[test]
    fn drill_suite_manifest_expectations_and_ledger_score_answer_quality() {
        let temp = tempdir().expect("manifest dir");
        let manifest_path = temp.path().join("agent-drill-cases.json");
        fs::write(
            &manifest_path,
            serde_json::json!({
                "suite": "answer-quality-suite",
                "cases": [
                    {
                        "slug": "alpha repo",
                        "project": "alpha-project",
                        "question": "Explain the public feed path.",
                        "anchors": ["Posts", "getElsewhereFeed", "getCommentAuth"],
                        "expect": {
                            "source_truth_files": [
                                "src/app/(frontend)/page.tsx",
                                "src/components/RootRuntimeHome.tsx",
                                "src/collections/SocialEntries.ts"
                            ],
                            "false_claims": [
                                "public homepage calls getElsewhereFeed directly"
                            ],
                            "min_anchor_resolution": 3,
                            "allow_partial_bridges": true
                        }
                    }
                ]
            })
            .to_string(),
        )
        .expect("write manifest");
        let owner_root = PathBuf::from("C:/owner");
        let (_suite, cases) =
            drill_suite_cases_from_manifest(&manifest_path, &owner_root).expect("suite cases");
        let expectations = cases[0].expectations.clone();
        assert_eq!(expectations.min_anchor_resolution, Some(3));
        assert_eq!(expectations.allow_partial_bridges, Some(true));
        assert_eq!(expectations.source_truth_files.len(), 3);
        assert_eq!(expectations.false_claims.len(), 1);

        let ledger_path = temp.path().join("ledger.json");
        fs::write(
            &ledger_path,
            serde_json::json!({
                "schema_version": 1,
                "suite": "answer-quality-suite",
                "cases": [
                    {
                        "slug": "alpha repo",
                        "draft_written": true,
                        "claims": [
                            {
                                "id": "claim-1",
                                "text": "Public homepage calls getElsewhereFeed directly.",
                                "classification": "correct",
                                "changed_after_source_read": false,
                                "source_files": ["src/app/(frontend)/page.tsx"]
                            },
                            {
                                "id": "claim-2",
                                "text": "Comments are persisted through Payload.",
                                "classification": "partial",
                                "changed_after_source_read": true,
                                "source_files": ["src/app/(frontend)/posts/[slug]/comments/route.ts"]
                            }
                        ],
                        "layer_findings": [
                            {
                                "layer": "graph_trail",
                                "status": "partial",
                                "detail": "component trail was incomplete"
                            }
                        ]
                    }
                ]
            })
            .to_string(),
        )
        .expect("write ledger");
        let ledger_cases = drill_suite_ledger_cases(Some(&ledger_path)).expect("ledger cases");
        let ledger_case = ledger_cases.get("alpha-repo").expect("alpha ledger");

        let mut summary = sample_drill_summary("alpha", "ready", 3, 0, 0);
        summary.source_truth.target_files = vec![
            "src/app/(frontend)/page.tsx".to_string(),
            "src/components/RootRuntimeHome.tsx".to_string(),
        ];
        let quality = drill_suite_answer_quality(&summary, &expectations, Some(ledger_case), true);

        assert_eq!(quality.ledger_status, "present");
        assert_eq!(quality.final_answer_status, "failed");
        assert_eq!(quality.claim_count, 2);
        assert_eq!(quality.claim_correct_count, 1);
        assert_eq!(quality.claim_partial_count, 1);
        assert_eq!(quality.material_revision_count, 1);
        assert_eq!(quality.expected_file_count, 3);
        assert_eq!(quality.expected_file_found_count, 2);
        assert_eq!(
            quality.missing_expected_files,
            vec!["src/collections/SocialEntries.ts"]
        );
        assert_eq!(quality.forbidden_claim_count, 1);
        assert_eq!(quality.layer_findings.len(), 1);
    }

    #[test]
    fn drill_suite_answer_quality_stays_pending_without_ledger() {
        let mut summary = sample_drill_summary("alpha", "degraded", 3, 1, 2);
        summary.source_truth.target_files = vec!["src/lib.rs".to_string()];
        let expectations = DrillSuiteExpectationOutput {
            source_truth_files: vec!["src/lib.rs".to_string()],
            false_claims: Vec::new(),
            min_anchor_resolution: Some(3),
            allow_partial_bridges: None,
        };

        let quality = drill_suite_answer_quality(&summary, &expectations, None, false);

        assert_eq!(quality.ledger_status, "not_supplied");
        assert_eq!(quality.final_answer_status, "pending_source_verification");
        assert_eq!(quality.expected_file_found_count, 1);
        assert!(
            quality
                .warnings
                .iter()
                .any(|item| item.contains("no source-truth ledger"))
        );
    }

    #[test]
    fn drill_suite_cache_dir_isolated_per_case_when_explicit() {
        assert_eq!(drill_suite_case_cache_dir(None, "alpha"), None);
        assert_eq!(
            drill_suite_case_cache_dir(Some(Path::new("C:/cache/codestory-suite")), "beta/runtime"),
            Some(PathBuf::from("C:/cache/codestory-suite/beta-runtime"))
        );
    }

    #[test]
    fn drill_suite_progress_messages_include_repo_index_and_verdict() {
        let case = DrillSuiteCase {
            slug: "alpha".to_string(),
            project_root: PathBuf::from("C:/repos/alpha"),
            question: "Explain how alpha indexes.".to_string(),
            anchors: vec!["AlphaRoot".to_string(), "AlphaStore".to_string()],
            expectations: empty_drill_suite_expectations(),
        };
        let start = drill_suite_repo_progress_start_message(
            1,
            3,
            &case,
            Path::new("target/drill-suite/alpha-drill"),
        );
        assert!(start.contains("[1/3] start alpha"));
        assert!(start.contains("C:/repos/alpha"));
        assert!(start.contains("target/drill-suite/alpha-drill"));

        let repo = sample_drill_suite_repo("alpha", "degraded", 3, 1, 4);
        let done = drill_suite_repo_progress_done_message(1, 3, "alpha", &repo.summary);
        assert!(done.contains("[1/3] done alpha"));
        assert!(done.contains("verdict=degraded"));
        assert!(done.contains("anchors=3/3"));
        assert!(done.contains("bridges=graph:2 partial:0 unresolved:1"));
    }

    #[test]
    fn drill_suite_markdown_summarizes_verdicts_and_source_truth() {
        let mut output = DrillSuiteOutput {
            suite: "generic-agent-drill".to_string(),
            project: "C:/repos/owner".to_string(),
            case_file: "C:/repos/owner/drill-cases.json".to_string(),
            output_dir: "target/drill-suite".to_string(),
            repo_count: 2,
            degraded_count: 1,
            blocked_count: 1,
            ready_count: 0,
            answer_ready_count: 0,
            answer_degraded_count: 0,
            answer_failed_count: 0,
            answer_pending_count: 2,
            repos: vec![
                sample_drill_suite_repo("alpha", "degraded", 3, 1, 4),
                sample_drill_suite_repo("beta", "blocked", 0, 0, 2),
            ],
            next_actions: vec![
                "alpha: Read source truth files named by the drill.".to_string(),
                "beta: Re-run after fixing index failure.".to_string(),
            ],
            retrieval_blockers: Vec::new(),
        };
        output.repos[0].summary.bridges.graph_path = 0;
        output.repos[0].summary.bridges.partial = 2;

        let markdown = render_drill_suite_markdown(&output);

        assert!(markdown.contains("- repos: 2 total, 0 ready, 1 degraded, 1 blocked"));
        assert!(markdown.contains("- answer_quality: 0 ready, 0 degraded, 0 failed, 2 pending"));
        assert!(markdown.contains("| source truth | reports | next action |"));
        assert!(markdown.contains("| `alpha` | degraded | pending_source_verification"));
        assert!(markdown.contains("| `beta` | blocked | pending_source_verification"));
        assert!(markdown.contains("## Repo Artifacts"));
        assert!(markdown.contains("`alpha`: report `target/drill-suite/alpha/drill-report.md`; json `target/drill-suite/alpha/drill-report.json`; bridge artifacts `target/drill-suite/alpha/*-bridge.json`"));
        assert!(markdown.contains("## Next Actions"));
        assert!(markdown.contains("beta: Re-run after fixing index failure."));
    }

    #[test]
    fn drill_suite_markdown_uses_ledger_ready_next_action() {
        let mut repo = sample_drill_suite_repo("alpha", "degraded", 3, 0, 4);
        repo.summary.bridges.graph_path = 0;
        repo.summary.bridges.partial = 3;
        repo.answer_quality = sample_drill_suite_answer_quality("ready");
        repo.answer_quality.ledger_status = "present".to_string();
        repo.answer_quality.draft_written = Some(true);
        repo.answer_quality.claim_count = 3;
        repo.answer_quality.claim_correct_count = 3;
        let next_action = format!("{}: {}", repo.slug, drill_suite_next_action(&repo));
        let output = DrillSuiteOutput {
            suite: "generic-agent-drill".to_string(),
            project: "C:/repos/owner".to_string(),
            case_file: "C:/repos/owner/drill-cases.json".to_string(),
            output_dir: "target/drill-suite".to_string(),
            repo_count: 1,
            degraded_count: 1,
            blocked_count: 0,
            ready_count: 0,
            answer_ready_count: 1,
            answer_degraded_count: 0,
            answer_failed_count: 0,
            answer_pending_count: 0,
            repos: vec![repo],
            next_actions: vec![next_action],
            retrieval_blockers: Vec::new(),
        };

        let markdown = render_drill_suite_markdown(&output);

        assert!(markdown.contains("- answer_quality: 1 ready, 0 degraded, 0 failed, 0 pending"));
        assert!(markdown.contains("ledger claims=3 correct=3 partial=0 misleading=0 unsupported=0 revisions=0; packet 1 targets / 4 pending"));
        assert!(markdown.contains("answer is source-verified; improve graph/bridge evidence"));
        assert!(!markdown.contains("Read source truth files named by the drill."));
    }

    #[test]
    fn drill_suite_blocked_case_reports_case_local_failure() {
        let case = DrillSuiteCase {
            slug: "alpha".to_string(),
            project_root: PathBuf::from("C:/repos/alpha"),
            question: "Explain alpha.".to_string(),
            anchors: vec!["AlphaRoot".to_string(), "AlphaStore".to_string()],
            expectations: empty_drill_suite_expectations(),
        };

        let repo = blocked_drill_suite_repo_output(
            &case,
            Path::new("target/drill-suite/alpha-drill"),
            RefreshMode::Full,
            args::OutputFormat::Markdown,
            "missing checkout",
            None,
            false,
        );

        assert_eq!(repo.slug, "alpha");
        assert_eq!(repo.artifact_extension, "md");
        assert_eq!(repo.summary.verdict.status, "blocked");
        assert_eq!(repo.summary.anchors.requested, 2);
        assert_eq!(repo.summary.anchors.resolved, 0);
        assert_eq!(repo.summary.anchors.unresolved, 2);
        assert!(
            repo.summary
                .verdict
                .next_action
                .contains("missing checkout")
        );
    }

    #[test]
    fn drill_suite_retrieval_blockers_group_non_hybrid_repos() {
        let mut output = DrillSuiteOutput {
            suite: "generic-agent-drill".to_string(),
            project: "C:/repos/owner".to_string(),
            case_file: "C:/repos/owner/drill-cases.json".to_string(),
            output_dir: "target/drill-suite".to_string(),
            repo_count: 3,
            degraded_count: 2,
            blocked_count: 0,
            ready_count: 1,
            answer_ready_count: 0,
            answer_degraded_count: 0,
            answer_failed_count: 0,
            answer_pending_count: 3,
            repos: vec![
                sample_drill_suite_repo("alpha", "degraded", 3, 1, 4),
                sample_drill_suite_repo("beta", "ready", 3, 0, 4),
                sample_drill_suite_repo("gamma", "degraded", 3, 1, 4),
            ],
            next_actions: Vec::new(),
            retrieval_blockers: Vec::new(),
        };
        output.repos[0].summary.mechanical.retrieval_status =
            Some("symbolic:semantic_unavailable:diagnostic=MissingEmbeddingRuntime".to_string());
        output.repos[1].summary.mechanical.retrieval_status = Some("full".to_string());
        output.repos[2].summary.mechanical.retrieval_status =
            Some("symbolic:semantic_unavailable:diagnostic=MissingEmbeddingRuntime".to_string());

        output.retrieval_blockers = drill_suite_retrieval_blockers(&output.repos);

        assert_eq!(output.retrieval_blockers.len(), 1);
        let blocker = &output.retrieval_blockers[0];
        assert_eq!(
            blocker.status,
            "symbolic:semantic_unavailable:diagnostic=MissingEmbeddingRuntime"
        );
        assert_eq!(blocker.repo_count, 2);
        assert_eq!(blocker.repos, vec!["alpha", "gamma"]);
        assert!(blocker.next_action.contains("setup embeddings"));

        let markdown = render_drill_suite_markdown(&output);
        assert!(markdown.contains("## Retrieval Blockers"));
        assert!(markdown.contains("MissingEmbeddingRuntime"));
        assert!(markdown.contains("[alpha, gamma]"));
        assert!(!markdown.contains("beta]"));
    }

    #[test]
    fn drill_suite_retrieval_label_requires_full_sidecar_mode() {
        assert_eq!(drill_suite_retrieval_label(Some("full")), "full");
        assert_eq!(
            drill_suite_retrieval_label(Some("hybrid:semantic_ready")),
            "degraded"
        );
        assert_eq!(
            drill_suite_retrieval_label(Some(
                "no_semantic:sidecar_degraded; legacy=hybrid:semantic_ready"
            )),
            "needs-retrieval-repair"
        );
    }

    fn sample_drill_suite_repo(
        slug: &str,
        verdict: &str,
        bridge_total: usize,
        bridge_unresolved: usize,
        source_check_count: usize,
    ) -> DrillSuiteRepoOutput {
        DrillSuiteRepoOutput {
            slug: slug.to_string(),
            project: format!("C:/repos/{slug}"),
            question: format!("Question for {slug}"),
            anchors: vec!["A".to_string(), "B".to_string(), "C".to_string()],
            output_dir: format!("target/drill-suite/{slug}"),
            artifact_extension: "json".to_string(),
            summary: sample_drill_summary(
                slug,
                verdict,
                bridge_total,
                bridge_unresolved,
                source_check_count,
            ),
            expectations: empty_drill_suite_expectations(),
            answer_quality: sample_drill_suite_answer_quality("pending_source_verification"),
        }
    }

    fn sample_drill_suite_answer_quality(status: &str) -> DrillSuiteAnswerQualityOutput {
        DrillSuiteAnswerQualityOutput {
            ledger_status: "not_supplied".to_string(),
            final_answer_status: status.to_string(),
            draft_written: None,
            claim_count: 0,
            claim_correct_count: 0,
            claim_partial_count: 0,
            claim_misleading_count: 0,
            claim_unsupported_count: 0,
            claim_unclassified_count: 0,
            material_revision_count: 0,
            expected_file_count: 0,
            expected_file_found_count: 0,
            expected_file_missing_count: 0,
            expected_file_recall: None,
            missing_expected_files: Vec::new(),
            forbidden_claim_count: 0,
            forbidden_claim_hits: Vec::new(),
            layer_findings: Vec::new(),
            warnings: Vec::new(),
        }
    }

    fn sample_drill_summary(
        slug: &str,
        verdict: &str,
        bridge_total: usize,
        bridge_unresolved: usize,
        source_check_count: usize,
    ) -> DrillSummaryOutput {
        DrillSummaryOutput {
            summary_version: 1,
            project: format!("C:/repos/{slug}"),
            label: Some(slug.to_string()),
            question: Some(format!("Question for {slug}")),
            output_dir: format!("target/drill-suite/{slug}"),
            full_report_json: format!("target/drill-suite/{slug}/drill-report.json"),
            full_report_markdown: format!("target/drill-suite/{slug}/drill-report.md"),
            mechanical: DrillSummaryMechanicalOutput {
                refresh: "none".to_string(),
                before: drill_summary_stats(1, 2, 3, 0),
                after: drill_summary_stats(1, 2, 3, 0),
                index_ready: verdict != "blocked",
                error_delta: 0,
                retrieval_status: Some("full".to_string()),
                freshness_status: Some("fresh".to_string()),
                stale_file_count: 0,
                freshness_samples: Vec::new(),
                phase_timing_available: true,
                drill_timings: DrillRuntimeTimingsOutput::default(),
            },
            anchors: DrillSummaryAnchorsOutput {
                requested: 3,
                resolved: 3,
                unresolved: 0,
                failed_command_count: 0,
                statuses: Vec::new(),
            },
            bridges: DrillSummaryBridgesOutput {
                total: bridge_total,
                graph_path: bridge_total.saturating_sub(bridge_unresolved),
                partial: 0,
                unresolved_or_error: bridge_unresolved,
                statuses: Vec::new(),
            },
            source_truth: DrillSummarySourceTruthOutput {
                required: true,
                check_count: source_check_count,
                pending_check_count: source_check_count,
                verified_check_count: 0,
                target_file_count: 1,
                target_files: vec![format!("src/{slug}.rs")],
                target_file_details: Vec::new(),
                checklist_item_count: 4,
                claim_count: 3,
                pending_claim_count: 3,
                verified_claim_count: 0,
            },
            open_gaps: DrillSummaryOpenGapsOutput {
                overall_status: ClaimReadinessDto::NeedsSourceRead,
                safe_to_say_count: 1,
                inferred_claim_count: 1,
                needs_verification_count: source_check_count,
                needs_verification_claim_count: source_check_count,
                pending_source_truth_check_count: source_check_count,
                next_command_count: 1,
                answer_quality_status: "pending_source_verification".to_string(),
                pending_claim_count: 3,
                open_gap_friendly: true,
                status: "open_gaps_explicit".to_string(),
            },
            verdict: DrillSummaryVerdictOutput {
                status: verdict.to_string(),
                reason: "sample".to_string(),
                next_action: "Read source truth files named by the drill.".to_string(),
            },
        }
    }

    #[test]
    fn drill_evidence_packet_marks_partial_readiness_and_source_checks() {
        let mut anchor = sample_drill_anchor("WorkspaceIndexer", "a");
        anchor.commands.push(DrillCommandStatusOutput {
            command: "search".to_string(),
            status: "ok".to_string(),
            duration_ms: 19,
            artifact: Some("WorkspaceIndexer-search.md".to_string()),
            error: None,
        });
        anchor.verification_targets.push(VerificationTargetOutput {
            role: "definition".to_string(),
            path: "crates/codestory-indexer/src/lib.rs".to_string(),
            line: 644,
            node_ref: Some("crates/codestory-indexer/src/lib.rs:644:WorkspaceIndexer".to_string()),
            reason: "selected symbol definition".to_string(),
        });
        let bridge = DrillBridgeOutput {
            evidence: fallback_drill_bridge(
                Path::new("C:/repo"),
                &anchor,
                &sample_drill_anchor("SearchService", "b"),
                anchor.chosen_anchor.clone().expect("from"),
                sample_search_hit_output("b", "SearchService"),
                &sample_bridge_trail(false),
                Vec::new(),
                false,
            ),
            command: DrillCommandStatusOutput {
                command: "bridge".to_string(),
                status: "ok".to_string(),
                duration_ms: 2,
                artifact: Some("bridge.md".to_string()),
                error: None,
            },
        };
        let next_commands = vec![
            "codestory-cli search --project C:/repo --query WorkspaceIndexer --refresh none"
                .to_string(),
        ];

        let packet = drill_evidence_packet(
            Some("How does indexing flow?"),
            None,
            &[],
            &[anchor.clone()],
            &[bridge],
            &anchor.verification_targets,
            &next_commands,
        );

        assert_eq!(packet.packet_version, 1);
        assert!(
            packet.items.iter().any(|item| item.id == "anchor-1"
                && item.verification_status == ClaimReadinessDto::Anchored)
        );
        assert!(packet.items.iter().any(|item| item.id == "bridge-1"
            && item.verification_status == ClaimReadinessDto::NeedsSourceRead));
        assert_eq!(
            packet.readiness.overall_status,
            ClaimReadinessDto::NeedsSourceRead
        );
        assert_eq!(
            packet.readiness.source_truth_checks.len(),
            2,
            "source-truth checks should group repeated bridge endpoint files: {packet:#?}"
        );
        assert!(
            packet
                .readiness
                .source_truth_checks
                .iter()
                .any(|check| check.reason.contains("bridge endpoint")),
            "grouped source-truth checks should preserve bridge endpoint role: {packet:#?}"
        );
        assert_eq!(packet.readiness.next_commands, next_commands);
    }

    #[test]
    fn drill_evidence_packet_requires_source_truth_targets() {
        let anchor = sample_drill_anchor("WorkspaceIndexer", "a");

        let packet = drill_evidence_packet(None, None, &[], &[anchor], &[], &[], &[]);

        assert_eq!(
            packet.readiness.overall_status,
            ClaimReadinessDto::NeedsSourceRead
        );
        assert!(
            packet
                .readiness
                .needs_verification
                .iter()
                .any(|item| item.contains("no source-truth targets were emitted"))
        );
    }

    #[test]
    fn drill_answer_readiness_requires_pending_source_truth_checks() {
        let item = EvidenceItemDto {
            id: "symbol-1".to_string(),
            evidence_type: EvidenceTypeDto::SymbolContext,
            command: "symbol".to_string(),
            status: "ok".to_string(),
            confidence: "high".to_string(),
            verification_status: ClaimReadinessDto::Supported,
            match_quality: None,
            source: None,
            artifacts: Vec::new(),
            notes: Vec::new(),
        };
        let check = SourceTruthCheckDto {
            id: "source-truth-1".to_string(),
            reason: "verify definition evidence".to_string(),
            path: "src/lib.rs".to_string(),
            line: Some(12),
            required: true,
        };

        let readiness = drill_answer_readiness(&[item], &[check], &[]);

        assert_eq!(readiness.overall_status, ClaimReadinessDto::NeedsSourceRead);
        assert!(
            readiness
                .needs_verification
                .iter()
                .any(|item| item.contains("required source-truth checks are pending"))
        );
    }

    #[test]
    fn drill_anchor_validation_rejects_empty_after_normalization() {
        let error =
            drill_targeting::validated_drill_anchors(&[",, ,".to_string()], "drill").unwrap_err();

        assert!(
            error
                .to_string()
                .contains("drill must name at least one anchor"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn drill_anchor_selection_does_not_guess_semantic_neighbors() {
        let neighbor = sample_runtime_hit(
            "neighbor",
            "ElsewhereFeedProps",
            NodeKind::TYPEDEF,
            Path::new("src/components/ElsewhereFeed.tsx"),
            1,
        );

        assert!(drill_targeting::choose_drill_anchor_hit("ElsewherePage", &[neighbor]).is_none());
    }

    #[test]
    fn drill_question_search_is_partial_discovery_evidence() {
        let question_search = DrillCommandStatusOutput {
            command: "question_search".to_string(),
            status: "ok".to_string(),
            duration_ms: 23,
            artifact: Some("question-search.md".to_string()),
            error: None,
        };

        let packet = drill_evidence_packet(
            Some("How does the public page connect to the feed?"),
            Some(&question_search),
            &[],
            &[],
            &[],
            &[],
            &[],
        );

        let question_item = packet
            .items
            .iter()
            .find(|item| item.id == "question-search")
            .expect("question search evidence item");
        assert_eq!(
            question_item.verification_status,
            ClaimReadinessDto::Partial
        );
        assert!(
            question_item
                .notes
                .iter()
                .any(|note| note.contains("broad discovery evidence")),
            "question search should not look like proof: {question_item:#?}"
        );
    }

    #[test]
    fn drill_question_search_targets_runtime_files_without_claiming_proof() {
        let mut page = sample_search_hit_output("page", "HomePage");
        page.file_path = Some("src/app/(frontend)/page.tsx".to_string());
        page.line = Some(11);
        page.match_quality = SearchMatchQualityDto::Prefix;
        page.verification_targets = vec![VerificationTargetOutput {
            role: "definition".to_string(),
            path: "src/app/(frontend)/page.tsx".to_string(),
            line: 11,
            node_ref: Some("src/app/(frontend)/page.tsx:11:HomePage".to_string()),
            reason: "primary source occurrence selected for this symbol".to_string(),
        }];
        let mut component = sample_search_hit_output("home", "RootRuntimeHome");
        component.file_path = Some("src/components/RootRuntimeHome.tsx".to_string());
        component.line = Some(237);
        component.match_quality = SearchMatchQualityDto::Prefix;
        component.verification_targets = vec![VerificationTargetOutput {
            role: "definition".to_string(),
            path: "src/components/RootRuntimeHome.tsx".to_string(),
            line: 237,
            node_ref: Some("src/components/RootRuntimeHome.tsx:237:RootRuntimeHome".to_string()),
            reason: "primary source occurrence selected for this symbol".to_string(),
        }];
        let mut collection = sample_search_hit_output("comments", "Comments");
        collection.file_path = Some("src/collections/Comments.ts".to_string());
        collection.line = Some(11);
        collection.verification_targets = vec![VerificationTargetOutput {
            role: "definition".to_string(),
            path: "src/collections/Comments.ts".to_string(),
            line: 11,
            node_ref: Some("src/collections/Comments.ts:11:Comments".to_string()),
            reason: "primary source occurrence selected for this symbol".to_string(),
        }];
        let mut lockfile = sample_search_hit_output("lock", "Cargo.lock");
        lockfile.file_path = Some("Cargo.lock".to_string());
        lockfile.line = Some(1);

        let output = SearchOutput {
            query: "How do public pages connect to comments?".to_string(),
            retrieval: sample_retrieval(),
            retrieval_shadow: None,
            freshness: None,
            limit_per_source: 10,
            repo_text_mode: RepoTextMode::On,
            repo_text_enabled: true,
            query_assessment: None,
            search_plan: None,
            explain: true,
            query_hints: Vec::new(),
            suggestions: Vec::new(),
            indexed_symbol_hits: vec![lockfile, collection, component, page],
            repo_text_hits: Vec::new(),
            repo_text_stats: None,
        };

        let targets = drill_question_search_verification_targets(
            &output,
            "question search source-truth target",
            8,
        );
        let paths = targets
            .iter()
            .map(|target| target.path.as_str())
            .collect::<Vec<_>>();

        assert_eq!(
            paths,
            vec![
                "src/app/(frontend)/page.tsx",
                "src/components/RootRuntimeHome.tsx",
                "src/collections/Comments.ts",
            ]
        );
        assert!(
            targets.iter().all(|target| target
                .reason
                .contains("question search source-truth target")),
            "question-derived targets must stay explicitly provisional: {targets:#?}"
        );
    }

    #[test]
    fn drill_question_supplemental_queries_cover_public_payload_and_store_terms() {
        let mut posts = sample_drill_anchor("Posts", "posts");
        posts.chosen_anchor.as_mut().expect("anchor").file_path =
            Some("src/collections/Posts.ts".to_string());

        let queries = drill_question_supplemental_queries(
            Path::new("C:/repo/codestory"),
            "Explain how public writing/social surfaces connect to Payload collections, comment auth, and the elsewhere feed.",
            &[posts],
        );

        assert!(queries.iter().any(|query| query == "Home"));
        assert!(queries.iter().any(|query| query == "Comments"));
        assert!(queries.iter().any(|query| query == "Posts"));
        assert!(queries.iter().any(|query| query == "social entries"));
        assert!(queries.iter().any(|query| query == "elsewhere feed"));

        let store_queries = drill_question_supplemental_queries(
            Path::new("C:/repo/codestory"),
            "Explain how the indexer store supports search, trail, and snippet.",
            &[],
        );
        assert!(store_queries.iter().any(|query| query == "codestory-store"));
        assert!(store_queries.iter().any(|query| query == "Store"));
    }

    #[test]
    fn stdio_metadata_lists_tools_resources_and_prompts() {
        let tools = stdio_tools_list_json();
        let tool_names = tools["result"]["tools"]
            .as_array()
            .expect("tools")
            .iter()
            .filter_map(|tool| tool["name"].as_str())
            .collect::<Vec<_>>();
        assert!(tool_names.contains(&"definition"));
        assert!(tool_names.contains(&"references"));

        let resources = stdio_resources_list_json();
        assert!(
            resources["result"]["resources"]
                .as_array()
                .expect("resources")
                .iter()
                .any(|resource| resource["uri"] == "codestory://grounding")
        );

        let prompts = stdio_prompts_list_json();
        assert!(
            prompts["result"]["prompts"]
                .as_array()
                .expect("prompts")
                .iter()
                .any(|prompt| prompt["name"] == "impact_analysis")
        );
    }

    #[test]
    fn index_watch_rejects_output_file_inside_project_tree() {
        let temp = tempdir().expect("create temp dir");
        let project = temp.path().join("repo");
        fs::create_dir_all(&project).expect("create project");
        let cmd = IndexCommand {
            project: args::ProjectArgs {
                project: project.clone(),
                cache_dir: None,
            },
            refresh: args::RefreshMode::Auto,
            format: args::OutputFormat::Markdown,
            output_file: Some(project.join("index.md")),
            dry_run: false,
            summarize: false,
            progress: false,
            watch: true,
        };

        let error =
            validate_index_watch_output_file(&cmd).expect_err("in-tree output should be rejected");

        assert!(
            error
                .to_string()
                .contains("--watch cannot write --output-file inside the watched project"),
            "{error:#}"
        );
    }

    #[test]
    fn non_trail_commands_reject_dot_format_before_running() {
        let error =
            ensure_dot_only_for_trail(args::OutputFormat::Dot, "search").expect_err("reject dot");

        assert!(
            error
                .to_string()
                .contains("--format dot is only supported by `trail`"),
            "{error:#}"
        );
    }

    #[test]
    fn hide_speculative_trail_edges_prunes_disconnected_nodes() {
        let context = TrailContextDto {
            focus: sample_node_details("a", "A"),
            trail: GraphResponse {
                center_id: NodeId("a".to_string()),
                nodes: vec![
                    sample_graph_node("a", "A"),
                    sample_graph_node("b", "B"),
                    sample_graph_node("c", "C"),
                    sample_graph_node("d", "D"),
                ],
                edges: vec![
                    sample_graph_edge("e1", "a", "b", Some("certain")),
                    sample_graph_edge("e2", "b", "c", Some("uncertain")),
                    sample_graph_edge("e3", "c", "d", Some("certain")),
                ],
                truncated: false,
                omitted_edge_count: 0,
                canonical_layout: None,
            },
            story: None,
        };

        let filtered = hide_speculative_trail_edges(context);
        let node_ids = filtered
            .trail
            .nodes
            .iter()
            .map(|node| node.id.0.as_str())
            .collect::<Vec<_>>();
        let edge_ids = filtered
            .trail
            .edges
            .iter()
            .map(|edge| edge.id.0.as_str())
            .collect::<Vec<_>>();

        assert_eq!(node_ids, vec!["a", "b"]);
        assert_eq!(edge_ids, vec!["e1"]);
        assert_eq!(filtered.trail.omitted_edge_count, 2);
    }

    #[test]
    fn explicit_cache_dir_is_not_hashed() {
        let root = Path::new("C:/repo");
        let cache_dir = Path::new("C:/cache/custom");
        assert_eq!(
            cache_root_for_project(root, Some(cache_dir)).expect("cache dir"),
            cache_dir
        );
    }

    #[test]
    fn default_cache_root_uses_project_hash() {
        let root = Path::new("C:/repo");
        let cache_root = cache_root_for_project(root, None).expect("cache root");
        let cache_root = cache_root.to_string_lossy();
        assert!(
            cache_root.ends_with(&fnv1a_hex(b"C:/repo")),
            "default cache root should end with the project hash"
        );
    }

    #[test]
    fn resolution_prefers_exact_type_name_over_member_hits() {
        let query = "AppController";
        let mut hits = [
            SearchHit {
                node_id: NodeId("2".to_string()),
                display_name: "AppController::open_project".to_string(),
                kind: codestory_contracts::api::NodeKind::FUNCTION,
                file_path: None,
                line: None,
                score: 0.9,
                origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
                match_quality: None,
                resolvable: true,
                score_breakdown: None,
                ..test_search_hit_defaults()
            },
            SearchHit {
                node_id: NodeId("1".to_string()),
                display_name: "AppController".to_string(),
                kind: codestory_contracts::api::NodeKind::CLASS,
                file_path: None,
                line: None,
                score: 0.9,
                origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
                match_quality: None,
                resolvable: true,
                score_breakdown: None,
                ..test_search_hit_defaults()
            },
        ];

        hits.sort_by(|left, right| compare_resolution_hits(query, left, right));
        assert_eq!(hits[0].display_name, "AppController");
    }

    #[test]
    fn resolution_prefers_declaration_anchor_over_impl_anchor() {
        let temp = tempdir().expect("create temp dir");
        let file_path = temp.path().join("lib.rs");
        fs::write(
            &file_path,
            "pub struct AppController;\nimpl AppController {\n    fn open_project(&self) {}\n}\n",
        )
        .expect("write file");

        let query = "AppController";
        let mut hits = [
            SearchHit {
                node_id: NodeId("2".to_string()),
                display_name: "AppController".to_string(),
                kind: codestory_contracts::api::NodeKind::CLASS,
                file_path: Some(file_path.to_string_lossy().to_string()),
                line: Some(2),
                score: 1.0,
                origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
                match_quality: None,
                resolvable: true,
                score_breakdown: None,
                ..test_search_hit_defaults()
            },
            SearchHit {
                node_id: NodeId("1".to_string()),
                display_name: "AppController".to_string(),
                kind: codestory_contracts::api::NodeKind::STRUCT,
                file_path: Some(file_path.to_string_lossy().to_string()),
                line: Some(1),
                score: 1.0,
                origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
                match_quality: None,
                resolvable: true,
                score_breakdown: None,
                ..test_search_hit_defaults()
            },
        ];

        hits.sort_by(|left, right| compare_resolution_hits(query, left, right));
        assert_eq!(hits[0].line, Some(1));
        assert_eq!(hits[0].kind, codestory_contracts::api::NodeKind::STRUCT);
    }

    #[test]
    fn resolution_prefers_callable_definitions_over_unknown_hits() {
        let query = "check_winner";
        let mut hits = [
            SearchHit {
                node_id: NodeId("2".to_string()),
                display_name: "check_winner".to_string(),
                kind: codestory_contracts::api::NodeKind::UNKNOWN,
                file_path: Some("src/callsite.rs".to_string()),
                line: Some(20),
                score: 0.9,
                origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
                match_quality: None,
                resolvable: true,
                score_breakdown: None,
                ..test_search_hit_defaults()
            },
            SearchHit {
                node_id: NodeId("1".to_string()),
                display_name: "check_winner".to_string(),
                kind: codestory_contracts::api::NodeKind::FUNCTION,
                file_path: Some("src/game.rs".to_string()),
                line: Some(10),
                score: 0.8,
                origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
                match_quality: None,
                resolvable: true,
                score_breakdown: None,
                ..test_search_hit_defaults()
            },
        ];

        hits.sort_by(|left, right| compare_resolution_hits(query, left, right));
        assert_eq!(hits[0].kind, codestory_contracts::api::NodeKind::FUNCTION);
    }

    #[test]
    fn resolution_prefers_callable_implementation_over_declaration() {
        let temp = tempdir().expect("create temp dir");
        let declaration_path = temp.path().join("Project.h");
        let implementation_path = temp.path().join("Project.cpp");
        fs::write(&declaration_path, "void buildIndex() const;\n").expect("write declaration");
        fs::write(
            &implementation_path,
            "void Project::buildIndex() const\n{\n    runIndexer();\n}\n",
        )
        .expect("write implementation");

        let query = "Project::buildIndex";
        let mut hits = [
            SearchHit {
                node_id: NodeId("declaration".to_string()),
                display_name: "Project::buildIndex".to_string(),
                kind: codestory_contracts::api::NodeKind::METHOD,
                file_path: Some(declaration_path.to_string_lossy().to_string()),
                line: Some(1),
                score: 1.0,
                origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
                match_quality: None,
                resolvable: true,
                score_breakdown: None,
                ..test_search_hit_defaults()
            },
            SearchHit {
                node_id: NodeId("implementation".to_string()),
                display_name: "Project::buildIndex".to_string(),
                kind: codestory_contracts::api::NodeKind::FUNCTION,
                file_path: Some(implementation_path.to_string_lossy().to_string()),
                line: Some(1),
                score: 1.0,
                origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
                match_quality: None,
                resolvable: true,
                score_breakdown: None,
                ..test_search_hit_defaults()
            },
        ];

        hits.sort_by(|left, right| compare_resolution_hits(query, left, right));
        assert_eq!(hits[0].node_id.0, "implementation");
    }

    #[test]
    fn resolution_prefers_multiline_callable_implementation_over_declaration() {
        let temp = tempdir().expect("create temp dir");
        let declaration_path = temp.path().join("IndexerJava.h");
        let implementation_path = temp.path().join("IndexerJava.cpp");
        fs::write(
            &declaration_path,
            "void doIndex(\n    Command command,\n    State state) override;\n",
        )
        .expect("write declaration");
        fs::write(
            &implementation_path,
            "void IndexerJava::doIndex(\n    Command command,\n    State state)\n{\n    parse(command);\n}\n",
        )
        .expect("write implementation");

        let query = "IndexerJava::doIndex";
        let mut hits = [
            SearchHit {
                node_id: NodeId("declaration".to_string()),
                display_name: "IndexerJava::doIndex".to_string(),
                kind: codestory_contracts::api::NodeKind::METHOD,
                file_path: Some(declaration_path.to_string_lossy().to_string()),
                line: Some(1),
                score: 1.0,
                origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
                match_quality: None,
                resolvable: true,
                score_breakdown: None,
                ..test_search_hit_defaults()
            },
            SearchHit {
                node_id: NodeId("implementation".to_string()),
                display_name: "IndexerJava::doIndex".to_string(),
                kind: codestory_contracts::api::NodeKind::FUNCTION,
                file_path: Some(implementation_path.to_string_lossy().to_string()),
                line: Some(1),
                score: 1.0,
                origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
                match_quality: None,
                resolvable: true,
                score_breakdown: None,
                ..test_search_hit_defaults()
            },
        ];

        hits.sort_by(|left, right| compare_resolution_hits(query, left, right));
        assert_eq!(hits[0].node_id.0, "implementation");
    }

    #[test]
    fn resolution_keeps_callable_implementation_when_body_contains_assignment() {
        let temp = tempdir().expect("create temp dir");
        let declaration_path = temp.path().join("Foo.h");
        let implementation_path = temp.path().join("Foo.cpp");
        fs::write(&declaration_path, "void bar();\n").expect("write declaration");
        fs::write(
            &implementation_path,
            "void Foo::bar()\n{\n    int status = 0;\n    use(status);\n}\n",
        )
        .expect("write implementation");

        let query = "Foo::bar";
        let mut hits = [
            SearchHit {
                node_id: NodeId("declaration".to_string()),
                display_name: "Foo::bar".to_string(),
                kind: codestory_contracts::api::NodeKind::METHOD,
                file_path: Some(declaration_path.to_string_lossy().to_string()),
                line: Some(1),
                score: 1.0,
                origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
                match_quality: None,
                resolvable: true,
                score_breakdown: None,
                ..test_search_hit_defaults()
            },
            SearchHit {
                node_id: NodeId("implementation".to_string()),
                display_name: "Foo::bar".to_string(),
                kind: codestory_contracts::api::NodeKind::FUNCTION,
                file_path: Some(implementation_path.to_string_lossy().to_string()),
                line: Some(1),
                score: 1.0,
                origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
                match_quality: None,
                resolvable: true,
                score_breakdown: None,
                ..test_search_hit_defaults()
            },
        ];

        hits.sort_by(|left, right| compare_resolution_hits(query, left, right));
        assert_eq!(hits[0].node_id.0, "implementation");
    }

    fn sample_runtime_hit(
        id: &str,
        display_name: &str,
        kind: NodeKind,
        file_path: &Path,
        line: u32,
    ) -> SearchHit {
        SearchHit {
            node_id: NodeId(id.to_string()),
            display_name: display_name.to_string(),
            kind,
            file_path: Some(file_path.to_string_lossy().to_string()),
            line: Some(line),
            score: 1.0,
            origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
            match_quality: None,
            resolvable: true,
            score_breakdown: None,
            ..test_search_hit_defaults()
        }
    }

    #[test]
    fn function_body_promotion_keeps_non_callable_selected_anchor() {
        let temp = tempdir().expect("create temp dir");
        let source_path = temp.path().join("posts.tsx");
        fs::write(
            &source_path,
            "export const Posts = { slug: \"posts\" };\n\nexport function PostsIndexPage() {\n  return \"posts\";\n}\n",
        )
        .expect("write source");

        let selected = sample_runtime_hit("posts", "Posts", NodeKind::CLASS, &source_path, 1);
        let alternative = sample_runtime_hit(
            "page",
            "PostsIndexPage",
            NodeKind::FUNCTION,
            &source_path,
            3,
        );
        let target = runtime::ResolvedTarget {
            selector: QuerySelectorOutput::Query,
            requested: "Posts".to_string(),
            file_filter: None,
            selected,
            alternatives: vec![alternative],
        };

        let promoted = prefer_function_body_target(temp.path(), target);

        assert_eq!(promoted.selected.node_id.0, "posts");
        assert_eq!(promoted.selected.display_name, "Posts");
    }

    #[test]
    fn function_body_promotion_keeps_same_callable_implementation() {
        let temp = tempdir().expect("create temp dir");
        let declaration_path = temp.path().join("Project.h");
        let implementation_path = temp.path().join("Project.cpp");
        fs::write(&declaration_path, "void Project::buildIndex();\n").expect("write declaration");
        fs::write(
            &implementation_path,
            "void Project::buildIndex()\n{\n    runIndexer();\n}\n",
        )
        .expect("write implementation");

        let selected = sample_runtime_hit(
            "declaration",
            "Project::buildIndex",
            NodeKind::METHOD,
            &declaration_path,
            1,
        );
        let alternative = sample_runtime_hit(
            "implementation",
            "Project::buildIndex",
            NodeKind::FUNCTION,
            &implementation_path,
            1,
        );
        let target = runtime::ResolvedTarget {
            selector: QuerySelectorOutput::Query,
            requested: "Project::buildIndex".to_string(),
            file_filter: None,
            selected,
            alternatives: vec![alternative],
        };

        let promoted = prefer_function_body_target(temp.path(), target);

        assert_eq!(promoted.selected.node_id.0, "implementation");
    }

    #[test]
    fn clean_path_unix_noop() {
        assert_eq!(clean_path_string("src/lib.rs"), "src/lib.rs");
    }

    #[test]
    fn clean_path_backslash_normalization() {
        assert_eq!(clean_path_string("C:\\foo\\bar"), "C:/foo/bar");
    }

    #[test]
    fn clean_path_extended_prefix_stripped() {
        assert_eq!(clean_path_string("\\\\?\\C:\\foo\\bar"), "C:/foo/bar");
    }

    #[test]
    fn clean_path_extended_prefix_unc() {
        assert_eq!(
            clean_path_string("\\\\?\\UNC\\server\\share"),
            "//server/share"
        );
    }

    #[test]
    fn relative_path_strips_root() {
        let root = Path::new("C:/repo");
        assert_eq!(relative_path(root, "C:/repo/src/lib.rs"), "src/lib.rs");
    }

    #[test]
    fn relative_path_outside_root() {
        let root = Path::new("C:/repo");
        assert_eq!(
            relative_path(root, "D:\\other\\file.rs"),
            "D:/other/file.rs"
        );
    }

    #[test]
    fn relative_path_extended_prefix_unc_keeps_share_format() {
        let root = Path::new("C:/repo");
        assert_eq!(
            relative_path(root, "\\\\?\\UNC\\server\\share\\file.rs"),
            "//server/share/file.rs"
        );
    }

    #[test]
    fn cli_sources_do_not_depend_on_index_or_storage_layers_directly() {
        let src_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
        let forbidden = [
            ["codestory_", "index::"].concat(),
            ["codestory_", "storage::"].concat(),
            ["codestory_", "project::"].concat(),
        ];

        for entry in fs::read_dir(src_dir).expect("read cli src dir") {
            let entry = entry.expect("src entry");
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
                continue;
            }

            let contents = fs::read_to_string(&path).expect("read source");
            for needle in &forbidden {
                assert!(
                    !contents.contains(needle),
                    "CLI source {} should not depend directly on {needle}",
                    path.display()
                );
            }
        }
    }
}
