use anyhow::{Context, Result, bail};
use clap::{CommandFactory, Parser};
use clap_complete::{Shell, generate};
use codestory_contracts::api::{
    AgentAskRequest, AgentConnectionSettingsDto, AgentHybridWeightsDto, AgentResponseModeDto,
    AppEventPayload, GraphArtifactDto, GroundingBudgetDto, IndexMode, LayoutDirection,
    ListChildrenSymbolsRequest, ListRootSymbolsRequest, NodeId, RetrievalScoreBreakdownDto,
    SearchHit, SearchHybridLimitsDto, SearchRepoTextMode, SearchRequest, SnippetContextDto,
    SymbolContextDto, TrailCallerScope, TrailConfigDto, TrailContextDto, TrailDirection, TrailMode,
};
use codestory_contracts::query::GraphQueryOperation;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    fs,
    io::{BufRead, IsTerminal, Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

mod args;
mod config;
mod display;
mod output;
mod query_resolution;
mod runtime;

use args::{
    AskCommand, Cli, Command, CompletionShell, DoctorCheckOutput, DoctorCommand, DoctorOutput,
    ExploreCommand, ExploreOutput, GenerateCompletionsCommand, GroundCommand, IndexCommand,
    IndexDryRunOutput, IndexOutput, NavigationOutput, QueryCommand, QueryItemOutput, QueryOutput,
    QueryResolutionOutput, RepoTextMode, SearchCommand, SearchHitOutput, SearchOutput,
    ServeCommand, SnippetCommand, SnippetJsonOutput, SymbolCommand, SymbolJsonOutput, TrailCommand,
    TrailJsonOutput, build_trail_request,
};
use output::{
    emit, emit_text, render_agent_answer_markdown, render_doctor_markdown, render_ground_markdown,
    render_index_dry_run_markdown, render_index_markdown, render_query_markdown,
    render_search_markdown, render_snippet_markdown, render_symbol_markdown, render_symbol_mermaid,
    render_trail_dot, render_trail_markdown, render_trail_mermaid,
};
use runtime::{
    RuntimeContext, ensure_index_ready, map_api_error, refresh_label, resolve_refresh_request,
    resolve_target,
};

#[derive(Debug, Clone, Copy)]
struct RepoTextOutputConfig {
    mode: RepoTextMode,
    enabled: bool,
}

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
        Command::Search(cmd) => run_search(cmd),
        Command::Symbol(cmd) => run_symbol(cmd),
        Command::Trail(cmd) => run_trail(cmd),
        Command::Snippet(cmd) => run_snippet(cmd),
        Command::Query(cmd) => run_query(cmd),
        Command::Explore(cmd) => run_explore(cmd),
        Command::Serve(cmd) => run_serve(cmd),
        Command::GenerateCompletions(cmd) => run_generate_completions(cmd),
    }
}

fn run_index(cmd: IndexCommand) -> Result<()> {
    ensure_dot_only_for_trail(cmd.format, "index")?;
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
                Ok(AppEventPayload::IndexingProgress { current, total }) => {
                    eprintln!(
                        "[{current}/{total}] {} indexing",
                        format_progress_bar(current, total)
                    );
                }
                Ok(AppEventPayload::IndexingStarted { file_count }) => {
                    eprintln!(
                        "[0/{file_count}] {} indexing started",
                        format_progress_bar(0, file_count)
                    );
                }
                Ok(_) => {}
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
            }
        }
    });
    ProgressPrinter { done, handle }
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
    let runtime = RuntimeContext::new(&cmd.project)?;
    let opened = runtime.ensure_open(cmd.refresh)?;
    ensure_index_ready(&opened, "ask")?;

    let request = AgentAskRequest {
        prompt: cmd.prompt.clone(),
        retrieval_profile: cmd.profile.into(),
        focus_node_id: cmd
            .focus_id
            .as_ref()
            .map(|id| NodeId(id.trim().to_string())),
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

    let answer = runtime.agent.ask(request).map_err(map_api_error)?;
    let markdown = render_agent_answer_markdown(&runtime.project_root, &answer);
    if let Some(bundle_dir) = cmd.bundle.as_deref() {
        write_ask_bundle(bundle_dir, &answer, &markdown)?;
    }
    emit(cmd.format, &answer, markdown, cmd.output_file.as_deref())
}

fn run_doctor(cmd: DoctorCommand) -> Result<()> {
    ensure_dot_only_for_trail(cmd.format, "doctor")?;
    let runtime = RuntimeContext::new(&cmd.project)?;
    let summary = runtime.open_project_summary()?;
    let output = build_doctor_output(&runtime, &summary);
    let markdown = render_doctor_markdown(&output);
    emit(cmd.format, &output, markdown, cmd.output_file.as_deref())
}

fn run_search(cmd: SearchCommand) -> Result<()> {
    ensure_dot_only_for_trail(cmd.format, "search")?;
    let runtime = RuntimeContext::new(&cmd.project)?;
    let opened = runtime.ensure_open(cmd.refresh)?;
    ensure_index_ready(&opened, "search")?;
    let hybrid_weights = hybrid_weights(cmd.hybrid_lexical, cmd.hybrid_semantic, cmd.hybrid_graph);
    let hybrid_limits = hybrid_limits(cmd.hybrid_lexical_limit, cmd.hybrid_semantic_limit);

    let search_results = runtime
        .search
        .search_results(SearchRequest {
            query: cmd.query.clone(),
            repo_text: to_api_repo_text_mode(cmd.repo_text),
            limit_per_source: cmd.limit.clamp(1, 50),
            hybrid_weights,
            hybrid_limits,
        })
        .map_err(map_api_error)?;
    let output = build_search_output(
        &runtime.project_root,
        &search_results.query,
        &search_results.retrieval,
        &search_results.indexed_symbol_hits,
        &search_results.repo_text_hits,
        &search_results.suggestions,
        search_results.limit_per_source,
        RepoTextOutputConfig {
            mode: from_api_repo_text_mode(search_results.repo_text_mode),
            enabled: search_results.repo_text_enabled,
        },
        cmd.why,
    );
    let markdown = render_search_markdown(&runtime.project_root, &output);
    emit(cmd.format, &output, markdown, cmd.output_file.as_deref())
}

