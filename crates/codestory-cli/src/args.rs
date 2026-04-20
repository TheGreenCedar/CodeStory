use clap::{ArgGroup, Args, Parser, Subcommand, ValueEnum};
use codestory_contracts::api::{
    GroundingBudgetDto, IndexDryRunDto, IndexingPhaseTimings, LayoutDirection, NodeId, NodeKind,
    ProjectSummary, RetrievalStateDto, SearchHitOrigin, SnippetContextDto, SummaryGenerationDto,
    SymbolContextDto, TrailCallerScope, TrailContextDto, TrailDirection, TrailMode,
};
use serde::Serialize;
use std::path::PathBuf;

const INDEX_REFRESH_HELP: &str = "Index defaults to `auto`: it chooses `full` for an empty cache and `incremental` once the \
cache already has indexed files.";
const READ_REFRESH_HELP: &str = "Read commands default to `none` so they only query the existing cache. Use `incremental` to \
refresh an existing cache in place, or `full` after a cache reset, schema change, or indexing \
failure.";

#[derive(Parser, Debug)]
#[command(author, version, about = "Skill-first repo grounding runtime", long_about = None)]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Command,
}

#[derive(Subcommand, Debug)]
pub(crate) enum Command {
    Index(IndexCommand),
    Ground(GroundCommand),
    Search(SearchCommand),
    Symbol(SymbolCommand),
    Trail(TrailCommand),
    Snippet(SnippetCommand),
    Query(QueryCommand),
    Explore(ExploreCommand),
    Serve(ServeCommand),
    GenerateCompletions(GenerateCompletionsCommand),
}

#[derive(Args, Debug, Clone)]
pub(crate) struct ProjectArgs {
    #[arg(
        long,
        alias = "path",
        default_value = ".",
        help = "Repository root to index or query."
    )]
    pub(crate) project: PathBuf,
    #[arg(
        long,
        help = "Cache directory to use exactly as passed. If omitted, codestory-cli uses the system cache root with a per-project hashed subdirectory."
    )]
    pub(crate) cache_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum OutputFormat {
    Markdown,
    Json,
    Dot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum RefreshMode {
    Auto,
    Full,
    Incremental,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RepoTextMode {
    Auto,
    On,
    Off,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum CliGroundingBudget {
    Strict,
    Balanced,
    Max,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum CliTrailMode {
    Neighborhood,
    Referenced,
    Referencing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum CliDirection {
    Incoming,
    Outgoing,
    Both,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum CliLayout {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum CompletionShell {
    Bash,
    Zsh,
    Fish,
    Powershell,
}

#[derive(Args, Debug)]
pub(crate) struct IndexCommand {
    #[command(flatten)]
    pub(crate) project: ProjectArgs,
    #[arg(
        long,
        value_enum,
        default_value_t = RefreshMode::Auto,
        long_help = INDEX_REFRESH_HELP
    )]
    pub(crate) refresh: RefreshMode,
    #[arg(long, value_enum, default_value_t = OutputFormat::Markdown)]
    pub(crate) format: OutputFormat,
    #[arg(
        long,
        value_name = "PATH",
        help = "Write command output to this file instead of stdout. The parent directory must already exist."
    )]
    pub(crate) output_file: Option<PathBuf>,
    #[arg(
        long,
        help = "Compute the refresh plan without parsing files or writing storage."
    )]
    pub(crate) dry_run: bool,
    #[arg(long, help = "Generate cached symbol summaries after indexing.")]
    pub(crate) summarize: bool,
    #[arg(long, help = "Print indexing progress to stderr.")]
    pub(crate) progress: bool,
    #[arg(
        long,
        help = "Keep running and incrementally re-index after file changes."
    )]
    pub(crate) watch: bool,
}

