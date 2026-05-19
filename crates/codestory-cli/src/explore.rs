use anyhow::Result;
use codestory_contracts::api::{
    IndexFreshnessDto, IndexFreshnessStatusDto, LayoutDirection, SnippetContextDto,
    SymbolContextDto, TrailCallerScope, TrailConfigDto, TrailContextDto, TrailDirection, TrailMode,
};
use std::{
    collections::{HashMap, HashSet},
    io::IsTerminal,
    time::Duration,
};

use crate::args::{
    self, ExploreCommand, ExploreOutput, ExploreSearchOutput, ExploreStatusOutput,
    NavigationOutput, QueryItemOutput, SearchHitOutput, TrailCommand,
};
use crate::output::{
    emit, render_retrieval_state, render_snippet_markdown, render_symbol_markdown,
    render_trail_markdown,
};
use crate::runtime::{self, RuntimeContext, ensure_index_ready, map_api_error, refresh_label};
use crate::{
    build_query_resolution_output, build_search_hit_output, ensure_dot_only_for_trail,
    preflight_output_file, resolve_target_or_emit_ambiguity,
};
use crate::{display, output};

pub(crate) fn run_explore(cmd: ExploreCommand) -> Result<()> {
    ensure_dot_only_for_trail(cmd.format, "explore")?;
    preflight_output_file(cmd.output_file.as_deref())?;
    let runtime = RuntimeContext::new(&cmd.project)?;
    let opened = runtime.ensure_open(cmd.refresh)?;
    ensure_index_ready(&opened, "explore")?;
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
    let trail = runtime
        .browser
        .trail_context(TrailConfigDto {
            root_id: target.selected.node_id.clone(),
            mode: TrailMode::Neighborhood,
            target_id: None,
            depth: cmd.depth,
            direction: TrailDirection::Both,
            caller_scope: TrailCallerScope::ProductionOnly,
            edge_filter: Vec::new(),
            show_utility_calls: false,
            hide_speculative: false,
            story: false,
            node_filter: Vec::new(),
            max_nodes: cmd.max_nodes.clamp(1, 120),
            layout_direction: LayoutDirection::Horizontal,
        })
        .map_err(map_api_error)?;
    let snippet_result = runtime
        .browser
        .snippet_context(target.selected.node_id.clone(), 4);
    let (snippet, snippet_layer_note) = match snippet_result {
        Ok(snippet) => (Some(snippet), "snippet_context: available".to_string()),
        Err(error) => (
            None,
            format!(
                "snippet_context: unavailable: {}: {}",
                error.code, error.message
            ),
        ),
    };
    let status = build_explore_status_output(
        &runtime,
        &opened,
        &target,
        cmd.refresh,
        cmd.output_file.as_deref(),
        &snippet_layer_note,
    );
    let search = build_explore_search_output(&runtime.project_root, &target);
    let navigation = build_navigation_output(&runtime.project_root, &target, &trail);
    let output = ExploreOutput {
        status,
        search,
        resolution: build_query_resolution_output(&runtime.project_root, &target),
        navigation,
        symbol: &symbol,
        trail: &trail,
        snippet: snippet.as_ref(),
    };
    let render_context = ExploreRenderContext {
        project_root: &runtime.project_root,
        target: &target,
        status: &output.status,
        search: &output.search,
        navigation: &output.navigation,
        symbol: &symbol,
        trail: &trail,
        snippet: snippet.as_ref(),
        snippet_layer_note: &snippet_layer_note,
    };
    let markdown = render_explore_markdown(&render_context);
    if cmd.format == args::OutputFormat::Markdown
        && cmd.output_file.is_none()
        && !cmd.no_tui
        && std::io::stdout().is_terminal()
    {
        return run_explore_tui(&render_context);
    }
    emit(cmd.format, &output, markdown, cmd.output_file.as_deref())
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

pub(crate) fn browser_query_item_to_output(
    project_root: &std::path::Path,
    item: &codestory_runtime::BrowserQueryItem,
) -> QueryItemOutput {
    QueryItemOutput {
        node_id: item.node_id.0.clone(),
        node_ref: output::node_ref(
            project_root,
            item.file_path.as_deref(),
            item.line,
            &item.display_name,
        ),
        display_name: item.display_name.clone(),
        kind: item.kind,
        file_path: item
            .file_path
            .as_deref()
            .map(|path| display::relative_path(project_root, path)),
        line: item.line,
        depth: item.depth,
        source: item.source.clone(),
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

fn build_explore_search_output(
    project_root: &std::path::Path,
    target: &runtime::ResolvedTarget,
) -> ExploreSearchOutput {
    ExploreSearchOutput {
        selector: target.selector,
        requested: target.requested.clone(),
        file_filter: target
            .file_filter
            .as_deref()
            .map(crate::display::clean_path_string),
        selected: build_search_hit_output(project_root, &target.selected, false),
        alternatives: target
            .alternatives
            .iter()
            .skip(1)
            .map(|hit| build_search_hit_output(project_root, hit, false))
            .collect(),
    }
}

fn build_explore_status_output(
    runtime: &RuntimeContext,
    opened: &runtime::OpenedProject,
    target: &runtime::ResolvedTarget,
    requested_refresh: args::RefreshMode,
    output_file: Option<&std::path::Path>,
    snippet_layer_note: &str,
) -> ExploreStatusOutput {
    let project = display::clean_path_string(&runtime.project_root.to_string_lossy());
    let storage_path = display::clean_path_string(&runtime.storage_path.to_string_lossy());
    let output_target = output_file
        .map(|path| display::clean_path_string(&path.to_string_lossy()))
        .unwrap_or_else(|| "stdout".to_string());
    let retrieval = opened.summary.retrieval.clone();
    let freshness = opened.summary.freshness.clone();
    let next_commands =
        explore_next_commands(&project, target, retrieval.as_ref(), freshness.as_ref());
    let layer_notes = explore_layer_notes(
        &storage_path,
        &output_target,
        &opened.summary,
        target,
        retrieval.as_ref(),
        freshness.as_ref(),
        snippet_layer_note,
    );

    ExploreStatusOutput {
        project,
        storage_path,
        refresh: refresh_label(requested_refresh, opened.refresh_mode),
        output_target,
        indexed_files: opened.summary.stats.file_count,
        indexed_nodes: opened.summary.stats.node_count,
        indexed_edges: opened.summary.stats.edge_count,
        retrieval,
        freshness,
        next_commands,
        layer_notes,
    }
}

fn explore_next_commands(
    project: &str,
    target: &runtime::ResolvedTarget,
    retrieval: Option<&codestory_contracts::api::RetrievalStateDto>,
    freshness: Option<&IndexFreshnessDto>,
) -> Vec<String> {
    let node_id = &target.selected.node_id.0;
    let mut commands = Vec::new();
    if freshness.is_some_and(|state| state.status == IndexFreshnessStatusDto::Stale) {
        commands.push(format!(
            "codestory-cli index --project \"{project}\" --refresh incremental"
        ));
    }
    commands.push(format!(
        "codestory-cli context --project \"{project}\" --id {node_id}"
    ));
    commands.push(format!(
        "codestory-cli trail --project \"{project}\" --id {node_id} --depth 3"
    ));
    commands.push(format!(
        "codestory-cli snippet --project \"{project}\" --id {node_id}"
    ));
    if retrieval.is_some_and(|state| !state.semantic_ready) {
        commands.push(format!(
            "codestory-cli doctor --project \"{project}\" --format markdown"
        ));
    }
    commands
}

fn explore_layer_notes(
    storage_path: &str,
    output_target: &str,
    summary: &codestory_contracts::api::ProjectSummary,
    target: &runtime::ResolvedTarget,
    retrieval: Option<&codestory_contracts::api::RetrievalStateDto>,
    freshness: Option<&IndexFreshnessDto>,
    snippet_layer_note: &str,
) -> Vec<String> {
    let mut notes = vec![
        format!("cache: reading existing SQLite cache at `{storage_path}`"),
        format!(
            "index: ready with files={} nodes={} edges={}",
            summary.stats.file_count, summary.stats.node_count, summary.stats.edge_count
        ),
        format!(
            "query_resolution: `{}` resolved to `{}`",
            target.requested, target.selected.display_name
        ),
        "context: use context --id for a deep evidence packet around this target".to_string(),
        format!("output_write: target `{output_target}` passed preflight"),
        snippet_layer_note.to_string(),
    ];
    notes.push(match retrieval {
        Some(retrieval) if retrieval.semantic_ready => {
            format!(
                "semantic_runtime: ready with {} semantic docs",
                retrieval.semantic_doc_count
            )
        }
        Some(retrieval) => format!(
            "semantic_runtime: {}",
            retrieval
                .fallback_message
                .as_deref()
                .unwrap_or("semantic retrieval fallback is active")
        ),
        None => "semantic_runtime: retrieval state unavailable".to_string(),
    });
    notes.push(match freshness {
        Some(freshness) => format!("freshness: {}", render_explore_freshness(freshness)),
        None => "freshness: not reported by this cache open".to_string(),
    });
    notes
}

fn render_explore_freshness(freshness: &IndexFreshnessDto) -> String {
    match freshness.status {
        IndexFreshnessStatusDto::Fresh => format!(
            "fresh checked_files={} duration_ms={}",
            freshness.checked_file_count, freshness.duration_ms
        ),
        IndexFreshnessStatusDto::Stale => format!(
            "stale changed={} new={} removed={} checked_files={} duration_ms={}",
            freshness.changed_file_count,
            freshness.new_file_count,
            freshness.removed_file_count,
            freshness.checked_file_count,
            freshness.duration_ms
        ),
        IndexFreshnessStatusDto::NotChecked => format!(
            "not_checked reason={}",
            freshness.reason.as_deref().unwrap_or("not reported")
        ),
    }
}

fn render_explore_status_markdown(status: &ExploreStatusOutput) -> String {
    let mut markdown = String::new();
    markdown.push_str(&format!("- project: `{}`\n", status.project));
    markdown.push_str(&format!("- cache: `{}`\n", status.storage_path));
    markdown.push_str(&format!("- refresh: `{}`\n", status.refresh));
    markdown.push_str(&format!("- output: `{}`\n", status.output_target));
    markdown.push_str(&format!(
        "- indexed: files={} nodes={} edges={}\n",
        status.indexed_files, status.indexed_nodes, status.indexed_edges
    ));
    if let Some(retrieval) = status.retrieval.as_ref() {
        markdown.push_str(&format!(
            "- retrieval: {}\n",
            render_retrieval_state(retrieval)
        ));
    } else {
        markdown.push_str("- retrieval: unavailable\n");
    }
    if let Some(freshness) = status.freshness.as_ref() {
        markdown.push_str(&format!(
            "- freshness: {}\n",
            render_explore_freshness(freshness)
        ));
    } else {
        markdown.push_str("- freshness: unavailable\n");
    }
    if let Some(command) = status.next_commands.first() {
        markdown.push_str(&format!("- next: `{command}`\n"));
    }
    markdown.push_str("- layers:\n");
    for note in &status.layer_notes {
        markdown.push_str(&format!("  - {note}\n"));
    }
    markdown
}

fn format_query_selector(selector: args::QuerySelectorOutput) -> &'static str {
    match selector {
        args::QuerySelectorOutput::Id => "id",
        args::QuerySelectorOutput::Query => "query",
    }
}

fn render_location_ref(
    node_ref: Option<&str>,
    file_path: Option<&str>,
    line: Option<u32>,
    display_name: &str,
) -> String {
    if let Some(node_ref) = node_ref {
        return format!("`{node_ref}`");
    }
    if let Some(file_path) = file_path {
        if let Some(line) = line {
            return format!("{display_name} `{file_path}:{line}`");
        }
        return format!("{display_name} `{file_path}`");
    }
    display_name.to_string()
}

fn render_search_hit_output_ref(hit: &SearchHitOutput) -> String {
    render_location_ref(
        hit.node_ref.as_deref(),
        hit.file_path.as_deref(),
        hit.line,
        &hit.display_name,
    )
}

fn render_query_item_output_ref(item: &QueryItemOutput) -> String {
    render_location_ref(
        item.node_ref.as_deref(),
        item.file_path.as_deref(),
        item.line,
        &item.display_name,
    )
}

fn render_explore_search_markdown(search: &ExploreSearchOutput) -> String {
    let mut markdown = String::new();
    markdown.push_str(&format!(
        "- selector: `{}`\n",
        format_query_selector(search.selector)
    ));
    markdown.push_str(&format!("- requested: `{}`\n", search.requested));
    if let Some(file_filter) = search.file_filter.as_deref() {
        markdown.push_str(&format!("- file_filter: `{file_filter}`\n"));
    }
    markdown.push_str(&format!(
        "- selected: {}\n",
        render_search_hit_output_ref(&search.selected)
    ));
    markdown.push_str(&format!("- alternatives: {}\n", search.alternatives.len()));
    for (index, alternative) in search.alternatives.iter().take(8).enumerate() {
        markdown.push_str(&format!(
            "  - {}. {}\n",
            index + 1,
            render_search_hit_output_ref(alternative)
        ));
    }
    markdown
}

fn render_explore_results_markdown(navigation: &NavigationOutput) -> String {
    let mut markdown = String::new();
    markdown.push_str(&format!(
        "- definition: {}\n",
        render_search_hit_output_ref(&navigation.definition)
    ));
    markdown.push_str(&format!(
        "- incoming_references: {}\n",
        navigation.incoming_references.len()
    ));
    markdown.push_str(&format!(
        "- outgoing_references: {}\n",
        navigation.outgoing_references.len()
    ));
    for incoming in navigation.incoming_references.iter().take(6) {
        markdown.push_str(&format!(
            "- incoming: {}\n",
            render_query_item_output_ref(incoming)
        ));
    }
    for outgoing in navigation.outgoing_references.iter().take(6) {
        markdown.push_str(&format!(
            "- outgoing: {}\n",
            render_query_item_output_ref(outgoing)
        ));
    }
    markdown
}

fn explore_trail_command(
    project_root: &std::path::Path,
    target: &runtime::ResolvedTarget,
    trail: &TrailContextDto,
) -> TrailCommand {
    TrailCommand {
        project: args::ProjectArgs {
            project: project_root.to_path_buf(),
            cache_dir: None,
        },
        target: args::TargetArgs {
            id: Some(target.selected.node_id.0.clone()),
            query: None,
            file: None,
            choose: None,
        },
        mode: args::CliTrailMode::Neighborhood,
        depth: Some(2),
        direction: Some(args::CliDirection::Both),
        max_nodes: trail.trail.nodes.len().min(u32::MAX as usize) as u32,
        include_tests: false,
        show_utility_calls: false,
        hide_speculative: false,
        story: false,
        layout: args::CliLayout::Horizontal,
        refresh: args::RefreshMode::None,
        format: args::OutputFormat::Markdown,
        output_file: None,
        mermaid: false,
    }
}

struct ExploreRenderContext<'a> {
    project_root: &'a std::path::Path,
    target: &'a runtime::ResolvedTarget,
    status: &'a ExploreStatusOutput,
    search: &'a ExploreSearchOutput,
    navigation: &'a NavigationOutput,
    symbol: &'a SymbolContextDto,
    trail: &'a TrailContextDto,
    snippet: Option<&'a SnippetContextDto>,
    snippet_layer_note: &'a str,
}

fn render_explore_markdown(context: &ExploreRenderContext<'_>) -> String {
    let mut markdown = String::new();
    markdown.push_str("# Explore\n");
    markdown.push_str("status:\n");
    markdown.push_str(&render_explore_status_markdown(context.status));
    markdown.push_str("search:\n");
    markdown.push_str(&render_explore_search_markdown(context.search));
    markdown.push_str("results:\n");
    markdown.push_str(&render_explore_results_markdown(context.navigation));
    markdown.push_str("resolution:\n");
    markdown.push_str(&format!(
        "- {}\n",
        output::node_ref(
            context.project_root,
            context.target.selected.file_path.as_deref(),
            context.target.selected.line,
            &context.target.selected.display_name
        )
        .unwrap_or_else(|| context.target.selected.display_name.clone())
    ));
    markdown.push_str("navigation:\n");
    if let Some(node_ref) = context.navigation.definition.node_ref.as_deref() {
        markdown.push_str(&format!("- definition: `{node_ref}`\n"));
    } else {
        markdown.push_str(&format!(
            "- definition: {}\n",
            context.navigation.definition.display_name
        ));
    }
    markdown.push_str(&format!(
        "- incoming_references: {}\n",
        context.navigation.incoming_references.len()
    ));
    markdown.push_str(&format!(
        "- outgoing_references: {}\n",
        context.navigation.outgoing_references.len()
    ));
    markdown.push_str("symbol:\n");
    markdown.push_str(&render_symbol_markdown(
        context.project_root,
        context.target,
        context.symbol,
    ));
    markdown.push_str("\ntrail:\n");
    let cmd = explore_trail_command(context.project_root, context.target, context.trail);
    markdown.push_str(&render_trail_markdown(
        context.project_root,
        context.target,
        context.trail,
        &cmd,
    ));
    markdown.push_str("\nsnippet:\n");
    markdown.push_str(&format!("- {}\n", context.snippet_layer_note));
    if let Some(snippet) = context.snippet {
        markdown.push_str(&render_snippet_markdown(
            context.project_root,
            context.target,
            snippet,
            false,
        ));
    }
    markdown
}

struct ExplorePane {
    label: &'static str,
    body: String,
}

fn build_explore_panes(context: &ExploreRenderContext<'_>) -> Vec<ExplorePane> {
    vec![
        ExplorePane {
            label: "Status",
            body: render_explore_status_markdown(context.status),
        },
        ExplorePane {
            label: "Search",
            body: render_explore_search_markdown(context.search),
        },
        ExplorePane {
            label: "Results",
            body: render_explore_results_markdown(context.navigation),
        },
        ExplorePane {
            label: "Detail",
            body: render_symbol_markdown(context.project_root, context.target, context.symbol),
        },
        ExplorePane {
            label: "Trail",
            body: {
                let cmd =
                    explore_trail_command(context.project_root, context.target, context.trail);
                render_trail_markdown(context.project_root, context.target, context.trail, &cmd)
            },
        },
        ExplorePane {
            label: "Snippet",
            body: format!(
                "{}\n{}",
                context.snippet_layer_note,
                context
                    .snippet
                    .map(|snippet| {
                        render_snippet_markdown(
                            context.project_root,
                            context.target,
                            snippet,
                            false,
                        )
                    })
                    .unwrap_or_default()
            ),
        },
    ]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExploreTuiAction {
    NextPane,
    PreviousPane,
    ScrollUp(u16),
    ScrollDown(u16),
    Home,
    Quit,
    None,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExploreTuiState {
    pub(crate) selected: usize,
    pub(crate) scroll: Vec<u16>,
}

impl ExploreTuiState {
    pub(crate) fn new(pane_count: usize) -> Self {
        Self {
            selected: 0,
            scroll: vec![0; pane_count.max(1)],
        }
    }

    pub(crate) fn apply(&mut self, action: ExploreTuiAction) -> bool {
        match action {
            ExploreTuiAction::NextPane => self.selected = (self.selected + 1) % self.scroll.len(),
            ExploreTuiAction::PreviousPane => {
                self.selected = (self.selected + self.scroll.len() - 1) % self.scroll.len();
            }
            ExploreTuiAction::ScrollUp(lines) => {
                self.scroll[self.selected] = self.scroll[self.selected].saturating_sub(lines);
            }
            ExploreTuiAction::ScrollDown(lines) => {
                self.scroll[self.selected] = self.scroll[self.selected].saturating_add(lines);
            }
            ExploreTuiAction::Home => self.scroll[self.selected] = 0,
            ExploreTuiAction::Quit => return true,
            ExploreTuiAction::None => {}
        }
        false
    }
}

pub(crate) fn explore_tui_action(key: crossterm::event::KeyEvent) -> ExploreTuiAction {
    use crossterm::event::{KeyCode, KeyModifiers};
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => ExploreTuiAction::Quit,
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            ExploreTuiAction::Quit
        }
        KeyCode::Tab => ExploreTuiAction::NextPane,
        KeyCode::BackTab => ExploreTuiAction::PreviousPane,
        KeyCode::Up | KeyCode::Char('k') => ExploreTuiAction::ScrollUp(1),
        KeyCode::Down | KeyCode::Char('j') => ExploreTuiAction::ScrollDown(1),
        KeyCode::PageUp => ExploreTuiAction::ScrollUp(10),
        KeyCode::PageDown => ExploreTuiAction::ScrollDown(10),
        KeyCode::Home => ExploreTuiAction::Home,
        _ => ExploreTuiAction::None,
    }
}

struct TerminalCleanup;

impl Drop for TerminalCleanup {
    fn drop(&mut self) {
        let _ = crossterm::terminal::disable_raw_mode();
        let _ = crossterm::execute!(std::io::stdout(), crossterm::terminal::LeaveAlternateScreen);
    }
}

fn run_explore_tui(context: &ExploreRenderContext<'_>) -> Result<()> {
    use crossterm::{
        event::{self, Event},
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

    let panes = build_explore_panes(context);

    enable_raw_mode()?;
    let _cleanup = TerminalCleanup;
    crossterm::execute!(std::io::stdout(), EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(std::io::stdout());
    let mut terminal = Terminal::new(backend)?;
    let mut state = ExploreTuiState::new(panes.len());

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
                context.project_root,
                context.target.selected.file_path.as_deref(),
                context.target.selected.line,
                &context.target.selected.display_name,
            )
            .unwrap_or_else(|| context.target.selected.display_name.clone());
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
                .map(|(idx, pane)| {
                    let style = if idx == state.selected {
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default()
                    };
                    ListItem::new(Line::from(Span::styled(pane.label, style)))
                })
                .collect::<Vec<_>>();
            frame.render_widget(
                List::new(nav_items).block(Block::default().borders(Borders::ALL).title("Panes")),
                body[0],
            );

            frame.render_widget(
                Paragraph::new(panes[state.selected].body.as_str())
                    .block(
                        Block::default()
                            .borders(Borders::ALL)
                            .title(panes[state.selected].label),
                    )
                    .wrap(Wrap { trim: false })
                    .scroll((state.scroll[state.selected], 0)),
                body[1],
            );
            frame.render_widget(
                Paragraph::new("Tab/Shift-Tab pane  Up/Down scroll  Home top  q quit"),
                shell[2],
            );
        })?;

        if event::poll(Duration::from_millis(250))?
            && let Event::Key(key) = event::read()?
            && state.apply(explore_tui_action(key))
        {
            break;
        }
    }
    terminal.show_cursor()?;
    Ok(())
}