fn run_symbol(cmd: SymbolCommand) -> Result<()> {
    ensure_dot_only_for_trail(cmd.format, "symbol")?;
    let runtime = RuntimeContext::new(&cmd.project)?;
    let opened = runtime.ensure_open(cmd.refresh)?;
    ensure_index_ready(&opened, "symbol")?;

    let file_filter = cmd.target.file_filter();
    let target = resolve_target(&runtime, cmd.target.selection()?, file_filter.as_deref())?;
    let context = runtime
        .grounding
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
    let runtime = RuntimeContext::new(&cmd.project)?;
    let opened = runtime.ensure_open(cmd.refresh)?;
    ensure_index_ready(&opened, "trail")?;

    let file_filter = cmd.target.file_filter();
    let target = resolve_target(&runtime, cmd.target.selection()?, file_filter.as_deref())?;
    let request = build_trail_request(&target.selected.node_id, &cmd);
    let mut context = runtime
        .grounding
        .trail_context(request)
        .map_err(map_api_error)?;
    if cmd.hide_speculative {
        context = hide_speculative_trail_edges(context);
    }
    if cmd.mermaid {
        return emit_text(render_trail_mermaid(&context), cmd.output_file.as_deref());
    }
    if cmd.format == args::OutputFormat::Dot {
        return emit_text(
            render_trail_dot(&runtime.project_root, &context),
            cmd.output_file.as_deref(),
        );
    }
    let markdown = render_trail_markdown(&runtime.project_root, &target, &context, &cmd);
    let output = TrailJsonOutput {
        resolution: build_query_resolution_output(&runtime.project_root, &target),
        trail: &context,
    };
    emit(cmd.format, &output, markdown, cmd.output_file.as_deref())
}

fn run_snippet(cmd: SnippetCommand) -> Result<()> {
    ensure_dot_only_for_trail(cmd.format, "snippet")?;
    let runtime = RuntimeContext::new(&cmd.project)?;
    let opened = runtime.ensure_open(cmd.refresh)?;
    ensure_index_ready(&opened, "snippet")?;

    let file_filter = cmd.target.file_filter();
    let target = resolve_target(&runtime, cmd.target.selection()?, file_filter.as_deref())?;
    let context = runtime
        .grounding
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
    let ast = codestory_runtime::parse_graph_query(&cmd.query)
        .map_err(|error| anyhow::anyhow!("{error}"))?;
    let runtime = RuntimeContext::new(&cmd.project)?;
    let opened = runtime.ensure_open(cmd.refresh)?;
    ensure_index_ready(&opened, "query")?;
    let mut items = Vec::<QueryItemOutput>::new();
    for op in &ast.operations {
        match op {
            GraphQueryOperation::Trail(query) => {
                let target = resolve_target(
                    &runtime,
                    args::TargetSelection::Query(query.symbol.clone()),
                    None,
                )?;
                let mut request = TrailConfigDto {
                    root_id: target.selected.node_id.clone(),
                    mode: TrailMode::Neighborhood,
                    target_id: None,
                    depth: query.depth.unwrap_or(2),
                    direction: query.direction.unwrap_or(TrailDirection::Both),
                    caller_scope: TrailCallerScope::ProductionOnly,
                    edge_filter: Vec::new(),
                    show_utility_calls: false,
                    node_filter: Vec::new(),
                    max_nodes: 120,
                    layout_direction: LayoutDirection::Horizontal,
                };
                if request.depth == 0 {
                    request.max_nodes = 200;
                }
                let context = runtime
                    .grounding
                    .trail_context(request)
                    .map_err(map_api_error)?;
                items = context
                    .trail
                    .nodes
                    .iter()
                    .map(|node| graph_node_to_query_item(&runtime.project_root, node, "trail"))
                    .collect();
            }
            GraphQueryOperation::Symbol(query) => {
                let target = resolve_target(
                    &runtime,
                    args::TargetSelection::Query(query.query.clone()),
                    None,
                )?;
                let context = runtime
                    .grounding
                    .symbol_context(target.selected.node_id.clone())
                    .map_err(map_api_error)?;
                items = std::iter::once(node_details_to_query_item(
                    &runtime.project_root,
                    &context.node,
                    Some(0),
                    "symbol",
                ))
                .chain(context.children.iter().map(|child| {
                    QueryItemOutput {
                        node_id: child.id.0.clone(),
                        node_ref: None,
                        display_name: child.label.clone(),
                        kind: child.kind,
                        file_path: child
                            .file_path
                            .as_deref()
                            .map(|path| display::relative_path(&runtime.project_root, path)),
                        line: None,
                        depth: Some(1),
                        source: "symbol_child".to_string(),
                    }
                }))
                .collect();
            }
            GraphQueryOperation::Search(query) => {
                let results = runtime
                    .search
                    .search_results(SearchRequest {
                        query: query.query.clone(),
                        repo_text: SearchRepoTextMode::Off,
                        limit_per_source: 50,
                        hybrid_weights: None,
                        hybrid_limits: None,
                    })
                    .map_err(map_api_error)?;
                items = results
                    .indexed_symbol_hits
                    .iter()
                    .map(|hit| search_hit_to_query_item(&runtime.project_root, hit, "search"))
                    .collect();
            }
            GraphQueryOperation::Filter(filter) => {
                items.retain(|item| {
                    filter.kind.is_none_or(|kind| item.kind == kind)
                        && filter
                            .depth
                            .is_none_or(|depth| item.depth.unwrap_or(0) <= depth)
                        && filter.file.as_deref().is_none_or(|needle| {
                            item.file_path
                                .as_deref()
                                .is_some_and(|path| path.contains(needle))
                        })
                });
            }
            GraphQueryOperation::Limit(limit) => {
                items.truncate(limit.count as usize);
            }
        }
    }
    let output = QueryOutput {
        query: cmd.query,
        ast,
        items,
    };
    let markdown = render_query_markdown(&output);
    emit(cmd.format, &output, markdown, cmd.output_file.as_deref())
}

