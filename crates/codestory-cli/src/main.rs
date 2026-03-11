use anyhow::{Context, Result, anyhow, bail};
use clap::{Args, Parser, Subcommand, ValueEnum};
use codestory_api::{
    ApiError, GroundingBudgetDto, GroundingSnapshotDto, IndexMode, IndexingPhaseTimings,
    LayoutDirection, NodeDetailsDto, NodeDetailsRequest, NodeId, ProjectSummary, SearchHit,
    SearchRequest, SnippetContextDto, SymbolContextDto, TrailCallerScope, TrailConfigDto,
    TrailContextDto, TrailDirection, TrailMode,
};
use codestory_app::AppController;
use directories::ProjectDirs;
use serde::Serialize;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

mod query_resolution;

use query_resolution::{compare_resolution_hits, resolution_rank, search_hit_matches_file_filter};

#[derive(Parser, Debug)]
#[command(author, version, about = "Skill-first repo grounding runtime", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    Index(IndexCommand),
    Ground(GroundCommand),
    Search(SearchCommand),
    Symbol(SymbolCommand),
    Trail(TrailCommand),
    Snippet(SnippetCommand),
}

#[derive(Args, Debug, Clone)]
struct ProjectArgs {
    #[arg(long, alias = "path", default_value = ".")]
    project: PathBuf,
    #[arg(long)]
    cache_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum OutputFormat {
    Markdown,
    Json,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum RefreshMode {
    Auto,
    Full,
    Incremental,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum CliGroundingBudget {
    Strict,
    Balanced,
    Max,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum CliTrailMode {
    Neighborhood,
    Referenced,
    Referencing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum CliDirection {
    Incoming,
    Outgoing,
    Both,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum CliLayout {
    Horizontal,
    Vertical,
}

#[derive(Args, Debug)]
struct IndexCommand {
    #[command(flatten)]
    project: ProjectArgs,
    #[arg(long, value_enum, default_value_t = RefreshMode::Auto)]
    refresh: RefreshMode,
    #[arg(long, value_enum, default_value_t = OutputFormat::Markdown)]
    format: OutputFormat,
}

#[derive(Args, Debug)]
struct GroundCommand {
    #[command(flatten)]
    project: ProjectArgs,
    #[arg(long, value_enum, default_value_t = CliGroundingBudget::Balanced)]
    budget: CliGroundingBudget,
    #[arg(long, value_enum, default_value_t = RefreshMode::Auto)]
    refresh: RefreshMode,
    #[arg(long, value_enum, default_value_t = OutputFormat::Markdown)]
    format: OutputFormat,
}

#[derive(Args, Debug)]
struct SearchCommand {
    #[command(flatten)]
    project: ProjectArgs,
    #[arg(long)]
    query: String,
    #[arg(long, default_value_t = 10)]
    limit: u32,
    #[arg(long, value_enum, default_value_t = RefreshMode::None)]
    refresh: RefreshMode,
    #[arg(long, value_enum, default_value_t = OutputFormat::Markdown)]
    format: OutputFormat,
}

#[derive(Args, Debug, Clone)]
struct TargetArgs {
    #[arg(long, conflicts_with = "query")]
    id: Option<String>,
    #[arg(long, conflicts_with = "id")]
    query: Option<String>,
    #[arg(long, requires = "query")]
    file: Option<String>,
}

#[derive(Args, Debug)]
struct SymbolCommand {
    #[command(flatten)]
    project: ProjectArgs,
    #[command(flatten)]
    target: TargetArgs,
    #[arg(long, value_enum, default_value_t = RefreshMode::None)]
    refresh: RefreshMode,
    #[arg(long, value_enum, default_value_t = OutputFormat::Markdown)]
    format: OutputFormat,
}

#[derive(Args, Debug)]
struct TrailCommand {
    #[command(flatten)]
    project: ProjectArgs,
    #[command(flatten)]
    target: TargetArgs,
    #[arg(long, value_enum, default_value_t = CliTrailMode::Neighborhood)]
    mode: CliTrailMode,
    #[arg(long)]
    depth: Option<u32>,
    #[arg(long, value_enum)]
    direction: Option<CliDirection>,
    #[arg(long, default_value_t = 24)]
    max_nodes: u32,
    #[arg(long)]
    include_tests: bool,
    #[arg(long)]
    show_utility_calls: bool,
    #[arg(long, value_enum, default_value_t = CliLayout::Horizontal)]
    layout: CliLayout,
    #[arg(long, value_enum, default_value_t = RefreshMode::None)]
    refresh: RefreshMode,
    #[arg(long, value_enum, default_value_t = OutputFormat::Markdown)]
    format: OutputFormat,
}

#[derive(Args, Debug)]
struct SnippetCommand {
    #[command(flatten)]
    project: ProjectArgs,
    #[command(flatten)]
    target: TargetArgs,
    #[arg(long, default_value_t = 4)]
    context: usize,
    #[arg(long, value_enum, default_value_t = RefreshMode::None)]
    refresh: RefreshMode,
    #[arg(long, value_enum, default_value_t = OutputFormat::Markdown)]
    format: OutputFormat,
}

struct RuntimeContext {
    controller: AppController,
    project_root: PathBuf,
    storage_path: PathBuf,
}

#[derive(Debug)]
struct OpenedProject {
    summary: ProjectSummary,
    refresh_mode: Option<IndexMode>,
    phase_timings: Option<IndexingPhaseTimings>,
}

#[derive(Debug)]
enum TargetSelection {
    Id(NodeId),
    Query(String),
}

#[derive(Debug, Clone)]
struct ResolvedTarget {
    requested: String,
    selected: SearchHit,
    alternatives: Vec<SearchHit>,
}

#[derive(Debug, Serialize)]
struct IndexOutput<'a> {
    project: &'a str,
    storage_path: &'a str,
    refresh: &'a str,
    summary: &'a ProjectSummary,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    phase_timings: Option<&'a IndexingPhaseTimings>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Index(cmd) => run_index(cmd),
        Command::Ground(cmd) => run_ground(cmd),
        Command::Search(cmd) => run_search(cmd),
        Command::Symbol(cmd) => run_symbol(cmd),
        Command::Trail(cmd) => run_trail(cmd),
        Command::Snippet(cmd) => run_snippet(cmd),
    }
}

fn run_index(cmd: IndexCommand) -> Result<()> {
    let runtime = RuntimeContext::new(&cmd.project)?;
    let opened = runtime.ensure_open(cmd.refresh)?;
    let refresh_label = refresh_label(cmd.refresh, opened.refresh_mode);
    let storage_path = runtime.storage_path.to_string_lossy().to_string();
    let output = IndexOutput {
        project: &opened.summary.root,
        storage_path: &storage_path,
        refresh: &refresh_label,
        summary: &opened.summary,
        phase_timings: opened.phase_timings.as_ref(),
    };

    let markdown = render_index_markdown(&output);
    emit(cmd.format, &output, markdown)
}

fn run_ground(cmd: GroundCommand) -> Result<()> {
    let runtime = RuntimeContext::new(&cmd.project)?;
    let opened = runtime.ensure_ground_open(cmd.refresh)?;
    ensure_index_ready(&opened, "ground")?;

    let snapshot = runtime
        .controller
        .grounding_snapshot(cmd.budget.into())
        .map_err(map_api_error)?;
    let markdown = render_ground_markdown(&runtime.project_root, &snapshot);
    emit(cmd.format, &snapshot, markdown)
}

fn run_search(cmd: SearchCommand) -> Result<()> {
    let runtime = RuntimeContext::new(&cmd.project)?;
    let opened = runtime.ensure_open(cmd.refresh)?;
    ensure_index_ready(&opened, "search")?;

    let limit = cmd.limit.clamp(1, 50) as usize;
    let mut hits = runtime
        .controller
        .search(SearchRequest {
            query: cmd.query.clone(),
        })
        .map_err(map_api_error)?;
    if looks_like_text_query(&cmd.query) {
        hits.extend(scan_repo_text_hits(
            &runtime.project_root,
            &cmd.query,
            limit,
        )?);
        hits.sort_by(|left, right| {
            right
                .score
                .partial_cmp(&left.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| left.display_name.cmp(&right.display_name))
        });
        hits.dedup_by(|left, right| {
            left.file_path == right.file_path
                && left.line == right.line
                && left.display_name == right.display_name
                && left.kind == right.kind
        });
    }
    hits.truncate(limit);

    let markdown = render_search_markdown(&runtime.project_root, &cmd.query, &hits);
    emit(cmd.format, &hits, markdown)
}

fn run_symbol(cmd: SymbolCommand) -> Result<()> {
    let runtime = RuntimeContext::new(&cmd.project)?;
    let opened = runtime.ensure_open(cmd.refresh)?;
    ensure_index_ready(&opened, "symbol")?;

    let file_filter = cmd.target.file_filter();
    let target = resolve_target(&runtime, cmd.target.selection()?, file_filter.as_deref())?;
    let context = runtime
        .controller
        .symbol_context(target.selected.node_id.clone())
        .map_err(map_api_error)?;
    let markdown = render_symbol_markdown(&runtime.project_root, &target, &context);
    emit(cmd.format, &context, markdown)
}

fn run_trail(cmd: TrailCommand) -> Result<()> {
    let runtime = RuntimeContext::new(&cmd.project)?;
    let opened = runtime.ensure_open(cmd.refresh)?;
    ensure_index_ready(&opened, "trail")?;

    let file_filter = cmd.target.file_filter();
    let target = resolve_target(&runtime, cmd.target.selection()?, file_filter.as_deref())?;
    let request = build_trail_request(&target.selected.node_id, &cmd);
    let context = runtime
        .controller
        .trail_context(request)
        .map_err(map_api_error)?;
    let markdown = render_trail_markdown(&runtime.project_root, &target, &context, &cmd);
    emit(cmd.format, &context, markdown)
}

fn run_snippet(cmd: SnippetCommand) -> Result<()> {
    let runtime = RuntimeContext::new(&cmd.project)?;
    let opened = runtime.ensure_open(cmd.refresh)?;
    ensure_index_ready(&opened, "snippet")?;

    let file_filter = cmd.target.file_filter();
    let target = resolve_target(&runtime, cmd.target.selection()?, file_filter.as_deref())?;
    let context = runtime
        .controller
        .snippet_context(target.selected.node_id.clone(), cmd.context)
        .map_err(map_api_error)?;
    let markdown = render_snippet_markdown(&runtime.project_root, &target, &context);
    emit(cmd.format, &context, markdown)
}

impl RuntimeContext {
    fn new(args: &ProjectArgs) -> Result<Self> {
        let project_root = canonicalize_project_root(&args.project)?;
        let cache_root = cache_root_for_project(&project_root, args.cache_dir.as_deref())?;
        let storage_path = cache_root.join("codestory.db");
        Ok(Self {
            controller: AppController::new(),
            project_root,
            storage_path,
        })
    }

    fn ensure_open(&self, refresh: RefreshMode) -> Result<OpenedProject> {
        let mut summary = self.open_project_summary()?;
        let refresh_mode = resolve_refresh_request(refresh, &summary);
        let mut phase_timings = None;
        if let Some(mode) = refresh_mode {
            phase_timings = Some(
                self.controller
                    .run_indexing_blocking_without_runtime_refresh(mode)
                    .map_err(map_api_error)?,
            );
            summary = self.open_project_summary()?;
        }

        Ok(OpenedProject {
            summary,
            refresh_mode,
            phase_timings,
        })
    }

    fn ensure_ground_open(&self, refresh: RefreshMode) -> Result<OpenedProject> {
        let mut summary = self.open_project_summary()?;
        let refresh_mode = resolve_refresh_request(refresh, &summary);
        let mut phase_timings = None;
        if let Some(mode) = refresh_mode {
            phase_timings = Some(
                self.controller
                    .run_indexing_blocking_without_runtime_refresh(mode)
                    .map_err(map_api_error)?,
            );
            summary = self.open_project_summary()?;
        }

        Ok(OpenedProject {
            summary,
            refresh_mode,
            phase_timings,
        })
    }

    fn open_project_summary(&self) -> Result<ProjectSummary> {
        self.controller
            .open_project_summary_with_storage_path(
                self.project_root.clone(),
                self.storage_path.clone(),
            )
            .map_err(map_api_error)
    }
}

impl TargetArgs {
    fn selection(&self) -> Result<TargetSelection> {
        match (&self.id, &self.query) {
            (Some(id), None) => Ok(TargetSelection::Id(NodeId(id.trim().to_string()))),
            (None, Some(query)) if !query.trim().is_empty() => {
                Ok(TargetSelection::Query(query.trim().to_string()))
            }
            (Some(_), Some(_)) => bail!("Pass only one of --id or --query."),
            (None, None) => bail!("Pass either --id or --query."),
            (None, Some(_)) => bail!("--query cannot be empty."),
        }
    }

    fn file_filter(&self) -> Option<String> {
        self.file
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    }
}

fn resolve_target(
    runtime: &RuntimeContext,
    target: TargetSelection,
    file_filter: Option<&str>,
) -> Result<ResolvedTarget> {
    match target {
        TargetSelection::Id(id) => {
            let details = runtime
                .controller
                .node_details(NodeDetailsRequest { id: id.clone() })
                .map_err(map_api_error)?;
            Ok(ResolvedTarget {
                requested: format!("id:{}", id.0),
                selected: search_hit_from_node(&details),
                alternatives: Vec::new(),
            })
        }
        TargetSelection::Query(query) => {
            let mut alternatives = runtime
                .controller
                .search_hybrid(
                    SearchRequest {
                        query: query.clone(),
                    },
                    None,
                    Some(50),
                    None,
                )
                .map_err(map_api_error)?;
            if let Some(file_filter) = file_filter {
                alternatives.retain(|hit| search_hit_matches_file_filter(hit, file_filter));
            }
            if alternatives.is_empty() {
                if let Some(file_filter) = file_filter {
                    return Err(anyhow!(
                        "No symbol matched query `{query}` within files matching `{file_filter}`. Run `codestory-cli search --query \"{query}\"` to inspect candidates."
                    ));
                }
                return Err(anyhow!(
                    "No symbol matched query `{query}`. Run `codestory-cli search --query \"{query}\"` to inspect candidates."
                ));
            }

            alternatives.sort_by(|left, right| compare_resolution_hits(&query, left, right));

            if alternatives.len() > 1 {
                let rank0 = resolution_rank(&query, &alternatives[0]);
                let rank1 = resolution_rank(&query, &alternatives[1]);
                if rank0 == rank1 {
                    let name0 = &alternatives[0].display_name;
                    let name1 = &alternatives[1].display_name;
                    bail!(
                        "Query `{query}` is ambiguous. It matches multiple distinct symbols equally well, including:\n  • {name0}\n  • {name1}\n\nPlease steer resolution by providing a more qualified name (e.g. `Namespace::Symbol` or a partial path)."
                    );
                }
            }

            let selected = alternatives.first().cloned().ok_or_else(|| {
                if let Some(file_filter) = file_filter {
                    anyhow!(
                        "No symbol matched query `{query}` within files matching `{file_filter}`. Run `codestory-cli search --query \"{query}\"` to inspect candidates."
                    )
                } else {
                    anyhow!(
                        "No symbol matched query `{query}`. Run `codestory-cli search --query \"{query}\"` to inspect candidates."
                    )
                }
            })?;

            Ok(ResolvedTarget {
                requested: query,
                selected,
                alternatives,
            })
        }
    }
}

fn build_trail_request(root_id: &NodeId, cmd: &TrailCommand) -> TrailConfigDto {
    let mode = match cmd.mode {
        CliTrailMode::Neighborhood => TrailMode::Neighborhood,
        CliTrailMode::Referenced => TrailMode::AllReferenced,
        CliTrailMode::Referencing => TrailMode::AllReferencing,
    };
    let direction = cmd
        .direction
        .map(Into::into)
        .unwrap_or_else(|| default_trail_direction(cmd.mode));

    TrailConfigDto {
        root_id: root_id.clone(),
        mode,
        target_id: None,
        depth: cmd.depth.unwrap_or(match cmd.mode {
            CliTrailMode::Neighborhood => 2,
            CliTrailMode::Referenced | CliTrailMode::Referencing => 0,
        }),
        direction,
        caller_scope: if cmd.include_tests {
            TrailCallerScope::IncludeTestsAndBenches
        } else {
            TrailCallerScope::ProductionOnly
        },
        edge_filter: Vec::new(),
        show_utility_calls: cmd.show_utility_calls,
        node_filter: Vec::new(),
        max_nodes: cmd.max_nodes.clamp(1, 200),
        layout_direction: match cmd.layout {
            CliLayout::Horizontal => LayoutDirection::Horizontal,
            CliLayout::Vertical => LayoutDirection::Vertical,
        },
    }
}

fn canonicalize_project_root(project: &Path) -> Result<PathBuf> {
    let project = if project.is_absolute() {
        project.to_path_buf()
    } else {
        std::env::current_dir()
            .context("Failed to resolve current working directory")?
            .join(project)
    };

    project.canonicalize().with_context(|| {
        format!(
            "Failed to resolve project path `{}`. Ensure the directory exists.",
            project.display()
        )
    })
}

fn cache_root_for_project(project_root: &Path, override_dir: Option<&Path>) -> Result<PathBuf> {
    let base = match override_dir {
        Some(path) => path.to_path_buf(),
        None => ProjectDirs::from("dev", "codestory", "codestory")
            .map(|dirs| dirs.cache_dir().to_path_buf())
            .ok_or_else(|| {
                anyhow!("Failed to determine a user cache directory for codestory-cli")
            })?,
    };
    Ok(base.join(fnv1a_hex(project_root.to_string_lossy().as_bytes())))
}

fn fnv1a_hex(bytes: &[u8]) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

fn resolve_refresh_request(refresh: RefreshMode, summary: &ProjectSummary) -> Option<IndexMode> {
    match refresh {
        RefreshMode::Auto => Some(if summary.stats.node_count == 0 {
            IndexMode::Full
        } else {
            IndexMode::Incremental
        }),
        RefreshMode::Full => Some(IndexMode::Full),
        RefreshMode::Incremental => Some(IndexMode::Incremental),
        RefreshMode::None => None,
    }
}

fn refresh_label(requested: RefreshMode, resolved: Option<IndexMode>) -> String {
    match (requested, resolved) {
        (RefreshMode::Auto, Some(IndexMode::Full)) => "auto(full)".to_string(),
        (RefreshMode::Auto, Some(IndexMode::Incremental)) => "auto(incremental)".to_string(),
        (RefreshMode::Full, Some(_)) => "full".to_string(),
        (RefreshMode::Incremental, Some(_)) => "incremental".to_string(),
        (RefreshMode::None, None) => "none".to_string(),
        _ => "unknown".to_string(),
    }
}

fn ensure_index_ready(opened: &OpenedProject, subcommand: &str) -> Result<()> {
    if opened.summary.stats.node_count == 0 {
        bail!(
            "No indexed files are available for `{subcommand}`. Run `codestory-cli index --project \"{}\" --refresh auto` first or rerun this command with `--refresh auto`.",
            opened.summary.root
        );
    }
    Ok(())
}

fn map_api_error(error: ApiError) -> anyhow::Error {
    anyhow!("{}: {}", error.code, error.message)
}

fn search_hit_from_node(node: &NodeDetailsDto) -> SearchHit {
    SearchHit {
        node_id: node.id.clone(),
        display_name: node.display_name.clone(),
        kind: node.kind,
        file_path: node.file_path.clone(),
        line: node.start_line,
        score: 0.0,
        origin: codestory_api::SearchHitOrigin::IndexedSymbol,
        resolvable: true,
    }
}

fn emit<T: Serialize>(format: OutputFormat, value: &T, markdown: String) -> Result<()> {
    match format {
        OutputFormat::Markdown => {
            println!("{markdown}");
            Ok(())
        }
        OutputFormat::Json => {
            println!(
                "{}",
                serde_json::to_string_pretty(value).context("Failed to serialize JSON output")?
            );
            Ok(())
        }
    }
}

fn render_index_markdown(output: &IndexOutput<'_>) -> String {
    let mut markdown = String::new();
    let _ = writeln!(markdown, "# Index");
    let _ = writeln!(markdown, "project: `{}`", clean_path_string(output.project));
    let _ = writeln!(
        markdown,
        "storage: `{}`",
        clean_path_string(output.storage_path)
    );
    let _ = writeln!(markdown, "refresh: `{}`", output.refresh);
    let _ = writeln!(
        markdown,
        "stats: nodes={} edges={} files={} errors={}",
        output.summary.stats.node_count,
        output.summary.stats.edge_count,
        output.summary.stats.file_count,
        output.summary.stats.error_count
    );
    if let Some(timings) = output.phase_timings {
        let _ = writeln!(
            markdown,
            "timings_ms: parse={} flush={} resolve={} cleanup={} cache_refresh={}",
            timings.parse_index_ms,
            timings.projection_flush_ms,
            timings.edge_resolution_ms,
            timings.cleanup_ms,
            timings.cache_refresh_ms.unwrap_or(0)
        );
        let _ = writeln!(
            markdown,
            "resolution: calls {}->{}, imports {}->{}",
            timings.unresolved_calls_start,
            timings.unresolved_calls_end,
            timings.unresolved_imports_start,
            timings.unresolved_imports_end
        );
        append_optional_timings_line(
            &mut markdown,
            "staged_publish_ms",
            &[
                ("deferred_indexes", timings.deferred_indexes_ms),
                ("summary_snapshot", timings.summary_snapshot_ms),
                ("detail_snapshot", timings.detail_snapshot_ms),
                ("publish", timings.publish_ms),
            ],
        );
        append_optional_timings_line(
            &mut markdown,
            "setup_ms",
            &[
                (
                    "existing_projection_ids",
                    timings.setup_existing_projection_ids_ms,
                ),
                ("seed_symbol_table", timings.setup_seed_symbol_table_ms),
            ],
        );
        append_optional_timings_line(
            &mut markdown,
            "flush_breakdown_ms",
            &[
                ("files", timings.flush_files_ms),
                ("nodes", timings.flush_nodes_ms),
                ("edges", timings.flush_edges_ms),
                ("occurrences", timings.flush_occurrences_ms),
                ("component_access", timings.flush_component_access_ms),
                ("callable_projection", timings.flush_callable_projection_ms),
            ],
        );
        append_optional_timings_line(
            &mut markdown,
            "resolution_ms",
            &[
                ("override_count", timings.resolution_override_count_ms),
                ("unresolved_counts", timings.resolution_unresolved_counts_ms),
                ("calls", timings.resolution_calls_ms),
                ("imports", timings.resolution_imports_ms),
                ("cleanup", timings.resolution_cleanup_ms),
            ],
        );
        append_optional_timings_line(
            &mut markdown,
            "resolution_indexes_ms",
            &[
                ("call_candidate", timings.resolution_call_candidate_index_ms),
                (
                    "import_candidate",
                    timings.resolution_import_candidate_index_ms,
                ),
                ("call_semantic", timings.resolution_call_semantic_index_ms),
                (
                    "import_semantic",
                    timings.resolution_import_semantic_index_ms,
                ),
            ],
        );
        append_optional_timings_line(
            &mut markdown,
            "resolution_detail_ms",
            &[
                (
                    "call_semantic_candidates",
                    timings.resolution_call_semantic_candidates_ms,
                ),
                (
                    "import_semantic_candidates",
                    timings.resolution_import_semantic_candidates_ms,
                ),
                ("call_compute", timings.resolution_call_compute_ms),
                ("import_compute", timings.resolution_import_compute_ms),
                ("call_apply", timings.resolution_call_apply_ms),
                ("import_apply", timings.resolution_import_apply_ms),
                ("overrides", timings.resolution_override_resolution_ms),
            ],
        );
        append_optional_timings_line(
            &mut markdown,
            "resolution_semantic_requests",
            &[
                ("call_rows", timings.resolution_call_semantic_requests),
                (
                    "call_unique",
                    timings.resolution_call_semantic_unique_requests,
                ),
                (
                    "call_skipped",
                    timings.resolution_call_semantic_skipped_requests,
                ),
                ("import_rows", timings.resolution_import_semantic_requests),
                (
                    "import_unique",
                    timings.resolution_import_semantic_unique_requests,
                ),
                (
                    "import_skipped",
                    timings.resolution_import_semantic_skipped_requests,
                ),
            ],
        );
    }
    markdown
}

fn append_optional_timings_line(
    markdown: &mut String,
    label: &str,
    entries: &[(&str, Option<u32>)],
) {
    let rendered = entries
        .iter()
        .filter_map(|(name, value)| value.map(|value| format!("{name}={value}")))
        .collect::<Vec<_>>();
    if rendered.is_empty() {
        return;
    }
    let _ = writeln!(markdown, "{label}: {}", rendered.join(" "));
}

fn render_ground_markdown(project_root: &Path, snapshot: &GroundingSnapshotDto) -> String {
    let mut markdown = String::new();
    let _ = writeln!(markdown, "# Grounding Snapshot");
    let _ = writeln!(markdown, "root: `{}`", clean_path_string(&snapshot.root));
    let _ = writeln!(markdown, "budget: `{}`", format_budget(snapshot.budget));
    let _ = writeln!(
        markdown,
        "coverage: files {}/{} symbols {}/{} compressed_files={}",
        snapshot.coverage.represented_files,
        snapshot.coverage.total_files,
        snapshot.coverage.represented_symbols,
        snapshot.coverage.total_symbols,
        snapshot.coverage.compressed_files
    );
    let _ = writeln!(
        markdown,
        "stats: nodes={} edges={} files={} errors={}",
        snapshot.stats.node_count,
        snapshot.stats.edge_count,
        snapshot.stats.file_count,
        snapshot.stats.error_count
    );
    if !snapshot.recommended_queries.is_empty() {
        let _ = writeln!(
            markdown,
            "recommended_queries: {}",
            snapshot.recommended_queries.join(", ")
        );
    }
    if !snapshot.notes.is_empty() {
        let _ = writeln!(markdown, "notes:");
        for note in &snapshot.notes {
            let _ = writeln!(markdown, "- {note}");
        }
    }
    let _ = writeln!(markdown, "root_symbols:");
    for symbol in &snapshot.root_symbols {
        let _ = writeln!(markdown, "- {}", render_ground_symbol(symbol));
    }
    let _ = writeln!(markdown, "files:");
    for file in &snapshot.files {
        let language = file.language.as_deref().unwrap_or("unknown");
        let status = if file.compressed {
            "compressed"
        } else {
            "full"
        };
        let focus = if file.symbols.is_empty() {
            "no indexed symbols".to_string()
        } else {
            file.symbols
                .iter()
                .map(render_ground_symbol)
                .collect::<Vec<_>>()
                .join(" | ")
        };
        let _ = writeln!(
            markdown,
            "- `{}` [{}] symbols {}/{} {} | {}",
            relative_path(project_root, &file.file_path),
            language,
            file.represented_symbol_count,
            file.symbol_count,
            status,
            focus
        );
    }
    if !snapshot.coverage_buckets.is_empty() {
        let _ = writeln!(markdown, "coverage_buckets:");
        for bucket in &snapshot.coverage_buckets {
            let sample_paths = if bucket.sample_paths.is_empty() {
                "no sample paths".to_string()
            } else {
                bucket.sample_paths.join(", ")
            };
            let _ = writeln!(
                markdown,
                "- `{}` files={} symbols={} samples={}",
                bucket.label, bucket.file_count, bucket.symbol_count, sample_paths
            );
        }
    }
    markdown
}

fn render_search_markdown(project_root: &Path, query: &str, hits: &[SearchHit]) -> String {
    let mut markdown = String::new();
    let _ = writeln!(markdown, "# Search");
    let _ = writeln!(markdown, "query: `{query}`");
    let _ = writeln!(markdown, "hits: {}", hits.len());
    for hit in hits {
        let _ = writeln!(markdown, "- {}", render_search_hit(project_root, hit));
    }
    markdown
}

fn render_symbol_markdown(
    project_root: &Path,
    target: &ResolvedTarget,
    context: &SymbolContextDto,
) -> String {
    let mut markdown = String::new();
    let _ = writeln!(markdown, "# Symbol");
    append_resolution(&mut markdown, project_root, target);
    let _ = writeln!(
        markdown,
        "focus: {}",
        render_node(project_root, &context.node)
    );
    let _ = writeln!(markdown, "children: {}", context.children.len());
    for child in &context.children {
        let _ = writeln!(
            markdown,
            "- [{}] {} [{}]{}",
            child.id.0,
            child.label,
            format_kind(child.kind),
            if child.has_children { " children" } else { "" }
        );
    }
    if !context.edge_digest.is_empty() {
        let _ = writeln!(markdown, "edge_digest:");
        for edge in &context.edge_digest {
            let _ = writeln!(markdown, "- {edge}");
        }
    }
    if !context.related_hits.is_empty() {
        let _ = writeln!(markdown, "related_hits:");
        for hit in &context.related_hits {
            let _ = writeln!(markdown, "- {}", render_search_hit(project_root, hit));
        }
    }
    markdown
}

fn render_trail_markdown(
    project_root: &Path,
    target: &ResolvedTarget,
    context: &TrailContextDto,
    cmd: &TrailCommand,
) -> String {
    let mut markdown = String::new();
    let _ = writeln!(markdown, "# Trail");
    append_resolution(&mut markdown, project_root, target);
    let _ = writeln!(
        markdown,
        "focus: {}",
        render_node(project_root, &context.focus)
    );
    let _ = writeln!(
        markdown,
        "mode: {} direction: {} depth: {} nodes: {} edges: {} truncated: {}",
        format_trail_mode(cmd.mode),
        format_direction(
            cmd.direction
                .map(Into::into)
                .unwrap_or_else(|| default_trail_direction(cmd.mode))
        ),
        cmd.depth.unwrap_or(match cmd.mode {
            CliTrailMode::Neighborhood => 2,
            CliTrailMode::Referenced | CliTrailMode::Referencing => 0,
        }),
        context.trail.nodes.len(),
        context.trail.edges.len(),
        context.trail.truncated
    );

    let labels = context
        .trail
        .nodes
        .iter()
        .map(|node| (node.id.clone(), node.label.clone()))
        .collect::<HashMap<_, _>>();

    let _ = writeln!(markdown, "nodes:");
    for node in &context.trail.nodes {
        let file = node
            .file_path
            .as_deref()
            .map(|value| relative_path(project_root, value))
            .unwrap_or_else(|| "-".to_string());
        let _ = writeln!(
            markdown,
            "- [{}] {} [{}] depth={} file={}",
            node.id.0,
            node.label,
            format_kind(node.kind),
            node.depth,
            file
        );
    }

    let _ = writeln!(markdown, "edges:");
    for edge in &context.trail.edges {
        let source = labels
            .get(&edge.source)
            .map(String::as_str)
            .unwrap_or(&edge.source.0);
        let target = labels
            .get(&edge.target)
            .map(String::as_str)
            .unwrap_or(&edge.target.0);
        let certainty = edge
            .certainty
            .as_deref()
            .map(|value| format!(" certainty={value}"))
            .unwrap_or_default();
        let _ = writeln!(
            markdown,
            "- [{}] {} -{}-> {}{}",
            edge.id.0,
            source,
            format!("{:?}", edge.kind).to_lowercase(),
            target,
            certainty
        );
    }
    markdown
}

fn render_snippet_markdown(
    project_root: &Path,
    target: &ResolvedTarget,
    context: &SnippetContextDto,
) -> String {
    let mut markdown = String::new();
    let _ = writeln!(markdown, "# Snippet");
    append_resolution(&mut markdown, project_root, target);
    let _ = writeln!(
        markdown,
        "focus: {}",
        render_node(project_root, &context.node)
    );
    let _ = writeln!(
        markdown,
        "path: `{}`:{}",
        relative_path(project_root, &context.path),
        context.line
    );
    let _ = writeln!(markdown, "{}", context.snippet);
    markdown
}

fn append_resolution(markdown: &mut String, project_root: &Path, target: &ResolvedTarget) {
    if target.requested.starts_with("id:") {
        return;
    }
    let _ = writeln!(
        markdown,
        "resolved_query: `{}` -> {}",
        target.requested,
        render_search_hit(project_root, &target.selected)
    );
    if target.alternatives.len() > 1 {
        let alternatives = target
            .alternatives
            .iter()
            .skip(1)
            .take(3)
            .map(|hit| render_search_hit(project_root, hit))
            .collect::<Vec<_>>();
        if !alternatives.is_empty() {
            let _ = writeln!(markdown, "alternate_hits:");
            for hit in alternatives {
                let _ = writeln!(markdown, "- {hit}");
            }
        }
    }
}

fn render_node(project_root: &Path, node: &NodeDetailsDto) -> String {
    let mut out = format!(
        "[{}] {} [{}]",
        node.id.0,
        node.display_name,
        format_kind(node.kind)
    );
    if let Some(path) = node.file_path.as_deref() {
        let _ = write!(out, " {}", relative_path(project_root, path));
    }
    if let Some(line) = node.start_line {
        let _ = write!(out, ":{line}");
    }
    out
}

fn render_search_hit(project_root: &Path, hit: &SearchHit) -> String {
    let mut out = format!(
        "[{}] {} [{}]",
        hit.node_id.0,
        hit.display_name,
        format_kind(hit.kind)
    );
    if let Some(path) = hit.file_path.as_deref() {
        let _ = write!(out, " {}", relative_path(project_root, path));
    }
    if let Some(line) = hit.line {
        let _ = write!(out, ":{line}");
    }
    let _ = write!(out, " score={:.2}", hit.score);
    out
}

fn render_ground_symbol(symbol: &codestory_api::GroundingSymbolDigestDto) -> String {
    let mut out = format!(
        "[{}] {} [{}]",
        symbol.id.0,
        symbol.label,
        format_kind(symbol.kind)
    );
    if let Some(line) = symbol.line {
        let _ = write!(out, " line={line}");
    }
    if let Some(member_count) = symbol.member_count {
        let _ = write!(out, " members={member_count}");
    }
    if !symbol.edge_digest.is_empty() {
        let _ = write!(out, " edges={}", symbol.edge_digest.join("; "));
    }
    out
}

fn clean_path_string(path: &str) -> String {
    let mut stringified = path.replace('\\', "/");
    if stringified.starts_with("//?/") {
        stringified = stringified[4..].to_string();
    }
    stringified
}

fn relative_path(project_root: &Path, raw: &str) -> String {
    let path = Path::new(raw);
    let relative = path
        .strip_prefix(project_root)
        .unwrap_or(path)
        .to_string_lossy();
    clean_path_string(&relative)
}

fn looks_like_text_query(query: &str) -> bool {
    let word_count = query.split_whitespace().count();
    let has_text_punctuation = query
        .chars()
        .any(|ch| matches!(ch, '.' | ',' | ':' | ';' | '!' | '?' | '"' | '\''));
    (word_count > 1 && has_text_punctuation) || query.len() > 28
}

fn scan_repo_text_hits(project_root: &Path, query: &str, limit: usize) -> Result<Vec<SearchHit>> {
    let mut hits = Vec::new();
    if query.trim().is_empty() || limit == 0 {
        return Ok(hits);
    }
    scan_repo_text_hits_inner(project_root, project_root, query, limit, &mut hits)?;
    Ok(hits)
}

fn scan_repo_text_hits_inner(
    project_root: &Path,
    dir: &Path,
    query: &str,
    limit: usize,
    hits: &mut Vec<SearchHit>,
) -> Result<()> {
    if hits.len() >= limit {
        return Ok(());
    }

    for entry in
        fs::read_dir(dir).with_context(|| format!("Failed to read directory {}", dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            if is_ignored_search_dir(&path) {
                continue;
            }
            scan_repo_text_hits_inner(project_root, &path, query, limit, hits)?;
            if hits.len() >= limit {
                break;
            }
            continue;
        }

        if let Some(hit) = scan_file_text_hit(project_root, &path, query, hits.len()) {
            hits.push(hit);
            if hits.len() >= limit {
                break;
            }
        }
    }

    Ok(())
}

fn is_ignored_search_dir(path: &Path) -> bool {
    matches!(
        path.file_name().and_then(|name| name.to_str()),
        Some(".git" | "target" | "node_modules" | ".next" | "dist")
    )
}

fn scan_file_text_hit(
    project_root: &Path,
    path: &Path,
    query: &str,
    rank: usize,
) -> Option<SearchHit> {
    let metadata = path.metadata().ok()?;
    if metadata.len() > 1_000_000 {
        return None;
    }

    let contents = fs::read_to_string(path).ok()?;
    let normalized_query = query.trim().to_ascii_lowercase();
    if normalized_query.is_empty() {
        return None;
    }

    let mut line_match = None;
    for (index, line) in contents.lines().enumerate() {
        if line.to_ascii_lowercase().contains(&normalized_query) {
            line_match = Some((index + 1).min(u32::MAX as usize) as u32);
            break;
        }
    }
    let line = line_match?;
    let relative = path
        .strip_prefix(project_root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/");
    let hash_hex = fnv1a_hex(format!("{relative}:{line}").as_bytes());
    let node_id_raw = i64::from_str_radix(&hash_hex[..15], 16).ok()?;

    Some(SearchHit {
        node_id: NodeId(node_id_raw.to_string()),
        display_name: relative.clone(),
        kind: codestory_api::NodeKind::FILE,
        file_path: Some(path.to_string_lossy().to_string()),
        line: Some(line),
        score: 500.0 - rank as f32,
        origin: codestory_api::SearchHitOrigin::TextMatch,
        resolvable: false,
    })
}

fn format_kind(kind: codestory_api::NodeKind) -> String {
    format!("{kind:?}").to_lowercase()
}

fn format_budget(budget: GroundingBudgetDto) -> &'static str {
    match budget {
        GroundingBudgetDto::Strict => "strict",
        GroundingBudgetDto::Balanced => "balanced",
        GroundingBudgetDto::Max => "max",
    }
}

fn format_trail_mode(mode: CliTrailMode) -> &'static str {
    match mode {
        CliTrailMode::Neighborhood => "neighborhood",
        CliTrailMode::Referenced => "referenced",
        CliTrailMode::Referencing => "referencing",
    }
}

fn format_direction(direction: TrailDirection) -> &'static str {
    match direction {
        TrailDirection::Incoming => "incoming",
        TrailDirection::Outgoing => "outgoing",
        TrailDirection::Both => "both",
    }
}

fn default_trail_direction(mode: CliTrailMode) -> TrailDirection {
    match mode {
        CliTrailMode::Neighborhood => TrailDirection::Both,
        CliTrailMode::Referenced => TrailDirection::Outgoing,
        CliTrailMode::Referencing => TrailDirection::Incoming,
    }
}

impl From<CliGroundingBudget> for GroundingBudgetDto {
    fn from(value: CliGroundingBudget) -> Self {
        match value {
            CliGroundingBudget::Strict => Self::Strict,
            CliGroundingBudget::Balanced => Self::Balanced,
            CliGroundingBudget::Max => Self::Max,
        }
    }
}

impl From<CliDirection> for TrailDirection {
    fn from(value: CliDirection) -> Self {
        match value {
            CliDirection::Incoming => Self::Incoming,
            CliDirection::Outgoing => Self::Outgoing,
            CliDirection::Both => Self::Both,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codestory_api::{IndexingPhaseTimings, StorageStatsDto};
    use std::fs;
    use tempfile::tempdir;

    fn summary_with_files(file_count: u32) -> ProjectSummary {
        ProjectSummary {
            root: "C:/repo".to_string(),
            stats: StorageStatsDto {
                node_count: file_count.saturating_mul(10),
                edge_count: 0,
                file_count,
                error_count: 0,
            },
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
        let output = IndexOutput {
            project: &summary.root,
            storage_path: "C:/repo/.cache/index.sqlite",
            refresh: "full",
            summary: &summary,
            phase_timings: Some(&timings),
        };

        let markdown = render_index_markdown(&output);

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

    fn copy_tictactoe_workspace() -> tempfile::TempDir {
        let temp = tempdir().expect("create temp dir");
        let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("workspace crates dir")
            .join("codestory-index")
            .join("tests")
            .join("fixtures")
            .join("tictactoe");

        for entry in fs::read_dir(&fixtures).expect("read fixtures") {
            let entry = entry.expect("fixture entry");
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let target = temp.path().join(entry.file_name());
            fs::copy(&path, &target).expect("copy fixture");
        }

        temp
    }

    fn indexed_runtime(project_root: &Path) -> RuntimeContext {
        let cache_dir = project_root.join(".cache");
        let args = ProjectArgs {
            project: project_root.to_path_buf(),
            cache_dir: Some(cache_dir),
        };
        let runtime = RuntimeContext::new(&args).expect("runtime");
        let opened = runtime
            .ensure_open(RefreshMode::Full)
            .expect("index project");
        ensure_index_ready(&opened, "test").expect("indexed project");
        runtime
    }

    #[test]
    fn ground_open_preserves_current_auto_refresh_semantics() {
        let temp = copy_tictactoe_workspace();
        let runtime = indexed_runtime(temp.path());

        let opened = runtime
            .ensure_ground_open(RefreshMode::Auto)
            .expect("ground open");

        assert_eq!(opened.refresh_mode, Some(IndexMode::Incremental));
        ensure_index_ready(&opened, "ground").expect("ground ready");
    }

    #[test]
    fn resolution_prefers_exact_type_name_over_member_hits() {
        let query = "AppController";
        let mut hits = vec![
            SearchHit {
                node_id: NodeId("2".to_string()),
                display_name: "AppController::open_project".to_string(),
                kind: codestory_api::NodeKind::FUNCTION,
                file_path: None,
                line: None,
                score: 0.9,
                origin: codestory_api::SearchHitOrigin::IndexedSymbol,
                resolvable: true,
            },
            SearchHit {
                node_id: NodeId("1".to_string()),
                display_name: "AppController".to_string(),
                kind: codestory_api::NodeKind::CLASS,
                file_path: None,
                line: None,
                score: 0.9,
                origin: codestory_api::SearchHitOrigin::IndexedSymbol,
                resolvable: true,
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
        let mut hits = vec![
            SearchHit {
                node_id: NodeId("2".to_string()),
                display_name: "AppController".to_string(),
                kind: codestory_api::NodeKind::CLASS,
                file_path: Some(file_path.to_string_lossy().to_string()),
                line: Some(2),
                score: 1.0,
                origin: codestory_api::SearchHitOrigin::IndexedSymbol,
                resolvable: true,
            },
            SearchHit {
                node_id: NodeId("1".to_string()),
                display_name: "AppController".to_string(),
                kind: codestory_api::NodeKind::STRUCT,
                file_path: Some(file_path.to_string_lossy().to_string()),
                line: Some(1),
                score: 1.0,
                origin: codestory_api::SearchHitOrigin::IndexedSymbol,
                resolvable: true,
            },
        ];

        hits.sort_by(|left, right| compare_resolution_hits(query, left, right));
        assert_eq!(hits[0].line, Some(1));
        assert_eq!(hits[0].kind, codestory_api::NodeKind::STRUCT);
    }

    #[test]
    fn resolution_prefers_callable_definitions_over_unknown_hits() {
        let query = "check_winner";
        let mut hits = vec![
            SearchHit {
                node_id: NodeId("2".to_string()),
                display_name: "check_winner".to_string(),
                kind: codestory_api::NodeKind::UNKNOWN,
                file_path: Some("src/callsite.rs".to_string()),
                line: Some(20),
                score: 0.9,
                origin: codestory_api::SearchHitOrigin::IndexedSymbol,
                resolvable: true,
            },
            SearchHit {
                node_id: NodeId("1".to_string()),
                display_name: "check_winner".to_string(),
                kind: codestory_api::NodeKind::FUNCTION,
                file_path: Some("src/game.rs".to_string()),
                line: Some(10),
                score: 0.8,
                origin: codestory_api::SearchHitOrigin::IndexedSymbol,
                resolvable: true,
            },
        ];

        hits.sort_by(|left, right| compare_resolution_hits(query, left, right));
        assert_eq!(hits[0].kind, codestory_api::NodeKind::FUNCTION);
    }

    #[test]
    fn resolve_target_file_filter_disambiguates_tictactoe_symbol_queries() {
        let workspace = copy_tictactoe_workspace();
        let runtime = indexed_runtime(workspace.path());

        let resolved = resolve_target(
            &runtime,
            TargetSelection::Query("TicTacToe".to_string()),
            Some("rust_tictactoe.rs"),
        )
        .expect("resolve filtered target");

        assert!(
            resolved
                .selected
                .file_path
                .as_deref()
                .is_some_and(|path| path.contains("rust_tictactoe.rs"))
        );
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
            "UNC/server/share"
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
}
