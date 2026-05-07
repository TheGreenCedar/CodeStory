use anyhow::{Context, Result, bail};
use clap::{CommandFactory, Parser};
use clap_complete::{Shell, generate};
use codestory_contracts::api::{
    AgentAnswerDto, AgentAskRequest, AgentConnectionSettingsDto, AgentHybridWeightsDto,
    AgentResponseModeDto, AppEventPayload, BookmarkCategoryDto, BookmarkDto,
    CreateBookmarkCategoryRequest, CreateBookmarkRequest, GraphArtifactDto, IndexFreshnessDto,
    IndexFreshnessStatusDto, IndexMode, NodeId, NodeKind, RepoTextScanStatsDto,
    RetrievalFallbackReasonDto, RetrievalScoreBreakdownDto, SearchHit, SearchHybridLimitsDto,
    SearchRepoTextMode, SearchRequest,
};
use std::{
    collections::HashMap,
    fmt::Write as _,
    fs,
    io::IsTerminal,
    net::TcpListener,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

mod args;
mod config;
mod display;
mod explore;
mod http_transport;
mod managed_embeddings;
mod output;
mod query_resolution;
mod runtime;
mod stdio_catalog;
mod stdio_transport;

use args::{
    AskCommand, BookmarkAction, BookmarkAddCommand, BookmarkAddOutput, BookmarkCommand,
    BookmarkListCommand, BookmarkListOutput, BookmarkOutput, BookmarkRemoveCommand,
    BookmarkRemoveOutput, Cli, Command, CompletionShell, DoctorCheckOutput, DoctorCommand,
    DoctorOutput, GenerateCompletionsCommand, GroundCommand, IndexCommand, IndexDryRunOutput,
    IndexOutput, QueryCommand, QueryOutput, QueryResolutionOutput, RepoTextMode, SearchCommand,
    SearchHitOutput, SearchOutput, ServeCommand, SetupAction, SetupCommand, SnippetCommand,
    SnippetJsonOutput, SymbolCommand, SymbolJsonOutput, TrailCommand, TrailJsonOutput,
    ask_retrieval_profile, build_trail_request,
};
#[cfg(test)]
use codestory_contracts::api::TrailContextDto;
#[cfg(test)]
use explore::{ExploreTuiAction, ExploreTuiState, explore_tui_action};
#[cfg(test)]
use http_transport::search_repo_text_mode_param;
use output::{
    emit, emit_text, render_agent_answer_markdown, render_doctor_markdown, render_ground_markdown,
    render_index_dry_run_markdown, render_index_markdown, render_query_markdown,
    render_search_markdown, render_snippet_markdown, render_symbol_markdown, render_symbol_mermaid,
    render_trail_dot, render_trail_markdown, render_trail_mermaid, render_trail_story_markdown,
    validate_output_file_parent,
};
use runtime::{
    AmbiguousTargetError, RuntimeContext, ensure_index_ready, map_api_error, refresh_label,
    resolve_refresh_request, resolve_target,
};
#[cfg(test)]
use std::collections::HashSet;
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

const ASK_BUNDLE_OUTPUT_BYTE_CAP: usize = 5 * 1024 * 1024;
const ASK_BUNDLE_MARKDOWN_SOFT_CAP: usize = 2 * 1024 * 1024;
const ASK_BUNDLE_TRUNCATION_SUFFIX: &str =
    "\n\n... bundle content truncated by ask bundle byte cap\n";

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

fn hybrid_weights(
    lexical: Option<f32>,
    semantic: Option<f32>,
    graph: Option<f32>,
) -> Option<AgentHybridWeightsDto> {
    (lexical.is_some() || semantic.is_some() || graph.is_some()).then_some(AgentHybridWeightsDto {
        lexical,
        semantic,
        graph,
    })
}

fn hybrid_limits(lexical: Option<u32>, semantic: Option<u32>) -> Option<SearchHybridLimitsDto> {
    (lexical.is_some() || semantic.is_some()).then_some(SearchHybridLimitsDto { lexical, semantic })
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Index(cmd) => run_index(cmd),
        Command::Ground(cmd) => run_ground(cmd),
        Command::Ask(cmd) => run_ask(cmd),
        Command::Doctor(cmd) => run_doctor(cmd),
        Command::Setup(cmd) => run_setup(cmd),
        Command::Search(cmd) => run_search(cmd),
        Command::Symbol(cmd) => run_symbol(cmd),
        Command::Trail(cmd) => run_trail(cmd),
        Command::Snippet(cmd) => run_snippet(cmd),
        Command::Query(cmd) => run_query(cmd),
        Command::Explore(cmd) => explore::run_explore(cmd),
        Command::Bookmark(cmd) => run_bookmark(cmd),
        Command::Serve(cmd) => run_serve(cmd),
        Command::GenerateCompletions(cmd) => run_generate_completions(cmd),
    }
}

