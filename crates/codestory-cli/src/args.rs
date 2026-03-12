use clap::{Args, Parser, Subcommand, ValueEnum};
use codestory_contracts::api::{
    GroundingBudgetDto, IndexingPhaseTimings, LayoutDirection, NodeId, ProjectSummary,
    TrailCallerScope, TrailDirection, TrailMode,
};
use serde::Serialize;
use std::path::PathBuf;

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
}

#[derive(Args, Debug, Clone)]
pub(crate) struct ProjectArgs {
    #[arg(long, alias = "path", default_value = ".")]
    pub(crate) project: PathBuf,
    #[arg(long)]
    pub(crate) cache_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum OutputFormat {
    Markdown,
    Json,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum RefreshMode {
    Auto,
    Full,
    Incremental,
    None,
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

#[derive(Args, Debug)]
pub(crate) struct IndexCommand {
    #[command(flatten)]
    pub(crate) project: ProjectArgs,
    #[arg(long, value_enum, default_value_t = RefreshMode::Auto)]
    pub(crate) refresh: RefreshMode,
    #[arg(long, value_enum, default_value_t = OutputFormat::Markdown)]
    pub(crate) format: OutputFormat,
}

#[derive(Args, Debug)]
pub(crate) struct GroundCommand {
    #[command(flatten)]
    pub(crate) project: ProjectArgs,
    #[arg(long, value_enum, default_value_t = CliGroundingBudget::Balanced)]
    pub(crate) budget: CliGroundingBudget,
    #[arg(long, value_enum, default_value_t = RefreshMode::Auto)]
    pub(crate) refresh: RefreshMode,
    #[arg(long, value_enum, default_value_t = OutputFormat::Markdown)]
    pub(crate) format: OutputFormat,
}

#[derive(Args, Debug)]
pub(crate) struct SearchCommand {
    #[command(flatten)]
    pub(crate) project: ProjectArgs,
    #[arg(long)]
    pub(crate) query: String,
    #[arg(long, default_value_t = 10)]
    pub(crate) limit: u32,
    #[arg(long, value_enum, default_value_t = RefreshMode::None)]
    pub(crate) refresh: RefreshMode,
    #[arg(long, value_enum, default_value_t = OutputFormat::Markdown)]
    pub(crate) format: OutputFormat,
}

#[derive(Args, Debug, Clone)]
pub(crate) struct TargetArgs {
    #[arg(long, conflicts_with = "query")]
    pub(crate) id: Option<String>,
    #[arg(long, conflicts_with = "id")]
    pub(crate) query: Option<String>,
    #[arg(long, requires = "query")]
    pub(crate) file: Option<String>,
}

#[derive(Args, Debug)]
pub(crate) struct SymbolCommand {
    #[command(flatten)]
    pub(crate) project: ProjectArgs,
    #[command(flatten)]
    pub(crate) target: TargetArgs,
    #[arg(long, value_enum, default_value_t = RefreshMode::None)]
    pub(crate) refresh: RefreshMode,
    #[arg(long, value_enum, default_value_t = OutputFormat::Markdown)]
    pub(crate) format: OutputFormat,
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
    #[arg(long, value_enum, default_value_t = CliLayout::Horizontal)]
    pub(crate) layout: CliLayout,
    #[arg(long, value_enum, default_value_t = RefreshMode::None)]
    pub(crate) refresh: RefreshMode,
    #[arg(long, value_enum, default_value_t = OutputFormat::Markdown)]
    pub(crate) format: OutputFormat,
}

#[derive(Args, Debug)]
pub(crate) struct SnippetCommand {
    #[command(flatten)]
    pub(crate) project: ProjectArgs,
    #[command(flatten)]
    pub(crate) target: TargetArgs,
    #[arg(long, default_value_t = 4)]
    pub(crate) context: usize,
    #[arg(long, value_enum, default_value_t = RefreshMode::None)]
    pub(crate) refresh: RefreshMode,
    #[arg(long, value_enum, default_value_t = OutputFormat::Markdown)]
    pub(crate) format: OutputFormat,
}

#[derive(Debug, Serialize)]
pub(crate) struct IndexOutput<'a> {
    pub(crate) project: &'a str,
    pub(crate) storage_path: &'a str,
    pub(crate) refresh: &'a str,
    pub(crate) summary: &'a ProjectSummary,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) phase_timings: Option<&'a IndexingPhaseTimings>,
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