fn run_explore(cmd: ExploreCommand) -> Result<()> {
    ensure_dot_only_for_trail(cmd.format, "explore")?;
    let runtime = RuntimeContext::new(&cmd.project)?;
    let opened = runtime.ensure_open(cmd.refresh)?;
    ensure_index_ready(&opened, "explore")?;
    let file_filter = cmd.target.file_filter();
    let target = resolve_target(&runtime, cmd.target.selection()?, file_filter.as_deref())?;
    let symbol = runtime
        .grounding
        .symbol_context(target.selected.node_id.clone())
        .map_err(map_api_error)?;
    let trail = runtime
        .grounding
        .trail_context(TrailConfigDto {
            root_id: target.selected.node_id.clone(),
            mode: TrailMode::Neighborhood,
            target_id: None,
            depth: cmd.depth,
            direction: TrailDirection::Both,
            caller_scope: TrailCallerScope::ProductionOnly,
            edge_filter: Vec::new(),
            show_utility_calls: false,
            node_filter: Vec::new(),
            max_nodes: cmd.max_nodes.clamp(1, 120),
            layout_direction: LayoutDirection::Horizontal,
        })
        .map_err(map_api_error)?;
    let snippet = runtime
        .grounding
        .snippet_context(target.selected.node_id.clone(), 4)
        .ok();
    let output = ExploreOutput {
        resolution: build_query_resolution_output(&runtime.project_root, &target),
        navigation: build_navigation_output(&runtime.project_root, &target, &trail),
        symbol: &symbol,
        trail: &trail,
        snippet: snippet.as_ref(),
    };
    let markdown = render_explore_markdown(
        &runtime.project_root,
        &target,
        &symbol,
        &trail,
        snippet.as_ref(),
    );
    if cmd.format == args::OutputFormat::Markdown
        && cmd.output_file.is_none()
        && !cmd.no_tui
        && std::io::stdout().is_terminal()
    {
        return run_explore_tui(
            &runtime.project_root,
            &target,
            &symbol,
            &trail,
            snippet.as_ref(),
        );
    }
    emit(cmd.format, &output, markdown, cmd.output_file.as_deref())
}