fn run_setup(cmd: SetupCommand) -> Result<()> {
    match cmd.action {
        SetupAction::Embeddings(cmd) => run_setup_embeddings(cmd),
    }
}

fn run_setup_embeddings(cmd: args::SetupEmbeddingsCommand) -> Result<()> {
    ensure_dot_only_for_trail(cmd.format, "setup embeddings")?;
    preflight_output_file(cmd.output_file.as_deref())?;
    let next_commands =
        setup_embeddings_next_commands(&cmd.project.project, cmd.project.cache_dir.as_deref());
    let project_root = runtime::canonicalize_project_root(&cmd.project.project)?;
    let config = config::load_config(&project_root)?;
    let cache_override = cmd
        .project
        .cache_dir
        .as_deref()
        .or(config.cache_dir.as_deref());
    let managed_root = managed_embeddings::managed_root(cache_override)?;
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
    format!(
        "\"{}\"",
        display::clean_path_string(&path.to_string_lossy()).replace('"', "\\\"")
    )
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
    let runtime = RuntimeContext::new(&cmd.project)?;
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
    let output = IndexOutput {
        project: &opened.summary.root,
        storage_path: &storage_path,
        refresh: &refresh_label,
        summary: &opened.summary,
        retrieval,
        phase_timings: opened.phase_timings.as_ref(),
        summary_generation: summary_generation.as_ref(),
        next_commands: index_next_commands(&opened.summary.root, Some(retrieval)),
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

fn run_ask(cmd: AskCommand) -> Result<()> {
    ensure_dot_only_for_trail(cmd.format, "ask")?;
    preflight_output_file(cmd.output_file.as_deref())?;
    let runtime = RuntimeContext::new(&cmd.project)?;
    let opened = runtime.ensure_open(cmd.refresh)?;
    ensure_index_ready(&opened, "ask")?;

    let bookmark_focus = cmd
        .bookmark
        .as_deref()
        .map(|id| load_bookmark_focus_by_id(&runtime, id))
        .transpose()?;
    let request = AgentAskRequest {
        prompt: cmd.prompt.clone(),
        retrieval_profile: ask_retrieval_profile(&cmd),
        focus_node_id: bookmark_focus
            .as_ref()
            .map(|bookmark| bookmark.node_id.clone())
            .or_else(|| {
                cmd.focus_id
                    .as_ref()
                    .map(|id| NodeId(id.trim().to_string()))
            }),
        max_results: Some(cmd.max_results.clamp(1, 25)),
        response_mode: AgentResponseModeDto::Markdown,
        latency_budget_ms: None,
        include_evidence: !cmd.no_evidence,
        hybrid_weights: hybrid_weights(cmd.hybrid_lexical, cmd.hybrid_semantic, cmd.hybrid_graph),
        connection: AgentConnectionSettingsDto {
            backend: cmd.backend.into(),
            command: cmd.agent_command.clone(),
        },
        run_local_agent: cmd.with_local_agent,
    };

    let mut answer = if cmd.with_local_agent {
        runtime.agent.ask(request).map_err(map_api_error)?
    } else {
        runtime.browser.ask(request).map_err(map_api_error)?
    };
    if let Some(bookmark) = bookmark_focus.as_ref() {
        annotate_answer_with_bookmark_focus(&mut answer, bookmark);
    }
    let markdown = render_agent_answer_markdown(&runtime.project_root, &answer);
    if let Some(bundle_dir) = cmd.bundle.as_deref() {
        write_ask_bundle(bundle_dir, &answer, &markdown)?;
    }
    emit(cmd.format, &answer, markdown, cmd.output_file.as_deref())
}

fn annotate_answer_with_bookmark_focus(answer: &mut AgentAnswerDto, bookmark: &BookmarkDto) {
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

fn run_search(cmd: SearchCommand) -> Result<()> {
    ensure_dot_only_for_trail(cmd.format, "search")?;
    preflight_output_file(cmd.output_file.as_deref())?;
    let runtime = RuntimeContext::new(&cmd.project)?;
    let opened = runtime.ensure_open(cmd.refresh)?;
    ensure_index_ready(&opened, "search")?;
    let search_results = runtime
        .browser
        .search_results(search_request_from_command(&cmd))
        .map_err(map_api_error)?;
    let output = search_output_from_results(&runtime.project_root, &search_results, cmd.why);
    let markdown = render_search_markdown(&runtime.project_root, &output);
    emit(cmd.format, &output, markdown, cmd.output_file.as_deref())
}

fn search_request_from_command(cmd: &SearchCommand) -> SearchRequest {
    SearchRequest {
        query: cmd.query.clone(),
        repo_text: to_api_repo_text_mode(cmd.repo_text),
        limit_per_source: cmd.limit.clamp(1, 50),
        hybrid_weights: hybrid_weights(cmd.hybrid_lexical, cmd.hybrid_semantic, cmd.hybrid_graph),
        hybrid_limits: hybrid_limits(cmd.hybrid_lexical_limit, cmd.hybrid_semantic_limit),
    }
}

fn search_output_from_results(
    project_root: &std::path::Path,
    search_results: &codestory_contracts::api::SearchResultsDto,
    include_score_details: bool,
) -> SearchOutput {
    build_search_output(
        project_root,
        &search_results.query,
        &search_results.retrieval,
        search_results.freshness.as_ref(),
        &search_results.indexed_symbol_hits,
        &search_results.repo_text_hits,
        search_results.repo_text_stats.as_ref(),
        &search_results.suggestions,
        search_results.limit_per_source,
        RepoTextOutputConfig {
            mode: from_api_repo_text_mode(search_results.repo_text_mode),
            enabled: search_results.repo_text_enabled,
        },
        include_score_details,
    )
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
    let markdown = render_symbol_markdown(&runtime.project_root, &target, &context);
    let output = SymbolJsonOutput {
        resolution: build_query_resolution_output(&runtime.project_root, &target),
        symbol: &context,
    };
    emit(cmd.format, &output, markdown, cmd.output_file.as_deref())
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
    let markdown = if let Some(story) = context.story.as_ref() {
        render_trail_story_markdown(&runtime.project_root, &target, &context, &cmd, story)
    } else {
        render_trail_markdown(&runtime.project_root, &target, &context, &cmd)
    };
    let output = TrailJsonOutput {
        resolution: build_query_resolution_output(&runtime.project_root, &target),
        trail: &context,
    };
    emit(cmd.format, &output, markdown, cmd.output_file.as_deref())
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
    let context = runtime
        .browser
        .snippet_context(target.selected.node_id.clone(), cmd.context)
        .map_err(map_api_error)?;
    let colorize = cmd.format == args::OutputFormat::Markdown
        && cmd.output_file.is_none()
        && std::io::stdout().is_terminal();
    let markdown = render_snippet_markdown(&runtime.project_root, &target, &context, colorize);
    let output = SnippetJsonOutput {
        resolution: build_query_resolution_output(&runtime.project_root, &target),
        snippet: &context,
    };
    emit(cmd.format, &output, markdown, cmd.output_file.as_deref())
}

fn run_query(cmd: QueryCommand) -> Result<()> {
    ensure_dot_only_for_trail(cmd.format, "query")?;
    preflight_output_file(cmd.output_file.as_deref())?;
    let ast = codestory_runtime::parse_graph_query(&cmd.query)
        .map_err(|error| anyhow::anyhow!("{error}"))?;
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
        query: cmd.query,
        ast,
        items,
    };
    let markdown = render_query_markdown(&output);
    emit(cmd.format, &output, markdown, cmd.output_file.as_deref())
}

fn run_serve(cmd: ServeCommand) -> Result<()> {
    let runtime = RuntimeContext::new(&cmd.project)?;
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
            if format == args::OutputFormat::Json
                && let Some(ambiguous) = error.downcast_ref::<AmbiguousTargetError>()
            {
                let output = build_ambiguous_target_error_output(&runtime.project_root, ambiguous);
                emit(
                    args::OutputFormat::Json,
                    &output,
                    ambiguous.message.clone(),
                    output_file,
                )?;
            }
            Err(error)
        }
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
        .map(|(index, hit)| build_numbered_search_hit_output(project_root, hit, index + 1))
        .collect::<Vec<_>>();
    let quoted_query = ambiguous.query.replace('"', "\\\"");
    let file_clause = ambiguous
        .file_filter
        .as_deref()
        .map(|file_filter| format!(" --file \"{}\"", file_filter.replace('"', "\\\"")))
        .unwrap_or_default();
    let mut next_commands = vec![format!(
        "codestory-cli symbol --query \"{}\"{} --choose 1",
        quoted_query, file_clause
    )];
    if let Some(first) = ambiguous.alternatives.first() {
        next_commands.push(format!("codestory-cli symbol --id {}", first.node_id.0));
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
                "search: inspect alternatives with `codestory-cli search --query`, then rerun this command with --choose, --id, or --file".to_string(),
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
    let retrieval = summary.retrieval.clone();
    let project = display::clean_path_string(&summary.root);
    let storage_path = display::clean_path_string(&runtime.storage_path.to_string_lossy());
    let storage_exists = runtime.storage_path.exists();
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
    if let Some(retrieval) = retrieval.as_ref() {
        checks.push(if retrieval.semantic_ready {
            doctor_check(
                "semantic",
                "ok",
                format!(
                    "Hybrid retrieval is ready with {} semantic docs.",
                    retrieval.semantic_doc_count
                ),
            )
        } else {
            doctor_check(
                "semantic",
                "info",
                retrieval.fallback_message.clone().unwrap_or_else(|| {
                    "Semantic retrieval is not ready; symbolic fallback is active.".to_string()
                }),
            )
        });
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
        "CODESTORY_EMBED_LLAMACPP_URL",
        "CODESTORY_EMBED_LLAMACPP_REQUEST_COUNT",
        "CODESTORY_STORED_VECTOR_ENCODING",
        "CODESTORY_HYBRID_RETRIEVAL_ENABLED",
        "CODESTORY_SEMANTIC_DOC_ALIAS_MODE",
    ]
    .into_iter()
    .map(|name| match std::env::var(name) {
        Ok(value) if !value.trim().is_empty() => {
            doctor_check(name, "ok", format!("set to `{}`", value.trim()))
        }
        _ => doctor_check(name, "info", "not set; using runtime defaults".to_string()),
    })
    .collect::<Vec<_>>();

    DoctorOutput {
        project: project.clone(),
        storage_path,
        indexed,
        stats: summary.stats.clone(),
        retrieval,
        freshness: summary.freshness.clone(),
        checks,
        next_commands: index_next_commands(&project, summary.retrieval.as_ref()),
        environment,
    }
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
                "Stored semantic docs match the current embedding contract (docs={}).",
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
                "{}. Resolve the embedding runtime first with `codestory-cli setup embeddings`; then run `codestory-cli index --refresh full` if semantic search should use the current config.",
                gaps.join("; ")
            ),
        )
    } else {
        doctor_check(
            "semantic_contract",
            "warn",
            format!(
                "{}. Run `codestory-cli index --refresh full` if semantic search should use the current config.",
                gaps.join("; ")
            ),
        )
    }
}