#[derive(Args, Debug)]
pub(crate) struct GroundCommand {
    #[command(flatten)]
    pub(crate) project: ProjectArgs,
    #[arg(long, value_enum, default_value_t = CliGroundingBudget::Balanced)]
    pub(crate) budget: CliGroundingBudget,
    #[arg(
        long,
        value_enum,
        default_value_t = RefreshMode::None,
        long_help = READ_REFRESH_HELP
    )]
    pub(crate) refresh: RefreshMode,
    #[arg(long, value_enum, default_value_t = OutputFormat::Markdown)]
    pub(crate) format: OutputFormat,
    #[arg(
        long,
        value_name = "PATH",
        help = "Write command output to this file instead of stdout. The parent directory must already exist."
    )]
    pub(crate) output_file: Option<PathBuf>,
}

#[derive(Args, Debug)]
pub(crate) struct SearchCommand {
    #[command(flatten)]
    pub(crate) project: ProjectArgs,
    #[arg(long)]
    pub(crate) query: String,
    #[arg(
        long,
        default_value_t = 10,
        help = "Maximum results per provenance group (`indexed_symbol` and `text_match`)."
    )]
    pub(crate) limit: u32,
    #[arg(
        long,
        value_enum,
        default_value_t = RepoTextMode::Auto,
        help = "Whether to scan repo text in addition to indexed symbols: `auto` enables it for natural-language queries, `on` always scans repo text, and `off` keeps the search index only."
    )]
    pub(crate) repo_text: RepoTextMode,
    #[arg(
        long,
        value_enum,
        default_value_t = RefreshMode::None,
        long_help = READ_REFRESH_HELP
    )]
    pub(crate) refresh: RefreshMode,
    #[arg(long, value_enum, default_value_t = OutputFormat::Markdown)]
    pub(crate) format: OutputFormat,
    #[arg(
        long,
        value_name = "PATH",
        help = "Write command output to this file instead of stdout. The parent directory must already exist."
    )]
    pub(crate) output_file: Option<PathBuf>,
    #[arg(
        long = "hybrid-lexical",
        value_name = "WEIGHT",
        help = "Override the lexical component weight for hybrid search research runs."
    )]
    pub(crate) hybrid_lexical: Option<f32>,
    #[arg(
        long = "hybrid-semantic",
        value_name = "WEIGHT",
        help = "Override the semantic component weight for hybrid search research runs."
    )]
    pub(crate) hybrid_semantic: Option<f32>,
    #[arg(
        long = "hybrid-graph",
        value_name = "WEIGHT",
        help = "Override the graph component weight for hybrid search research runs."
    )]
    pub(crate) hybrid_graph: Option<f32>,
}

#[derive(Args, Debug, Clone)]
#[command(group(
    ArgGroup::new("selector")
        .args(["id", "query"])
        .required(true)
        .multiple(false)
))]
pub(crate) struct TargetArgs {
    #[arg(
        long,
        group = "selector",
        help = "Resolve the target from an exact node id returned by `search`, `symbol`, or `trail`."
    )]
    pub(crate) id: Option<String>,
    #[arg(
        long,
        group = "selector",
        help = "Resolve the target from a symbol query. Required if you also pass `--file`."
    )]
    pub(crate) query: Option<String>,
    #[arg(
        long,
        requires = "query",
        help = "Limit query resolution to files whose path matches this fragment."
    )]
    pub(crate) file: Option<String>,
}

#[derive(Args, Debug)]
pub(crate) struct SymbolCommand {
    #[command(flatten)]
    pub(crate) project: ProjectArgs,
    #[command(flatten)]
    pub(crate) target: TargetArgs,
    #[arg(
        long,
        value_enum,
        default_value_t = RefreshMode::None,
        long_help = READ_REFRESH_HELP
    )]
    pub(crate) refresh: RefreshMode,
    #[arg(long, value_enum, default_value_t = OutputFormat::Markdown)]
    pub(crate) format: OutputFormat,
    #[arg(
        long,
        value_name = "PATH",
        help = "Write command output to this file instead of stdout. The parent directory must already exist."
    )]
    pub(crate) output_file: Option<PathBuf>,
    #[arg(long, help = "Render a Mermaid graph instead of Markdown/JSON output.")]
    pub(crate) mermaid: bool,
}