fn run_serve(cmd: ServeCommand) -> Result<()> {
    let runtime = RuntimeContext::new(&cmd.project)?;
    let opened = runtime.ensure_open(cmd.refresh)?;
    ensure_index_ready(&opened, "serve")?;
    if cmd.stdio {
        return run_stdio_server(runtime);
    }
    let listener = TcpListener::bind(&cmd.addr)
        .with_context(|| format!("Failed to bind server to {}", cmd.addr))?;
    eprintln!("codestory serve listening on http://{}", cmd.addr);
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                if let Err(error) = handle_http_request(&runtime, stream) {
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
    }

    let environment = [
        "CODESTORY_EMBED_PROFILE",
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
        checks,
        next_commands: index_next_commands(&project, summary.retrieval.as_ref()),
        environment,
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
    fs::write(bundle_dir.join("answer.md"), markdown).with_context(|| {
        format!(
            "Failed to write {}",
            display::clean_path_string(&bundle_dir.join("answer.md").to_string_lossy())
        )
    })?;
    fs::write(
        bundle_dir.join("answer.json"),
        serde_json::to_string_pretty(answer).context("Failed to serialize ask answer JSON")?,
    )
    .with_context(|| {
        format!(
            "Failed to write {}",
            display::clean_path_string(&bundle_dir.join("answer.json").to_string_lossy())
        )
    })?;
    for graph in &answer.graphs {
        if let GraphArtifactDto::Mermaid {
            id, mermaid_syntax, ..
        } = graph
        {
            let file_name = format!("{}.mmd", sanitize_artifact_name(id));
            fs::write(bundle_dir.join(file_name), mermaid_syntax)?;
        }
    }
    Ok(())
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

fn graph_node_to_query_item(
    project_root: &std::path::Path,
    node: &codestory_contracts::api::GraphNodeDto,
    source: &str,
) -> QueryItemOutput {
    let file_path = node
        .file_path
        .as_deref()
        .map(|path| display::relative_path(project_root, path));
    QueryItemOutput {
        node_id: node.id.0.clone(),
        node_ref: None,
        display_name: node.label.clone(),
        kind: node.kind,
        file_path,
        line: None,
        depth: Some(node.depth),
        source: source.to_string(),
    }
}

fn search_hit_to_query_item(
    project_root: &std::path::Path,
    hit: &SearchHit,
    source: &str,
) -> QueryItemOutput {
    QueryItemOutput {
        node_id: hit.node_id.0.clone(),
        node_ref: output::node_ref(
            project_root,
            hit.file_path.as_deref(),
            hit.line,
            &hit.display_name,
        ),
        display_name: hit.display_name.clone(),
        kind: hit.kind,
        file_path: hit
            .file_path
            .as_deref()
            .map(|path| display::relative_path(project_root, path)),
        line: hit.line,
        depth: None,
        source: source.to_string(),
    }
}

fn build_navigation_output(
    project_root: &std::path::Path,
    target: &runtime::ResolvedTarget,
    trail: &TrailContextDto,
) -> NavigationOutput {
    let center = &target.selected.node_id;
    let nodes = trail
        .trail
        .nodes
        .iter()
        .map(|node| (node.id.clone(), node))
        .collect::<HashMap<_, _>>();
    let mut incoming_seen = HashSet::new();
    let mut outgoing_seen = HashSet::new();
    let mut incoming_references = Vec::new();
    let mut outgoing_references = Vec::new();

    for edge in &trail.trail.edges {
        if &edge.target == center
            && incoming_seen.insert(edge.source.clone())
            && let Some(node) = nodes.get(&edge.source)
        {
            incoming_references.push(graph_node_to_query_item(
                project_root,
                node,
                "incoming_reference",
            ));
        }
        if &edge.source == center
            && outgoing_seen.insert(edge.target.clone())
            && let Some(node) = nodes.get(&edge.target)
        {
            outgoing_references.push(graph_node_to_query_item(
                project_root,
                node,
                "outgoing_reference",
            ));
        }
    }

    NavigationOutput {
        definition: build_search_hit_output(project_root, &target.selected, false),
        incoming_references,
        outgoing_references,
    }
}

fn node_details_to_query_item(
    project_root: &std::path::Path,
    node: &codestory_contracts::api::NodeDetailsDto,
    depth: Option<u32>,
    source: &str,
) -> QueryItemOutput {
    QueryItemOutput {
        node_id: node.id.0.clone(),
        node_ref: output::node_ref(
            project_root,
            node.file_path.as_deref(),
            node.start_line,
            &node.display_name,
        ),
        display_name: node.display_name.clone(),
        kind: node.kind,
        file_path: node
            .file_path
            .as_deref()
            .map(|path| display::relative_path(project_root, path)),
        line: node.start_line,
        depth,
        source: source.to_string(),
    }
}

fn render_explore_markdown(
    project_root: &std::path::Path,
    target: &runtime::ResolvedTarget,
    symbol: &SymbolContextDto,
    trail: &TrailContextDto,
    snippet: Option<&SnippetContextDto>,
) -> String {
    let mut markdown = String::new();
    markdown.push_str("# Explore\n");
    markdown.push_str("resolution:\n");
    markdown.push_str(&format!(
        "- {}\n",
        output::node_ref(
            project_root,
            target.selected.file_path.as_deref(),
            target.selected.line,
            &target.selected.display_name
        )
        .unwrap_or_else(|| target.selected.display_name.clone())
    ));
    let navigation = build_navigation_output(project_root, target, trail);
    markdown.push_str("navigation:\n");
    if let Some(node_ref) = navigation.definition.node_ref.as_deref() {
        markdown.push_str(&format!("- definition: `{node_ref}`\n"));
    } else {
        markdown.push_str(&format!(
            "- definition: {}\n",
            navigation.definition.display_name
        ));
    }
    markdown.push_str(&format!(
        "- incoming_references: {}\n",
        navigation.incoming_references.len()
    ));
    markdown.push_str(&format!(
        "- outgoing_references: {}\n",
        navigation.outgoing_references.len()
    ));
    markdown.push_str("symbol:\n");
    markdown.push_str(&render_symbol_markdown(project_root, target, symbol));
    markdown.push_str("\ntrail:\n");
    let cmd = TrailCommand {
        project: args::ProjectArgs {
            project: project_root.to_path_buf(),
            cache_dir: None,
        },
        target: args::TargetArgs {
            id: Some(target.selected.node_id.0.clone()),
            query: None,
            file: None,
        },
        mode: args::CliTrailMode::Neighborhood,
        depth: Some(2),
        direction: Some(args::CliDirection::Both),
        max_nodes: trail.trail.nodes.len().min(u32::MAX as usize) as u32,
        include_tests: false,
        show_utility_calls: false,
        hide_speculative: false,
        layout: args::CliLayout::Horizontal,
        refresh: args::RefreshMode::None,
        format: args::OutputFormat::Markdown,
        output_file: None,
        mermaid: false,
    };
    markdown.push_str(&render_trail_markdown(project_root, target, trail, &cmd));
    if let Some(snippet) = snippet {
        markdown.push_str("\nsnippet:\n");
        markdown.push_str(&render_snippet_markdown(
            project_root,
            target,
            snippet,
            false,
        ));
    }
    markdown
}

struct TerminalCleanup;

impl Drop for TerminalCleanup {
    fn drop(&mut self) {
        let _ = crossterm::terminal::disable_raw_mode();
        let _ = crossterm::execute!(std::io::stdout(), crossterm::terminal::LeaveAlternateScreen);
    }
}

fn run_explore_tui(
    project_root: &std::path::Path,
    target: &runtime::ResolvedTarget,
    symbol: &SymbolContextDto,
    trail: &TrailContextDto,
    snippet: Option<&SnippetContextDto>,
) -> Result<()> {
    use crossterm::{
        event::{self, Event, KeyCode, KeyModifiers},
        terminal::{EnterAlternateScreen, enable_raw_mode},
    };
    use ratatui::{
        Terminal,
        backend::CrosstermBackend,
        layout::{Constraint, Direction, Layout},
        style::{Color, Modifier, Style},
        text::{Line, Span},
        widgets::{Block, Borders, List, ListItem, Paragraph, Wrap},
    };

    let panes = [
        (
            "Symbol",
            render_symbol_markdown(project_root, target, symbol),
        ),
        ("Trail", {
            let cmd = TrailCommand {
                project: args::ProjectArgs {
                    project: project_root.to_path_buf(),
                    cache_dir: None,
                },
                target: args::TargetArgs {
                    id: Some(target.selected.node_id.0.clone()),
                    query: None,
                    file: None,
                },
                mode: args::CliTrailMode::Neighborhood,
                depth: Some(2),
                direction: Some(args::CliDirection::Both),
                max_nodes: trail.trail.nodes.len().min(u32::MAX as usize) as u32,
                include_tests: false,
                show_utility_calls: false,
                hide_speculative: false,
                layout: args::CliLayout::Horizontal,
                refresh: args::RefreshMode::None,
                format: args::OutputFormat::Markdown,
                output_file: None,
                mermaid: false,
            };
            render_trail_markdown(project_root, target, trail, &cmd)
        }),
        (
            "Snippet",
            snippet
                .map(|context| render_snippet_markdown(project_root, target, context, false))
                .unwrap_or_else(|| "No snippet available for this symbol.".to_string()),
        ),
    ];

    enable_raw_mode()?;
    crossterm::execute!(std::io::stdout(), EnterAlternateScreen)?;
    let _cleanup = TerminalCleanup;
    let backend = CrosstermBackend::new(std::io::stdout());
    let mut terminal = Terminal::new(backend)?;
    let mut selected = 0usize;
    let mut scroll = [0u16; 3];

    loop {
        terminal.draw(|frame| {
            let area = frame.area();
            let shell = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(3),
                    Constraint::Min(1),
                    Constraint::Length(1),
                ])
                .split(area);
            let body = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Length(24), Constraint::Min(30)])
                .split(shell[1]);

            let title = output::node_ref(
                project_root,
                target.selected.file_path.as_deref(),
                target.selected.line,
                &target.selected.display_name,
            )
            .unwrap_or_else(|| target.selected.display_name.clone());
            frame.render_widget(
                Paragraph::new(title).block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title("CodeStory Explore"),
                ),
                shell[0],
            );

            let nav_items = panes
                .iter()
                .enumerate()
                .map(|(idx, (label, _))| {
                    let style = if idx == selected {
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default()
                    };
                    ListItem::new(Line::from(Span::styled(*label, style)))
                })
                .collect::<Vec<_>>();
            frame.render_widget(
                List::new(nav_items).block(Block::default().borders(Borders::ALL).title("Panes")),
                body[0],
            );

            frame.render_widget(
                Paragraph::new(panes[selected].1.as_str())
                    .block(
                        Block::default()
                            .borders(Borders::ALL)
                            .title(panes[selected].0),
                    )
                    .wrap(Wrap { trim: false })
                    .scroll((scroll[selected], 0)),
                body[1],
            );
            frame.render_widget(
                Paragraph::new("Tab switch pane  Up/Down scroll  q quit"),
                shell[2],
            );
        })?;

        if event::poll(Duration::from_millis(250))?
            && let Event::Key(key) = event::read()?
        {
            match key.code {
                KeyCode::Esc | KeyCode::Char('q') => break,
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => break,
                KeyCode::Tab => selected = (selected + 1) % panes.len(),
                KeyCode::BackTab => selected = (selected + panes.len() - 1) % panes.len(),
                KeyCode::Up | KeyCode::Char('k') => {
                    scroll[selected] = scroll[selected].saturating_sub(1);
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    scroll[selected] = scroll[selected].saturating_add(1);
                }
                KeyCode::PageUp => {
                    scroll[selected] = scroll[selected].saturating_sub(10);
                }
                KeyCode::PageDown => {
                    scroll[selected] = scroll[selected].saturating_add(10);
                }
                KeyCode::Home => scroll[selected] = 0,
                _ => {}
            }
        }
    }
    terminal.show_cursor()?;
    Ok(())
}