fn managed_doctor_status(state: &str) -> &'static str {
    match state {
        "managed_server_running" | "external_llama_configured" | "disabled_by_config" => "ok",
        "missing_managed_assets" => "info",
        "managed_server_stopped" | "external_llama_unreachable" => "warn",
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

fn index_next_commands(
    project: &str,
    retrieval: Option<&codestory_contracts::api::RetrievalStateDto>,
) -> Vec<String> {
    let project = display::clean_path_string(project);
    let mut commands = vec![
        format!("codestory-cli ground --project \"{project}\""),
        format!(
            "codestory-cli search --project \"{project}\" --query \"<symbol or question>\" --why"
        ),
        format!("codestory-cli ask --project \"{project}\" \"How does this repo fit together?\""),
    ];
    if retrieval.is_some_and(|state| !state.semantic_ready) {
        if retrieval.is_some_and(|state| {
            state.fallback_reason == Some(RetrievalFallbackReasonDto::MissingEmbeddingRuntime)
        }) {
            commands.push(format!(
                "codestory-cli setup embeddings --project \"{project}\""
            ));
        }
        commands.push(format!(
            "codestory-cli doctor --project \"{project}\" --format markdown"
        ));
    }
    commands
}

fn write_ask_bundle(
    bundle_dir: &std::path::Path,
    answer: &codestory_contracts::api::AgentAnswerDto,
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
    let full_answer_json =
        serde_json::to_string_pretty(answer).context("Failed to serialize ask answer JSON")?;
    let mut answer_json = if markdown.len().saturating_add(full_answer_json.len())
        > ASK_BUNDLE_OUTPUT_BYTE_CAP
    {
        notes.push(
            "answer.json was reduced to a valid manifest summary because the full answer exceeded the bundle byte cap."
                .to_string(),
        );
        ask_bundle_summary_json(answer, true)?
    } else {
        full_answer_json
    };
    if answer_json.len() > ASK_BUNDLE_OUTPUT_BYTE_CAP {
        notes.push(
            "answer.json retrieval trace was omitted because the summary still exceeded the bundle byte cap."
                .to_string(),
        );
        answer_json = ask_bundle_summary_json(answer, false)?;
    }

    let mut markdown = if markdown.len() > ASK_BUNDLE_MARKDOWN_SOFT_CAP {
        notes.push(format!(
            "answer.md was truncated to {} bytes before writing.",
            ASK_BUNDLE_MARKDOWN_SOFT_CAP
        ));
        truncate_utf8_with_suffix(
            markdown,
            ASK_BUNDLE_MARKDOWN_SOFT_CAP,
            ASK_BUNDLE_TRUNCATION_SUFFIX,
        )
    } else {
        markdown.to_string()
    };
    let remaining_markdown_bytes = ASK_BUNDLE_OUTPUT_BYTE_CAP.saturating_sub(answer_json.len());
    if markdown.len() > remaining_markdown_bytes {
        notes.push(format!(
            "answer.md was truncated to fit the remaining {} bundle bytes.",
            remaining_markdown_bytes
        ));
        markdown = truncate_utf8_with_suffix(
            &markdown,
            remaining_markdown_bytes,
            ASK_BUNDLE_TRUNCATION_SUFFIX,
        );
    }
    fs::write(bundle_dir.join("answer.md"), &markdown).with_context(|| {
        format!(
            "Failed to write {}",
            display::clean_path_string(&bundle_dir.join("answer.md").to_string_lossy())
        )
    })?;
    fs::write(bundle_dir.join("answer.json"), &answer_json).with_context(|| {
        format!(
            "Failed to write {}",
            display::clean_path_string(&bundle_dir.join("answer.json").to_string_lossy())
        )
    })?;
    let mut written_bytes = markdown.len().saturating_add(answer_json.len());
    for graph in &answer.graphs {
        if let GraphArtifactDto::Mermaid {
            id, mermaid_syntax, ..
        } = graph
        {
            let file_name = format!("{}.mmd", sanitize_artifact_name(id));
            let artifact_path = bundle_dir.join(&file_name);
            if written_bytes.saturating_add(mermaid_syntax.len()) > ASK_BUNDLE_OUTPUT_BYTE_CAP {
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
        "output_byte_cap": ASK_BUNDLE_OUTPUT_BYTE_CAP,
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

fn ask_bundle_summary_json(
    answer: &codestory_contracts::api::AgentAnswerDto,
    include_trace: bool,
) -> Result<String> {
    if include_trace {
        serde_json::to_string_pretty(&serde_json::json!({
            "truncated": true,
            "reason": "ask bundle output hit its byte cap",
            "action": "Narrow focus, reduce trail depth, or use JSON output without --bundle for the full in-memory response.",
            "answer_id": &answer.answer_id,
            "prompt": &answer.prompt,
            "summary": &answer.summary,
            "retrieval_version": &answer.retrieval_version,
            "citation_count": answer.citations.len(),
            "graph_count": answer.graphs.len(),
            "retrieval_trace": &answer.retrieval_trace,
        }))
        .context("Failed to serialize ask bundle summary JSON")
    } else {
        serde_json::to_string_pretty(&serde_json::json!({
            "truncated": true,
            "reason": "ask bundle output hit its byte cap",
            "action": "Narrow focus, reduce trail depth, or use JSON output without --bundle for the full in-memory response.",
            "answer_id": &answer.answer_id,
            "prompt": truncate_utf8_with_suffix(&answer.prompt, 4096, ASK_BUNDLE_TRUNCATION_SUFFIX),
            "summary": truncate_utf8_with_suffix(&answer.summary, 8192, ASK_BUNDLE_TRUNCATION_SUFFIX),
            "retrieval_version": &answer.retrieval_version,
            "citation_count": answer.citations.len(),
            "graph_count": answer.graphs.len(),
            "retrieval_trace_omitted": true,
        }))
        .context("Failed to serialize minimal ask bundle summary JSON")
    }
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

fn build_search_output(
    project_root: &std::path::Path,
    query: &str,
    retrieval: &codestory_contracts::api::RetrievalStateDto,
    freshness: Option<&IndexFreshnessDto>,
    symbol_hits: &[SearchHit],
    repo_text_hits: &[SearchHit],
    repo_text_stats: Option<&RepoTextScanStatsDto>,
    suggestions: &[SearchHit],
    limit_per_source: u32,
    repo_text: RepoTextOutputConfig,
    explain: bool,
) -> SearchOutput {
    let indexed_symbol_hits = symbol_hits
        .iter()
        .map(|hit| build_search_hit_output(project_root, hit, explain))
        .collect::<Vec<_>>();
    let mut duplicate_index = HashMap::new();
    for hit in &indexed_symbol_hits {
        if let Some(key) = search_hit_location_key(hit) {
            duplicate_index
                .entry(key)
                .or_insert_with(|| hit.node_id.clone());
        }
    }
    let repo_text_hits = repo_text_hits
        .iter()
        .map(|hit| {
            let mut output = build_search_hit_output(project_root, hit, explain);
            if let Some(key) = search_hit_location_key(&output) {
                output.duplicate_of = duplicate_index.get(&key).cloned();
            }
            output
        })
        .collect::<Vec<_>>();
    let query_hints = search_query_hints(query, &indexed_symbol_hits, &repo_text_hits);

    SearchOutput {
        query: query.to_string(),
        retrieval: retrieval.clone(),
        freshness: freshness.cloned(),
        limit_per_source,
        repo_text_mode: repo_text.mode,
        repo_text_enabled: repo_text.enabled,
        explain,
        query_hints,
        suggestions: suggestions
            .iter()
            .map(|hit| build_search_hit_output(project_root, hit, explain))
            .collect(),
        indexed_symbol_hits,
        repo_text_hits,
        repo_text_stats: repo_text_stats.cloned(),
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
        resolved: build_search_hit_output(project_root, &target.selected, false),
        alternatives: target
            .alternatives
            .iter()
            .skip(1)
            .map(|hit| build_search_hit_output(project_root, hit, false))
            .collect(),
    }
}

fn build_search_hit_output(
    project_root: &std::path::Path,
    hit: &SearchHit,
    explain: bool,
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
        resolvable: hit.resolvable,
        score_breakdown,
        duplicate_of: None,
        excerpt: repo_text_excerpt(project_root, hit),
        why,
    }
}

fn build_numbered_search_hit_output(
    project_root: &std::path::Path,
    hit: &SearchHit,
    number: usize,
) -> SearchHitOutput {
    let mut output = build_search_hit_output(project_root, hit, false);
    output.number = Some(number);
    output
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
            "matched repository text directly; this hit is evidence but not a resolvable symbol"
                .to_string(),
        ),
        None => why.push(format!(
            "ranked by symbolic score {:.3} with origin {}",
            hit.score,
            hit.origin.as_str()
        )),
    }
    if hit.resolvable {
        why.push(
            "can be passed to symbol, trail, snippet, explore, or ask as a focus id".to_string(),
        );
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
        .filter(|edge| !is_speculative_certainty(edge.certainty.as_deref()))
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
            !is_speculative_certainty(edge.certainty.as_deref())
                && reachable.contains(&edge.source)
                && reachable.contains(&edge.target)
        });
    }

    context
}

#[cfg(test)]
fn is_speculative_certainty(certainty: Option<&str>) -> bool {
    matches!(
        certainty.map(|value| value.to_ascii_lowercase()).as_deref(),
        Some("uncertain" | "speculative")
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
        AgentAnswerDto, AgentRetrievalPolicyModeDto, AgentRetrievalPresetDto,
        AgentRetrievalTraceDto, EdgeId, EdgeKind, GraphEdgeDto, GraphNodeDto, GraphResponse,
        IndexMode, IndexingPhaseTimings, NodeDetailsDto, NodeId, ProjectSummary, RetrievalModeDto,
        RetrievalStateDto, SearchHit, StorageStatsDto, TrailContextDto,
    };
    use std::fs;
    use std::path::{Path, PathBuf};
    use tempfile::tempdir;

    fn sample_retrieval() -> RetrievalStateDto {
        RetrievalStateDto {
            mode: RetrievalModeDto::Hybrid,
            hybrid_configured: true,
            semantic_ready: true,
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
            answer_id: "answer-test".to_string(),
            prompt: "Explain the capped bundle".to_string(),
            summary: "Bundle summary".to_string(),
            freshness: None,
            sections: Vec::new(),
            citations: Vec::new(),
            subgraph_ids: vec!["big-mermaid".to_string()],
            retrieval_version: "test".to_string(),
            graphs: vec![graph],
            retrieval_trace: AgentRetrievalTraceDto {
                request_id: "answer-test".to_string(),
                resolved_profile: AgentRetrievalPresetDto::Architecture,
                policy_mode: AgentRetrievalPolicyModeDto::LatencyFirst,
                total_latency_ms: 1,
                sla_target_ms: None,
                sla_missed: false,
                annotations: Vec::new(),
                steps: Vec::new(),
            },
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
            semantic_doc_build_ms: Some(7),
            semantic_embedding_ms: Some(8),
            semantic_db_upsert_ms: Some(9),
            semantic_reload_ms: Some(10),
            semantic_docs_reused: Some(11),
            semantic_docs_embedded: Some(12),
            semantic_docs_pending: Some(13),
            semantic_docs_stale: Some(14),
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
        GraphEdgeDto {
            id: EdgeId(id.to_string()),
            source: NodeId(source.to_string()),
            target: NodeId(target.to_string()),
            kind: EdgeKind::CALL,
            confidence: None,
            certainty: certainty.map(ToOwned::to_owned),
            callsite_identity: None,
            candidate_targets: Vec::new(),
        }
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
            next_commands: Vec::new(),
        };

        let markdown = render_index_markdown(&output);

        assert!(markdown.contains("semantic_ms: doc_build=7 embedding=8 db_upsert=9 reload=10"));
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
            resolvable: true,
            score_breakdown: None,
        }];
        let repo_text_hits = vec![SearchHit {
            node_id: NodeId("repo-text".to_string()),
            display_name: "README.md".to_string(),
            kind: codestory_contracts::api::NodeKind::FILE,
            file_path: Some("README.md".to_string()),
            line: Some(3),
            score: 500.0,
            origin: codestory_contracts::api::SearchHitOrigin::TextMatch,
            resolvable: false,
            score_breakdown: None,
        }];

        let output = build_search_output(
            root,
            "needle",
            &sample_retrieval(),
            None,
            &symbol_hits,
            &repo_text_hits,
            None,
            &[],
            5,
            RepoTextOutputConfig {
                mode: RepoTextMode::Auto,
                enabled: true,
            },
            false,
        );

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
    fn write_ask_bundle_caps_disk_artifacts_and_writes_manifest() {
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
                "x".repeat(ASK_BUNDLE_OUTPUT_BYTE_CAP + 1024)
            ),
        });

        write_ask_bundle(temp.path(), &answer, "short answer").expect("write capped bundle");

        let manifest: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(temp.path().join("bundle_manifest.json"))
                .expect("read bundle manifest"),
        )
        .expect("parse bundle manifest");
        let answer_json: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(temp.path().join("answer.json")).expect("read answer json"),
        )
        .expect("parse answer json");

        assert_eq!(manifest["truncated"], serde_json::Value::Bool(true));
        assert_eq!(
            manifest["omitted_mermaid_artifacts"].as_u64(),
            Some(1),
            "{manifest}"
        );
        assert!(
            manifest["written_bytes_excluding_manifest"]
                .as_u64()
                .is_some_and(|bytes| bytes <= ASK_BUNDLE_OUTPUT_BYTE_CAP as u64),
            "{manifest}"
        );
        assert_eq!(answer_json["truncated"], serde_json::Value::Bool(true));
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
            resolvable: true,
            score_breakdown: None,
        }];

        let output = build_search_output(
            root,
            "ResolutionPass",
            &sample_retrieval(),
            None,
            &symbol_hits,
            &[],
            None,
            &[],
            5,
            RepoTextOutputConfig {
                mode: RepoTextMode::Auto,
                enabled: false,
            },
            false,
        );

        assert_eq!(
            output.indexed_symbol_hits[0].node_ref.as_deref(),
            Some("src/resolution/mod.rs:42:ResolutionPass")
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
            resolvable: true,
            score_breakdown: None,
        }];
        let repo_text_hits = vec![SearchHit {
            node_id: NodeId("text-1".to_string()),
            display_name: "src/lib.rs".to_string(),
            kind: codestory_contracts::api::NodeKind::FILE,
            file_path: Some("C:/repo/src/lib.rs".to_string()),
            line: Some(7),
            score: 500.0,
            origin: codestory_contracts::api::SearchHitOrigin::TextMatch,
            resolvable: false,
            score_breakdown: None,
        }];

        let output = build_search_output(
            root,
            "snapshot digest",
            &sample_retrieval(),
            None,
            &symbol_hits,
            &repo_text_hits,
            None,
            &[],
            5,
            RepoTextOutputConfig {
                mode: RepoTextMode::Auto,
                enabled: true,
            },
            false,
        );

        assert_eq!(
            output.repo_text_hits[0].duplicate_of.as_deref(),
            Some("symbol-1")
        );
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
                "query",
                "search(query: 'Foo') | limit(1)",
                "--output-file",
                "out.md",
            ],
            vec![
                "codestory-cli",
                "ask",
                "How does this work?",
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
            resolvable: true,
            score_breakdown: Some(codestory_contracts::api::RetrievalScoreBreakdownDto {
                lexical: 0.7,
                semantic: 0.2,
                graph: 0.1,
                total: 0.9,
            }),
        }];

        let output = build_search_output(
            root,
            "ranked",
            &sample_retrieval(),
            None,
            &symbol_hits,
            &[],
            None,
            &[],
            5,
            RepoTextOutputConfig {
                mode: RepoTextMode::Off,
                enabled: false,
            },
            true,
        );

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
    fn stdio_metadata_lists_tools_resources_and_prompts() {
        let tools = stdio_tools_list_json();
        let tool_names = tools["result"]["tools"]
            .as_array()
            .expect("tools")
            .iter()
            .filter_map(|tool| tool["name"].as_str())
            .collect::<Vec<_>>();
        assert!(tool_names.contains(&"ask"));
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
                resolvable: true,
                score_breakdown: None,
            },
            SearchHit {
                node_id: NodeId("1".to_string()),
                display_name: "AppController".to_string(),
                kind: codestory_contracts::api::NodeKind::CLASS,
                file_path: None,
                line: None,
                score: 0.9,
                origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
                resolvable: true,
                score_breakdown: None,
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
                resolvable: true,
                score_breakdown: None,
            },
            SearchHit {
                node_id: NodeId("1".to_string()),
                display_name: "AppController".to_string(),
                kind: codestory_contracts::api::NodeKind::STRUCT,
                file_path: Some(file_path.to_string_lossy().to_string()),
                line: Some(1),
                score: 1.0,
                origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
                resolvable: true,
                score_breakdown: None,
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
                resolvable: true,
                score_breakdown: None,
            },
            SearchHit {
                node_id: NodeId("1".to_string()),
                display_name: "check_winner".to_string(),
                kind: codestory_contracts::api::NodeKind::FUNCTION,
                file_path: Some("src/game.rs".to_string()),
                line: Some(10),
                score: 0.8,
                origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
                resolvable: true,
                score_breakdown: None,
            },
        ];

        hits.sort_by(|left, right| compare_resolution_hits(query, left, right));
        assert_eq!(hits[0].kind, codestory_contracts::api::NodeKind::FUNCTION);
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