#[derive(Args, Debug)]
pub(crate) struct TrailCommand {
    #[command(flatten)]
    pub(crate) project: ProjectArgs,
    #[command(flatten)]
    pub(crate) target: TargetArgs,
    #[arg(long, value_enum, default_value_t = CliTrailMode::Neighborhood)]
    pub(crate) mode: CliTrailMode,
    #[arg(long)]
    pub(crate) depth: Option<u32>,
    #[arg(long, value_enum)]
    pub(crate) direction: Option<CliDirection>,
    #[arg(long, default_value_t = 24)]
    pub(crate) max_nodes: u32,
    #[arg(long)]
    pub(crate) include_tests: bool,
    #[arg(long)]
    pub(crate) show_utility_calls: bool,
    #[arg(
        long,
        help = "Hide uncertain/speculative edges and remove nodes disconnected from the trail focus."
    )]
    pub(crate) hide_speculative: bool,
    #[arg(long, value_enum, default_value_t = CliLayout::Horizontal)]
    pub(crate) layout: CliLayout,
    #[arg(
        long,
        value_enum,
        default_value_t = RefreshMode::None,
        long_help = READ_REFRESH_HELP
    )]
    pub(crate) refresh: RefreshMode,
    #[arg(long, value_enum, default_value_t = OutputFormat::Markdown)]
    pub(crate) format: OutputFormat,
    #[arg(
        long,
        value_name = "PATH",
        help = "Write command output to this file instead of stdout. The parent directory must already exist."
    )]
    pub(crate) output_file: Option<PathBuf>,
    #[arg(long, help = "Render a Mermaid graph instead of Markdown/JSON output.")]
    pub(crate) mermaid: bool,
}

#[derive(Args, Debug)]
pub(crate) struct SnippetCommand {
    #[command(flatten)]
    pub(crate) project: ProjectArgs,
    #[command(flatten)]
    pub(crate) target: TargetArgs,
    #[arg(long, default_value_t = 4)]
    pub(crate) context: usize,
    #[arg(
        long,
        value_enum,
        default_value_t = RefreshMode::None,
        long_help = READ_REFRESH_HELP
    )]
    pub(crate) refresh: RefreshMode,
    #[arg(long, value_enum, default_value_t = OutputFormat::Markdown)]
    pub(crate) format: OutputFormat,
    #[arg(
        long,
        value_name = "PATH",
        help = "Write command output to this file instead of stdout. The parent directory must already exist."
    )]
    pub(crate) output_file: Option<PathBuf>,
}

#[derive(Args, Debug)]
pub(crate) struct QueryCommand {
    #[command(flatten)]
    pub(crate) project: ProjectArgs,
    #[arg(value_name = "QUERY")]
    pub(crate) query: String,
    #[arg(
        long,
        value_enum,
        default_value_t = RefreshMode::None,
        long_help = READ_REFRESH_HELP
    )]
    pub(crate) refresh: RefreshMode,
    #[arg(long, value_enum, default_value_t = OutputFormat::Markdown)]
    pub(crate) format: OutputFormat,
    #[arg(
        long,
        value_name = "PATH",
        help = "Write command output to this file instead of stdout. The parent directory must already exist."
    )]
    pub(crate) output_file: Option<PathBuf>,
}