fn handle_http_request(runtime: &RuntimeContext, mut stream: TcpStream) -> Result<()> {
    let mut buffer = [0u8; 8192];
    let read = stream.read(&mut buffer)?;
    let request = String::from_utf8_lossy(&buffer[..read]);
    let line = request.lines().next().unwrap_or_default();
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or_default();
    let target = parts.next().unwrap_or("/");
    if method != "GET" {
        return write_http_json(
            &mut stream,
            405,
            &serde_json::json!({"error": "method not allowed"}),
        );
    }
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let params = parse_query_string(query);
    match path {
        "/health" => write_http_json(&mut stream, 200, &serde_json::json!({"ok": true})),
        "/search" => {
            let query = params.get("q").cloned().unwrap_or_default();
            let repo_text = params
                .get("repo_text")
                .and_then(|value| search_repo_text_mode_param(value))
                .unwrap_or(SearchRepoTextMode::Auto);
            let limit_per_source = params
                .get("limit")
                .and_then(|value| value.parse::<u32>().ok())
                .unwrap_or(10)
                .clamp(1, 100);
            let results = runtime
                .search
                .search_results(SearchRequest {
                    query,
                    repo_text,
                    limit_per_source,
                    hybrid_weights: None,
                    hybrid_limits: None,
                })
                .map_err(map_api_error)?;
            write_http_json(&mut stream, 200, &results)
        }
        "/symbol" => {
            let query = params.get("q").cloned().unwrap_or_default();
            let target = resolve_target(runtime, args::TargetSelection::Query(query), None)?;
            let context = runtime
                .grounding
                .symbol_context(target.selected.node_id)
                .map_err(map_api_error)?;
            write_http_json(&mut stream, 200, &context)
        }
        "/definition" => {
            let target = resolve_target(runtime, target_selection_from_params(&params)?, None)?;
            let context = runtime
                .grounding
                .symbol_context(target.selected.node_id.clone())
                .map_err(map_api_error)?;
            write_http_json(
                &mut stream,
                200,
                &serde_json::json!({
                    "resolution": build_query_resolution_output(&runtime.project_root, &target),
                    "definition": build_search_hit_output(&runtime.project_root, &target.selected, false),
                    "symbol": context,
                }),
            )
        }
        "/references" => {
            let target = resolve_target(runtime, target_selection_from_params(&params)?, None)?;
            let depth = params
                .get("depth")
                .and_then(|value| value.parse::<u32>().ok())
                .unwrap_or(0);
            let context = runtime
                .grounding
                .trail_context(TrailConfigDto {
                    root_id: target.selected.node_id.clone(),
                    mode: TrailMode::AllReferencing,
                    target_id: None,
                    depth,
                    direction: TrailDirection::Incoming,
                    caller_scope: TrailCallerScope::IncludeTestsAndBenches,
                    edge_filter: Vec::new(),
                    show_utility_calls: false,
                    node_filter: Vec::new(),
                    max_nodes: 120,
                    layout_direction: LayoutDirection::Horizontal,
                })
                .map_err(map_api_error)?;
            write_http_json(
                &mut stream,
                200,
                &serde_json::json!({
                    "resolution": build_query_resolution_output(&runtime.project_root, &target),
                    "references": context,
                }),
            )
        }
        "/symbols" => {
            let limit = params
                .get("limit")
                .and_then(|value| value.parse::<u32>().ok())
                .map(|value| value.clamp(1, 2_000));
            if let Some(parent_id) = params.get("parent_id").filter(|value| !value.is_empty()) {
                let symbols = runtime
                    .grounding
                    .list_children_symbols(ListChildrenSymbolsRequest {
                        parent_id: NodeId(parent_id.clone()),
                    })
                    .map_err(map_api_error)?;
                write_http_json(&mut stream, 200, &symbols)
            } else {
                let symbols = runtime
                    .grounding
                    .list_root_symbols(ListRootSymbolsRequest { limit })
                    .map_err(map_api_error)?;
                write_http_json(&mut stream, 200, &symbols)
            }
        }
        "/trail" => {
            let query = params.get("q").cloned().unwrap_or_default();
            let target = resolve_target(runtime, args::TargetSelection::Query(query), None)?;
            let depth = params
                .get("depth")
                .and_then(|value| value.parse::<u32>().ok())
                .unwrap_or(2);
            let context = runtime
                .grounding
                .trail_context(TrailConfigDto {
                    root_id: target.selected.node_id,
                    mode: TrailMode::Neighborhood,
                    target_id: None,
                    depth,
                    direction: TrailDirection::Both,
                    caller_scope: TrailCallerScope::ProductionOnly,
                    edge_filter: Vec::new(),
                    show_utility_calls: false,
                    node_filter: Vec::new(),
                    max_nodes: 80,
                    layout_direction: LayoutDirection::Horizontal,
                })
                .map_err(map_api_error)?;
            write_http_json(&mut stream, 200, &context)
        }
        _ => write_http_json(&mut stream, 404, &serde_json::json!({"error": "not found"})),
    }
}

fn target_selection_from_params(params: &HashMap<String, String>) -> Result<args::TargetSelection> {
    if let Some(id) = params.get("id").filter(|value| !value.trim().is_empty()) {
        return Ok(args::TargetSelection::Id(NodeId(id.trim().to_string())));
    }
    let query = params.get("q").cloned().unwrap_or_default();
    if query.trim().is_empty() {
        bail!("Pass `q` or `id`.");
    }
    Ok(args::TargetSelection::Query(query))
}

fn write_http_json<T: serde::Serialize>(
    stream: &mut TcpStream,
    status: u16,
    value: &T,
) -> Result<()> {
    let body = serde_json::to_string_pretty(value)?;
    let status_text = match status {
        200 => "OK",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "OK",
    };
    write!(
        stream,
        "HTTP/1.1 {status} {status_text}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    )?;
    Ok(())
}

fn parse_query_string(query: &str) -> HashMap<String, String> {
    query
        .split('&')
        .filter(|part| !part.is_empty())
        .filter_map(|part| {
            let (key, value) = part.split_once('=').unwrap_or((part, ""));
            Some((url_decode(key)?, url_decode(value)?))
        })
        .collect()
}

fn search_repo_text_mode_param(value: &str) -> Option<SearchRepoTextMode> {
    match value.trim().to_ascii_lowercase().as_str() {
        "auto" => Some(SearchRepoTextMode::Auto),
        "on" | "true" | "1" => Some(SearchRepoTextMode::On),
        "off" | "false" | "0" => Some(SearchRepoTextMode::Off),
        _ => None,
    }
}

fn url_decode(value: &str) -> Option<String> {
    let mut out = Vec::with_capacity(value.len());
    let bytes = value.as_bytes();
    let mut idx = 0usize;
    while idx < bytes.len() {
        match bytes[idx] {
            b'+' => out.push(b' '),
            b'%' if idx + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[idx + 1..idx + 3]).ok()?;
                out.push(u8::from_str_radix(hex, 16).ok()?);
                idx += 2;
            }
            byte => out.push(byte),
        }
        idx += 1;
    }
    String::from_utf8(out).ok()
}

fn run_stdio_server(runtime: RuntimeContext) -> Result<()> {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let response = handle_stdio_message(&runtime, &line);
        writeln!(stdout, "{}", serde_json::to_string(&response)?)?;
        stdout.flush()?;
    }
    Ok(())
}

fn handle_stdio_message(runtime: &RuntimeContext, line: &str) -> serde_json::Value {
    let request: serde_json::Value = match serde_json::from_str(line) {
        Ok(value) => value,
        Err(error) => return serde_json::json!({"error": error.to_string()}),
    };
    let method = request
        .get("method")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    match method {
        "initialize" => serde_json::json!({"result": {"name": "codestory", "version": "0.1.0"}}),
        "tools/list" => stdio_tools_list_json(),
        "resources/list" => stdio_resources_list_json(),
        "resources/templates/list" => stdio_resource_templates_list_json(),
        "prompts/list" => stdio_prompts_list_json(),
        "prompts/get" => stdio_prompt_get_json(
            request
                .pointer("/params/name")
                .and_then(|value| value.as_str())
                .unwrap_or_default(),
        ),
        "resources/read" => {
            let uri = request
                .pointer("/params/uri")
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            read_stdio_resource(runtime, uri)
        }
        "tools/call" => handle_stdio_tool_call(runtime, &request),
        _ => serde_json::json!({"error": "unknown method"}),
    }
}

fn handle_stdio_tool_call(
    runtime: &RuntimeContext,
    request: &serde_json::Value,
) -> serde_json::Value {
    let name = request
        .pointer("/params/name")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    let query = request
        .pointer("/params/arguments/query")
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .to_string();
    match name {
        "search" => handle_stdio_search(runtime, query),
        "symbol" => handle_stdio_symbol(runtime, query),
        "trail" => handle_stdio_trail(runtime, query),
        "definition" => handle_stdio_definition(runtime, request),
        "references" => handle_stdio_references(runtime, request),
        "symbols" => handle_stdio_symbols(runtime, request),
        "snippet" => handle_stdio_snippet(runtime, request),
        "ask" => handle_stdio_ask(runtime, request, &query),
        _ => serde_json::json!({"error": "unknown tool"}),
    }
}

fn handle_stdio_search(runtime: &RuntimeContext, query: String) -> serde_json::Value {
    runtime
        .search
        .search_results(SearchRequest {
            query,
            repo_text: SearchRepoTextMode::Auto,
            limit_per_source: 10,
            hybrid_weights: None,
            hybrid_limits: None,
        })
        .map(|result| serde_json::json!({"result": result}))
        .unwrap_or_else(|error| serde_json::json!({"error": map_api_error(error).to_string()}))
}

fn handle_stdio_symbol(runtime: &RuntimeContext, query: String) -> serde_json::Value {
    resolve_target(runtime, args::TargetSelection::Query(query), None)
        .and_then(|target| {
            runtime
                .grounding
                .symbol_context(target.selected.node_id)
                .map_err(map_api_error)
        })
        .map(|result| serde_json::json!({"result": result}))
        .unwrap_or_else(|error| serde_json::json!({"error": error.to_string()}))
}

fn handle_stdio_trail(runtime: &RuntimeContext, query: String) -> serde_json::Value {
    resolve_target(runtime, args::TargetSelection::Query(query), None)
        .and_then(|target| {
            runtime
                .grounding
                .trail_context(TrailConfigDto {
                    root_id: target.selected.node_id,
                    mode: TrailMode::Neighborhood,
                    target_id: None,
                    depth: 2,
                    direction: TrailDirection::Both,
                    caller_scope: TrailCallerScope::ProductionOnly,
                    edge_filter: Vec::new(),
                    show_utility_calls: false,
                    node_filter: Vec::new(),
                    max_nodes: 80,
                    layout_direction: LayoutDirection::Horizontal,
                })
                .map_err(map_api_error)
        })
        .map(|result| serde_json::json!({"result": result}))
        .unwrap_or_else(|error| serde_json::json!({"error": error.to_string()}))
}