#[derive(Args, Debug)]
pub(crate) struct ExploreCommand {
    #[command(flatten)]
    pub(crate) project: ProjectArgs,
    #[command(flatten)]
    pub(crate) target: TargetArgs,
    #[arg(long, default_value_t = 2)]
    pub(crate) depth: u32,
    #[arg(long, default_value_t = 18)]
    pub(crate) max_nodes: u32,
    #[arg(
        long,
        help = "Print plain Markdown instead of opening the terminal explorer when stdout is interactive."
    )]
    pub(crate) no_tui: bool,
    #[arg(
        long,
        value_enum,
        default_value_t = RefreshMode::None,
        long_help = READ_REFRESH_HELP
    )]
    pub(crate) refresh: RefreshMode,
    #[arg(long, value_enum, default_value_t = OutputFormat::Markdown)]
    pub(crate) format: OutputFormat,
    #[arg(
        long,
        value_name = "PATH",
        help = "Write command output to this file instead of stdout. The parent directory must already exist."
    )]
    pub(crate) output_file: Option<PathBuf>,
}

#[derive(Args, Debug)]
pub(crate) struct ServeCommand {
    #[command(flatten)]
    pub(crate) project: ProjectArgs,
    #[arg(long, default_value = "127.0.0.1:3917")]
    pub(crate) addr: String,
    #[arg(
        long,
        help = "Serve a small MCP-style JSON-lines protocol over stdio instead of HTTP."
    )]
    pub(crate) stdio: bool,
    #[arg(
        long,
        value_enum,
        default_value_t = RefreshMode::None,
        long_help = READ_REFRESH_HELP
    )]
    pub(crate) refresh: RefreshMode,
}

#[derive(Args, Debug)]
pub(crate) struct GenerateCompletionsCommand {
    #[arg(long, value_enum)]
    pub(crate) shell: CompletionShell,
}

#[derive(Debug, Serialize)]
pub(crate) struct IndexOutput<'a> {
    pub(crate) project: &'a str,
    pub(crate) storage_path: &'a str,
    pub(crate) refresh: &'a str,
    pub(crate) summary: &'a ProjectSummary,
    pub(crate) retrieval: &'a RetrievalStateDto,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) phase_timings: Option<&'a IndexingPhaseTimings>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) summary_generation: Option<&'a SummaryGenerationDto>,
}

#[derive(Debug, Serialize)]
pub(crate) struct IndexDryRunOutput<'a> {
    pub(crate) dry_run: &'a IndexDryRunDto,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum QuerySelectorOutput {
    Id,
    Query,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct SearchHitOutput {
    pub(crate) node_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) node_ref: Option<String>,
    pub(crate) display_name: String,
    pub(crate) kind: NodeKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) file_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) line: Option<u32>,
    pub(crate) score: f32,
    pub(crate) origin: SearchHitOrigin,
    pub(crate) resolvable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) duplicate_of: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) excerpt: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct SearchOutput {
    pub(crate) query: String,
    pub(crate) retrieval: RetrievalStateDto,
    pub(crate) limit_per_source: u32,
    pub(crate) repo_text_mode: RepoTextMode,
    pub(crate) repo_text_enabled: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) suggestions: Vec<SearchHitOutput>,
    pub(crate) indexed_symbol_hits: Vec<SearchHitOutput>,
    pub(crate) repo_text_hits: Vec<SearchHitOutput>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct QueryResolutionOutput {
    pub(crate) selector: QuerySelectorOutput,
    pub(crate) requested: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) file_filter: Option<String>,
    pub(crate) resolved: SearchHitOutput,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) alternatives: Vec<SearchHitOutput>,
}

#[derive(Debug, Serialize)]
pub(crate) struct SymbolJsonOutput<'a> {
    pub(crate) resolution: QueryResolutionOutput,
    pub(crate) symbol: &'a SymbolContextDto,
}

#[derive(Debug, Serialize)]
pub(crate) struct TrailJsonOutput<'a> {
    pub(crate) resolution: QueryResolutionOutput,
    pub(crate) trail: &'a TrailContextDto,
}