fn handle_stdio_definition(
    runtime: &RuntimeContext,
    request: &serde_json::Value,
) -> serde_json::Value {
    resolve_target(runtime, stdio_target_selection(request), None)
        .and_then(|target| {
            runtime
                .grounding
                .symbol_context(target.selected.node_id.clone())
                .map_err(map_api_error)
                .map(|symbol| {
                    serde_json::json!({
                        "resolution": build_query_resolution_output(&runtime.project_root, &target),
                        "definition": build_search_hit_output(&runtime.project_root, &target.selected, false),
                        "symbol": symbol,
                    })
                })
        })
        .map(|result| serde_json::json!({"result": result}))
        .unwrap_or_else(|error| serde_json::json!({"error": error.to_string()}))
}

fn handle_stdio_references(
    runtime: &RuntimeContext,
    request: &serde_json::Value,
) -> serde_json::Value {
    resolve_target(runtime, stdio_target_selection(request), None)
        .and_then(|target| {
            runtime
                .grounding
                .trail_context(TrailConfigDto {
                    root_id: target.selected.node_id.clone(),
                    mode: TrailMode::AllReferencing,
                    target_id: None,
                    depth: 0,
                    direction: TrailDirection::Incoming,
                    caller_scope: TrailCallerScope::IncludeTestsAndBenches,
                    edge_filter: Vec::new(),
                    show_utility_calls: false,
                    node_filter: Vec::new(),
                    max_nodes: 120,
                    layout_direction: LayoutDirection::Horizontal,
                })
                .map_err(map_api_error)
        })
        .map(|result| serde_json::json!({"result": result}))
        .unwrap_or_else(|error| serde_json::json!({"error": error.to_string()}))
}

fn handle_stdio_symbols(
    runtime: &RuntimeContext,
    request: &serde_json::Value,
) -> serde_json::Value {
    let limit = request
        .pointer("/params/arguments/limit")
        .and_then(|value| value.as_u64())
        .map(|value| value.min(2_000) as u32);
    let parent_id = request
        .pointer("/params/arguments/parent_id")
        .and_then(|value| value.as_str())
        .filter(|value| !value.is_empty());
    let result = if let Some(parent_id) = parent_id {
        runtime
            .grounding
            .list_children_symbols(ListChildrenSymbolsRequest {
                parent_id: NodeId(parent_id.to_string()),
            })
            .map(|symbols| {
                serde_json::to_value(symbols)
                    .unwrap_or_else(|error| serde_json::json!({"error": error.to_string()}))
            })
    } else {
        runtime
            .grounding
            .list_root_symbols(ListRootSymbolsRequest { limit })
            .map(|symbols| {
                serde_json::to_value(symbols)
                    .unwrap_or_else(|error| serde_json::json!({"error": error.to_string()}))
            })
    };
    result
        .map(|value| serde_json::json!({"result": value}))
        .unwrap_or_else(|error| serde_json::json!({"error": map_api_error(error).to_string()}))
}

fn handle_stdio_snippet(
    runtime: &RuntimeContext,
    request: &serde_json::Value,
) -> serde_json::Value {
    resolve_target(runtime, stdio_target_selection(request), None)
        .and_then(|target| {
            runtime
                .grounding
                .snippet_context(target.selected.node_id, 4)
                .map_err(map_api_error)
        })
        .map(|result| serde_json::json!({"result": result}))
        .unwrap_or_else(|error| serde_json::json!({"error": error.to_string()}))
}

fn handle_stdio_ask(
    runtime: &RuntimeContext,
    request: &serde_json::Value,
    default_prompt: &str,
) -> serde_json::Value {
    let prompt = request
        .pointer("/params/arguments/prompt")
        .and_then(|value| value.as_str())
        .unwrap_or(default_prompt)
        .to_string();
    runtime
        .agent
        .ask(AgentAskRequest {
            prompt,
            retrieval_profile: codestory_contracts::api::AgentRetrievalProfileSelectionDto::Auto,
            focus_node_id: None,
            max_results: Some(8),
            response_mode: AgentResponseModeDto::Structured,
            latency_budget_ms: None,
            include_evidence: true,
            hybrid_weights: None,
            connection: AgentConnectionSettingsDto::default(),
            run_local_agent: false,
        })
        .map(|result| serde_json::json!({"result": result}))
        .unwrap_or_else(|error| serde_json::json!({"error": map_api_error(error).to_string()}))
}

fn stdio_target_selection(request: &serde_json::Value) -> args::TargetSelection {
    if let Some(id) = request
        .pointer("/params/arguments/id")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
    {
        return args::TargetSelection::Id(NodeId(id.trim().to_string()));
    }
    let query = request
        .pointer("/params/arguments/query")
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .to_string();
    args::TargetSelection::Query(query)
}

fn stdio_tools_list_json() -> serde_json::Value {
    serde_json::json!({
        "result": {
            "tools": [
                stdio_tool_spec("search", "Search indexed symbols and repo text.", &["query"]),
                stdio_tool_spec("symbol", "Resolve a symbol and return details.", &["query"]),
                stdio_tool_spec("trail", "Return a graph trail around a symbol.", &["query"]),
                stdio_tool_spec("definition", "Return definition metadata for a symbol id or query.", &["query", "id"]),
                stdio_tool_spec("references", "Return incoming references for a symbol id or query.", &["query", "id"]),
                stdio_tool_spec("symbols", "Browse root symbols or children for a parent id.", &["parent_id", "limit"]),
                stdio_tool_spec("snippet", "Return a focused source snippet for a symbol id or query.", &["query", "id"]),
                stdio_tool_spec("ask", "Run DB-first agentic retrieval and return an answer packet.", &["prompt"])
            ]
        }
    })
}