#[derive(Debug, Serialize)]
pub(crate) struct SnippetJsonOutput<'a> {
    pub(crate) resolution: QueryResolutionOutput,
    pub(crate) snippet: &'a SnippetContextDto,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct QueryItemOutput {
    pub(crate) node_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) node_ref: Option<String>,
    pub(crate) display_name: String,
    pub(crate) kind: NodeKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) file_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) line: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) depth: Option<u32>,
    pub(crate) source: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct QueryOutput {
    pub(crate) query: String,
    pub(crate) ast: codestory_contracts::query::GraphQueryAst,
    pub(crate) items: Vec<QueryItemOutput>,
}

#[derive(Debug, Serialize)]
pub(crate) struct ExploreOutput<'a> {
    pub(crate) resolution: QueryResolutionOutput,
    pub(crate) symbol: &'a SymbolContextDto,
    pub(crate) trail: &'a TrailContextDto,
    pub(crate) snippet: Option<&'a SnippetContextDto>,
}

#[derive(Debug)]
pub(crate) enum TargetSelection {
    Id(NodeId),
    Query(String),
}

impl TargetArgs {
    pub(crate) fn selection(&self) -> anyhow::Result<TargetSelection> {
        match (&self.id, &self.query) {
            (Some(id), None) => Ok(TargetSelection::Id(NodeId(id.trim().to_string()))),
            (None, Some(query)) if !query.trim().is_empty() => {
                Ok(TargetSelection::Query(query.trim().to_string()))
            }
            (Some(_), Some(_)) => anyhow::bail!("Pass only one of --id or --query."),
            (None, None) => anyhow::bail!("Pass either --id or --query."),
            (None, Some(_)) => anyhow::bail!("--query cannot be empty."),
        }
    }

    pub(crate) fn file_filter(&self) -> Option<String> {
        self.file
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
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

pub(crate) fn build_trail_request(
    root_id: &NodeId,
    cmd: &TrailCommand,
) -> codestory_contracts::api::TrailConfigDto {
    build_trail_request_impl(root_id, cmd)
}

fn build_trail_request_impl(
    root_id: &NodeId,
    cmd: &TrailCommand,
) -> codestory_contracts::api::TrailConfigDto {
    let mode = match cmd.mode {
        CliTrailMode::Neighborhood => TrailMode::Neighborhood,
        CliTrailMode::Referenced => TrailMode::AllReferenced,
        CliTrailMode::Referencing => TrailMode::AllReferencing,
    };
    let direction = cmd
        .direction
        .map(Into::into)
        .unwrap_or_else(|| default_trail_direction(cmd.mode));

    codestory_contracts::api::TrailConfigDto {
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

pub(crate) fn default_trail_direction(mode: CliTrailMode) -> TrailDirection {
    match mode {
        CliTrailMode::Neighborhood => TrailDirection::Both,
        CliTrailMode::Referenced => TrailDirection::Outgoing,
        CliTrailMode::Referencing => TrailDirection::Incoming,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    fn render_subcommand_help(name: &str) -> String {
        let mut command = Cli::command();
        let subcommand = command
            .find_subcommand_mut(name)
            .expect("subcommand should exist");
        subcommand.render_long_help().to_string()
    }

    #[test]
    fn symbol_help_requires_exactly_one_selector() {
        let help = render_subcommand_help("symbol");
        assert!(help.contains("--id <ID>"));
        assert!(help.contains("--query <QUERY>"));
        assert!(help.contains("--file <FILE>"));
        assert!(
            help.contains("<--id <ID>|--query <QUERY>>"),
            "selector help should surface the required selector group in the usage line: {help}"
        );
    }

    #[test]
    fn read_commands_explain_refresh_none_default() {
        for name in ["ground", "search", "symbol", "trail", "snippet"] {
            let help = render_subcommand_help(name);
            assert!(
                help.contains("Read commands default to `none`"),
                "{name} help should explain refresh semantics"
            );
        }
    }

    #[test]
    fn search_help_explains_repo_text_modes() {
        let help = render_subcommand_help("search");
        assert!(help.contains("--repo-text <REPO_TEXT>"));
        assert!(help.contains("auto"));
        assert!(help.contains("on"));
        assert!(help.contains("off"));
    }
}