fn stdio_tool_spec(name: &str, description: &str, properties: &[&str]) -> serde_json::Value {
    let props = properties
        .iter()
        .map(|property| {
            (
                (*property).to_string(),
                serde_json::json!({
                    "type": if *property == "limit" { "number" } else { "string" }
                }),
            )
        })
        .collect::<serde_json::Map<_, _>>();
    serde_json::json!({
        "name": name,
        "description": description,
        "inputSchema": {
            "type": "object",
            "properties": props
        }
    })
}

fn stdio_resources_list_json() -> serde_json::Value {
    serde_json::json!({
        "result": {
            "resources": [
                {"uri": "codestory://project", "name": "Project summary", "mimeType": "application/json"},
                {"uri": "codestory://grounding", "name": "Grounding snapshot", "mimeType": "application/json"},
                {"uri": "codestory://symbols/root", "name": "Root symbols", "mimeType": "application/json"}
            ]
        }
    })
}

fn stdio_resource_templates_list_json() -> serde_json::Value {
    serde_json::json!({
        "result": {
            "resourceTemplates": [
                {"uriTemplate": "codestory://symbol/{node_id}", "name": "Symbol details", "mimeType": "application/json"},
                {"uriTemplate": "codestory://references/{node_id}", "name": "Symbol references", "mimeType": "application/json"},
                {"uriTemplate": "codestory://snippet/{node_id}", "name": "Symbol snippet", "mimeType": "application/json"},
                {"uriTemplate": "codestory://trail/{node_id}", "name": "Symbol trail", "mimeType": "application/json"}
            ]
        }
    })
}

fn stdio_prompts_list_json() -> serde_json::Value {
    serde_json::json!({
        "result": {
            "prompts": [
                {"name": "explain_symbol", "description": "Explain a symbol using definition, references, and snippet context."},
                {"name": "trace_callflow", "description": "Trace the outgoing call flow for a symbol."},
                {"name": "impact_analysis", "description": "Find incoming references and likely downstream impact."}
            ]
        }
    })
}

fn stdio_prompt_get_json(name: &str) -> serde_json::Value {
    let prompt = match name {
        "trace_callflow" => {
            "Trace the call flow for `{symbol}`. Use CodeStory trail, definition, and snippet context. Return key calls, uncertain edges, and review notes."
        }
        "impact_analysis" => {
            "Analyze the impact of changing `{symbol}`. Use incoming references, related symbols, and snippets. Separate direct callers from broader risk."
        }
        _ => {
            "Explain `{symbol}` using CodeStory definition, references, and source snippet context. Keep claims tied to retrieved evidence."
        }
    };
    serde_json::json!({
        "result": {
            "description": name,
            "messages": [
                {
                    "role": "user",
                    "content": {
                        "type": "text",
                        "text": prompt
                    }
                }
            ]
        }
    })
}

fn read_stdio_resource(runtime: &RuntimeContext, uri: &str) -> serde_json::Value {
    let result = match uri {
        "codestory://project" => runtime
            .open_project_summary()
            .map(|summary| serde_json::json!(summary)),
        "codestory://grounding" => runtime
            .grounding
            .grounding_snapshot(GroundingBudgetDto::Balanced)
            .map(|snapshot| serde_json::json!(snapshot))
            .map_err(map_api_error),
        "codestory://symbols/root" => runtime
            .grounding
            .list_root_symbols(ListRootSymbolsRequest { limit: Some(300) })
            .map(|symbols| serde_json::json!(symbols))
            .map_err(map_api_error),
        _ => read_stdio_template_resource(runtime, uri),
    };
    result
        .map(|value| serde_json::json!({"result": {"contents": [{"uri": uri, "mimeType": "application/json", "text": value.to_string()}]}}))
        .unwrap_or_else(|error| serde_json::json!({"error": error.to_string()}))
}

fn read_stdio_template_resource(runtime: &RuntimeContext, uri: &str) -> Result<serde_json::Value> {
    let Some((kind, node_id)) = uri
        .strip_prefix("codestory://")
        .and_then(|tail| tail.split_once('/'))
    else {
        bail!("unknown resource");
    };
    let node_id = NodeId(node_id.to_string());
    match kind {
        "symbol" => runtime
            .grounding
            .symbol_context(node_id)
            .map(|value| serde_json::json!(value))
            .map_err(map_api_error),
        "references" => runtime
            .grounding
            .trail_context(TrailConfigDto {
                root_id: node_id,
                mode: TrailMode::AllReferencing,
                target_id: None,
                depth: 0,
                direction: TrailDirection::Incoming,
                caller_scope: TrailCallerScope::IncludeTestsAndBenches,
                edge_filter: Vec::new(),
                show_utility_calls: false,
                node_filter: Vec::new(),
                max_nodes: 120,
                layout_direction: LayoutDirection::Horizontal,
            })
            .map(|value| serde_json::json!(value))
            .map_err(map_api_error),
        "snippet" => runtime
            .grounding
            .snippet_context(node_id, 4)
            .map(|value| serde_json::json!(value))
            .map_err(map_api_error),
        "trail" => runtime
            .grounding
            .trail_context(TrailConfigDto {
                root_id: node_id,
                mode: TrailMode::Neighborhood,
                target_id: None,
                depth: 2,
                direction: TrailDirection::Both,
                caller_scope: TrailCallerScope::ProductionOnly,
                edge_filter: Vec::new(),
                show_utility_calls: false,
                node_filter: Vec::new(),
                max_nodes: 120,
                layout_direction: LayoutDirection::Horizontal,
            })
            .map(|value| serde_json::json!(value))
            .map_err(map_api_error),
        _ => bail!("unknown resource"),
    }
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
    symbol_hits: &[SearchHit],
    repo_text_hits: &[SearchHit],
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
    let mut queue = VecDeque::new();
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
        EdgeId, EdgeKind, GraphEdgeDto, GraphNodeDto, GraphResponse, IndexMode,
        IndexingPhaseTimings, NodeDetailsDto, NodeId, ProjectSummary, RetrievalModeDto,
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
            fallback_reason: None,
            fallback_message: None,
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
            &symbol_hits,
            &repo_text_hits,
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
            &symbol_hits,
            &[],
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
            &symbol_hits,
            &repo_text_hits,
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
        ];

        for command in commands {
            Cli::try_parse_from(command).expect("command should parse --output-file");
        }
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
            &symbol_hits,
            &[],
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
