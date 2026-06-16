use clap::{ArgGroup, Args, Parser, Subcommand, ValueEnum};
use codestory_contracts::api::{
    BookmarkCategoryDto, BookmarkDto, ClaimReadinessDto, EvidencePacketDto, GroundingBudgetDto,
    IndexDryRunDto, IndexFreshnessDto, IndexedFileRoleDto, IndexingPhaseTimings, LayoutDirection,
    NodeId, NodeKind, PacketBudgetModeDto, PacketTaskClassDto, ProjectSummary, ReadinessGoalDto,
    ReadinessVerdictDto, RepoTextScanStatsDto, RetrievalScoreBreakdownDto, RetrievalShadowDto,
    RetrievalStateDto, SearchHitOrigin, SearchMatchQualityDto, SearchPlanDto,
    SearchQueryAssessmentDto, SnippetContextDto, SummaryGenerationDto, SymbolContextDto,
    TrailCallerScope, TrailContextDto, TrailDirection, TrailMode,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

const INDEX_REFRESH_HELP: &str = "Index defaults to `auto`: it chooses `full` for an empty cache and `incremental` once the \
cache already has indexed files.";
const READ_REFRESH_HELP: &str = "Read commands default to `none` so they only query the existing cache. Use `incremental` to \
refresh an existing cache in place, or `full` after a cache reset, schema change, or indexing \
failure.";
const DRILL_REFRESH_HELP: &str = "Drill defaults to `full` so each report is mechanically fresh. Use `none` only after a \
fresh index, or `incremental` to refresh an existing cache in place.";
const CLI_LONG_ABOUT: &str = "\
CodeStory turns a local repository into auditable grounding evidence.

Common lanes:
  New repo:      codestory-cli index --project <repo> --refresh full
  Broad question: codestory-cli packet --project <repo> --question \"How does this system work?\"
  Exact target:  codestory-cli context --project <repo> --query <symbol-or-file>

For agent packet/search readiness, first run retrieval bootstrap + retrieval index and require retrieval status to report retrieval_mode=full.";

#[derive(Parser, Debug)]
#[command(author, version, about = "Skill-first repo grounding runtime", long_about = CLI_LONG_ABOUT)]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Command,
}

#[derive(Subcommand, Debug)]
pub(crate) enum Command {
    #[command(about = "Build or refresh the repository index.")]
    Index(IndexCommand),
    #[command(about = "Summarize indexed repository context.")]
    Ground(GroundCommand),
    #[command(about = "Generate a repo report or machine graph export from the current store.")]
    Report(ReportCommand),
    #[command(about = "Gather evidence for one concrete target.")]
    Context(ContextCommand),
    #[command(about = "Answer a broad repository question with evidence.")]
    Packet(PacketCommand),
    #[command(about = "Check cache, index, and retrieval health.")]
    Doctor(DoctorCommand),
    #[command(about = "Print compact readiness verdicts for local navigation or agent search.")]
    Ready(ReadyCommand),
    #[command(about = "Install or check local setup assets.")]
    Setup(SetupCommand),
    #[command(about = "Find symbols and repo text evidence.")]
    Search(SearchCommand),
    #[command(about = "Run a repeatable grounding quality check.")]
    Drill(DrillCommand),
    #[command(about = "Run a drill manifest across repositories.")]
    DrillSuite(DrillSuiteCommand),
    #[command(about = "Inspect a symbol by query or id.")]
    Symbol(SymbolCommand),
    #[command(about = "Trace calls, refs, and related symbols.")]
    Trail(TrailCommand),
    #[command(about = "Show source around a symbol.")]
    Snippet(SnippetCommand),
    #[command(about = "Run structured graph queries.")]
    Query(QueryCommand),
    #[command(about = "Open the terminal explorer or print an exploration packet.")]
    Explore(ExploreCommand),
    #[command(about = "List indexed files and coverage.")]
    Files(FilesCommand),
    #[command(about = "Explain impact from changed files.")]
    Affected(AffectedCommand),
    #[command(about = "Save and reuse investigation targets.")]
    Bookmark(BookmarkCommand),
    #[command(about = "Start the local integration surface.")]
    Serve(ServeCommand),
    #[command(about = "Generate shell completions.")]
    GenerateCompletions(GenerateCompletionsCommand),
    #[command(about = "Manage retrieval sidecar data.")]
    Retrieval(RetrievalCommand),
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

fn parse_read_output_format(value: &str) -> Result<OutputFormat, String> {
    match value {
        "markdown" => Ok(OutputFormat::Markdown),
        "json" => Ok(OutputFormat::Json),
        "dot" => Err("--format dot is only supported by `trail`; use markdown or json".to_string()),
        other => Err(format!(
            "invalid output format `{other}`; expected `markdown` or `json`"
        )),
    }
}

fn parse_positive_usize(value: &str) -> Result<usize, String> {
    let parsed = value
        .parse::<usize>()
        .map_err(|_| format!("invalid positive integer `{value}`"))?;
    if parsed == 0 {
        Err("value must be greater than zero".to_string())
    } else {
        Ok(parsed)
    }
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
pub(crate) enum CliPacketBudget {
    Tiny,
    Compact,
    Standard,
    Deep,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum CliPacketTaskClass {
    ArchitectureExplanation,
    BugLocalization,
    ChangeImpact,
    RouteTracing,
    SymbolOwnership,
    DataFlow,
    EditPlanning,
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
pub(crate) enum CliFileRole {
    Source,
    Test,
    Generated,
    Vendor,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum CompletionShell {
    Bash,
    Zsh,
    Fish,
    Powershell,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CliEmbeddingQuant {
    #[value(name = "q8_0")]
    Q8_0,
    #[value(name = "q4_k_m")]
    Q4KM,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CliLlamaVariant {
    Cpu,
    Vulkan,
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
    #[arg(long, value_name = "FORMAT", value_parser = parse_read_output_format, default_value = "markdown")]
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
    #[arg(long, value_name = "FORMAT", value_parser = parse_read_output_format, default_value = "markdown")]
    pub(crate) format: OutputFormat,
    #[arg(
        long,
        value_name = "PATH",
        help = "Write command output to this file instead of stdout. The parent directory must already exist."
    )]
    pub(crate) output_file: Option<PathBuf>,
    #[arg(
        long,
        help = "Explain retrieval mode, coverage, and query hints in the Markdown output."
    )]
    pub(crate) why: bool,
}

#[derive(Args, Debug)]
pub(crate) struct ReportCommand {
    #[command(flatten)]
    pub(crate) project: ProjectArgs,
    #[arg(long, value_name = "FORMAT", value_parser = parse_read_output_format, default_value = "markdown")]
    pub(crate) format: OutputFormat,
    #[arg(
        long,
        value_name = "PATH",
        help = "Write the generated report/export artifact to this file instead of stdout. The parent directory must already exist."
    )]
    pub(crate) output_file: Option<PathBuf>,
    #[arg(long, value_enum, default_value_t = ReportProfile::Full)]
    pub(crate) profile: ReportProfile,
    #[arg(
        long,
        value_name = "N",
        value_parser = parse_positive_usize,
        default_value_t = 10,
        help = "Maximum number of hotspots, entry points, bridges, and follow-up queries to include in report sections. JSON graph export still includes the full graph."
    )]
    pub(crate) limit: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, ValueEnum)]
pub(crate) enum ReportProfile {
    #[default]
    Full,
    Handoff,
}

#[derive(Args, Debug)]
#[command(group(
    ArgGroup::new("context_target")
        .args(["id", "query", "bookmark"])
        .required(true)
        .multiple(false)
))]
pub(crate) struct ContextCommand {
    #[command(flatten)]
    pub(crate) project: ProjectArgs,
    #[arg(
        long,
        allow_hyphen_values = true,
        value_name = "NODE_ID",
        group = "context_target",
        help = "Build context around an exact node id from search, symbol, trail, or explore output."
    )]
    pub(crate) id: Option<String>,
    #[arg(
        long,
        group = "context_target",
        help = "Resolve a concrete symbol, file, literal, API path, module, or behavior term before building context."
    )]
    pub(crate) query: Option<String>,
    #[arg(
        long,
        value_name = "BOOKMARK_ID",
        group = "context_target",
        help = "Build context around a saved bookmark target."
    )]
    pub(crate) bookmark: Option<String>,
    #[arg(long, default_value_t = 8)]
    pub(crate) max_results: u32,
    #[arg(
        long,
        value_enum,
        default_value_t = RefreshMode::None,
        long_help = READ_REFRESH_HELP
    )]
    pub(crate) refresh: RefreshMode,
    #[arg(long, value_name = "FORMAT", value_parser = parse_read_output_format, default_value = "markdown")]
    pub(crate) format: OutputFormat,
    #[arg(
        long,
        value_name = "PATH",
        help = "Write command output to this file instead of stdout. The parent directory must already exist."
    )]
    pub(crate) output_file: Option<PathBuf>,
    #[arg(
        long,
        value_name = "DIR",
        help = "Write a context bundle with Markdown, JSON, and generated graph artifacts."
    )]
    pub(crate) bundle: Option<PathBuf>,
    #[arg(
        long,
        help = "Omit citation edge ids and score breakdowns from the structured context packet."
    )]
    pub(crate) no_evidence: bool,
}

#[derive(Args, Debug)]
pub(crate) struct PacketCommand {
    #[command(flatten)]
    pub(crate) project: ProjectArgs,
    #[arg(
        long,
        help = "Broad repository question or task to ground in one packet."
    )]
    pub(crate) question: String,
    #[arg(long, value_enum, default_value_t = CliPacketBudget::Compact)]
    pub(crate) budget: CliPacketBudget,
    #[arg(long, value_enum)]
    pub(crate) task_class: Option<CliPacketTaskClass>,
    #[arg(
        long = "extra-probe",
        value_name = "QUERY",
        help = "Add an explicit file, symbol, or file-scoped symbol probe to the packet plan. Repeatable; intended for audited benchmark or operator-supplied anchors."
    )]
    pub(crate) extra_probes: Vec<String>,
    #[arg(
        long,
        value_enum,
        default_value_t = RefreshMode::None,
        long_help = READ_REFRESH_HELP
    )]
    pub(crate) refresh: RefreshMode,
    #[arg(long, value_name = "FORMAT", value_parser = parse_read_output_format, default_value = "markdown")]
    pub(crate) format: OutputFormat,
    #[arg(
        long,
        value_name = "PATH",
        help = "Write command output to this file instead of stdout. The parent directory must already exist."
    )]
    pub(crate) output_file: Option<PathBuf>,
    #[arg(
        long,
        help = "Omit citation edge ids and score breakdowns from the structured packet."
    )]
    pub(crate) no_evidence: bool,
    #[arg(
        long,
        value_name = "MS",
        help = "Optional packet-level latency budget in milliseconds."
    )]
    pub(crate) latency_budget_ms: Option<u32>,
    #[arg(
        long,
        value_name = "PATH",
        help = "Write per-step packet retrieval trace JSON for golden scoring."
    )]
    pub(crate) step_trace_out: Option<PathBuf>,
}

#[derive(Args, Debug)]
pub(crate) struct DoctorCommand {
    #[command(flatten)]
    pub(crate) project: ProjectArgs,
    #[arg(long, value_name = "FORMAT", value_parser = parse_read_output_format, default_value = "markdown")]
    pub(crate) format: OutputFormat,
    #[arg(
        long,
        value_name = "PATH",
        help = "Write command output to this file instead of stdout. The parent directory must already exist."
    )]
    pub(crate) output_file: Option<PathBuf>,
}

#[derive(Args, Debug)]
pub(crate) struct ReadyCommand {
    #[command(flatten)]
    pub(crate) project: ProjectArgs,
    #[arg(long, value_enum)]
    pub(crate) goal: Option<ReadyGoal>,
    #[arg(long, value_name = "FORMAT", value_parser = parse_read_output_format, default_value = "markdown")]
    pub(crate) format: OutputFormat,
    #[arg(
        long,
        value_name = "PATH",
        help = "Write command output to this file instead of stdout. The parent directory must already exist."
    )]
    pub(crate) output_file: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum ReadyGoal {
    Local,
    Agent,
}

impl ReadyGoal {
    pub(crate) const fn as_dto(self) -> ReadinessGoalDto {
        match self {
            Self::Local => ReadinessGoalDto::LocalNavigation,
            Self::Agent => ReadinessGoalDto::AgentPacketSearch,
        }
    }
}

#[derive(Args, Debug)]
pub(crate) struct RetrievalCommand {
    #[command(subcommand)]
    pub(crate) action: RetrievalAction,
}

#[derive(Subcommand, Debug)]
pub(crate) enum RetrievalAction {
    /// Start Docker Compose sidecars (when available), prepare cache dirs, and wait for health.
    Bootstrap(RetrievalBootstrapCommand),
    /// Prepare local sidecar data directories and write sidecar state (does not start Docker).
    Up,
    /// Remove local sidecar state file (does not stop external processes).
    Down,
    /// Probe Zoekt, Qdrant, and SCIP availability for the project.
    Status(RetrievalStatusCommand),
    /// Run workspace index then persist retrieval_index_manifest sidecar metadata.
    Index(RetrievalIndexCommand),
    /// Execute a standalone sidecar retrieval query against indexed sidecar metadata.
    Query(RetrievalQueryCommand),
}

#[derive(Args, Debug)]
pub(crate) struct RetrievalQueryCommand {
    /// Natural-language, symbol, or path query string.
    pub(crate) query: String,
    #[command(flatten)]
    pub(crate) project: ProjectArgs,
    #[arg(
        long,
        value_name = "MS",
        help = "Hard deadline for the staged retrieval plan."
    )]
    pub(crate) budget_ms: Option<u64>,
    #[arg(long, value_name = "FORMAT", value_parser = parse_read_output_format, default_value = "json")]
    pub(crate) format: OutputFormat,
    #[arg(long, value_name = "PATH")]
    pub(crate) output_file: Option<PathBuf>,
}

#[derive(Args, Debug)]
pub(crate) struct RetrievalBootstrapCommand {
    #[command(flatten)]
    pub(crate) project: ProjectArgs,
    #[arg(
        long,
        help = "Skip docker compose even when Docker is installed (dirs + state file only)."
    )]
    pub(crate) skip_compose: bool,
    #[arg(
        long,
        value_name = "SECS",
        default_value_t = 90,
        help = "Seconds to wait for Zoekt and Qdrant HTTP probes (0 = no wait)."
    )]
    pub(crate) wait_secs: u64,
    #[arg(
        long,
        value_name = "PATH",
        help = "Override docker/retrieval-compose.yml path."
    )]
    pub(crate) compose_file: Option<PathBuf>,
    #[arg(long, value_name = "FORMAT", value_parser = parse_read_output_format, default_value = "json")]
    pub(crate) format: OutputFormat,
    #[arg(long, value_name = "PATH")]
    pub(crate) output_file: Option<PathBuf>,
}

#[derive(Args, Debug)]
pub(crate) struct RetrievalStatusCommand {
    #[command(flatten)]
    pub(crate) project: ProjectArgs,
    #[arg(long, value_name = "FORMAT", value_parser = parse_read_output_format, default_value = "json")]
    pub(crate) format: OutputFormat,
    #[arg(long, value_name = "PATH")]
    pub(crate) output_file: Option<PathBuf>,
}

#[derive(Args, Debug)]
pub(crate) struct RetrievalIndexCommand {
    #[command(flatten)]
    pub(crate) project: ProjectArgs,
    #[arg(long, value_enum, default_value_t = RefreshMode::Auto, help = INDEX_REFRESH_HELP)]
    pub(crate) refresh: RefreshMode,
    #[arg(long, value_name = "FORMAT", value_parser = parse_read_output_format, default_value = "json")]
    pub(crate) format: OutputFormat,
    #[arg(long, value_name = "PATH")]
    pub(crate) output_file: Option<PathBuf>,
}

#[derive(Args, Debug)]
pub(crate) struct SetupCommand {
    #[command(subcommand)]
    pub(crate) action: SetupAction,
}

#[derive(Subcommand, Debug)]
pub(crate) enum SetupAction {
    Embeddings(SetupEmbeddingsCommand),
}

#[derive(Args, Debug)]
pub(crate) struct SetupEmbeddingsCommand {
    #[command(flatten)]
    pub(crate) project: ProjectArgs,
    #[arg(
        long,
        value_enum,
        default_value_t = CliEmbeddingQuant::Q8_0,
        help = "Legacy GGUF quant selector retained for CLI compatibility; managed setup now installs the pinned ONNX model."
    )]
    pub(crate) quant: CliEmbeddingQuant,
    #[arg(
        long,
        value_enum,
        default_value_t = CliLlamaVariant::Vulkan,
        help = "Legacy llama.cpp variant selector retained for CLI compatibility; managed setup now uses ONNX Runtime."
    )]
    pub(crate) variant: CliLlamaVariant,
    #[arg(
        long,
        help = "Show the managed ONNX asset plan without downloading anything."
    )]
    pub(crate) dry_run: bool,
    #[arg(
        long,
        help = "Compatibility flag; managed ONNX setup never starts a server."
    )]
    pub(crate) no_start: bool,
    #[arg(long, value_name = "FORMAT", value_parser = parse_read_output_format, default_value = "markdown")]
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
    #[arg(long, value_name = "FORMAT", value_parser = parse_read_output_format, default_value = "markdown")]
    pub(crate) format: OutputFormat,
    #[arg(
        long,
        value_name = "PATH",
        help = "Write command output to this file instead of stdout. The parent directory must already exist."
    )]
    pub(crate) output_file: Option<PathBuf>,
    #[arg(
        long,
        help = "Show compact ranking, uncertainty, and next-action explanations for each result."
    )]
    pub(crate) why: bool,
    #[arg(
        long = "plan-details",
        requires = "why",
        help = "Include the full search plan in --why output. By default --why keeps provenance compact."
    )]
    pub(crate) plan_details: bool,
}

#[derive(Args, Debug)]
pub(crate) struct DrillCommand {
    #[command(flatten)]
    pub(crate) project: ProjectArgs,
    #[arg(
        long,
        value_delimiter = ',',
        num_args = 1..,
        required = true,
        help = "Comma-separated concrete anchors to investigate deterministically."
    )]
    pub(crate) anchors: Vec<String>,
    #[arg(
        long,
        help = "Human label for the drill question. Stored in the report only; it is not interpreted."
    )]
    pub(crate) label: Option<String>,
    #[arg(
        long,
        help = "Natural-language architecture question to collect repo-text evidence for. Stored and searched; it is not answered by the CLI."
    )]
    pub(crate) question: Option<String>,
    #[arg(
        long,
        value_name = "DIR",
        help = "Directory where the drill report and command artifacts are written."
    )]
    pub(crate) output_dir: PathBuf,
    #[arg(
        long,
        value_enum,
        default_value_t = RefreshMode::Full,
        long_help = DRILL_REFRESH_HELP
    )]
    pub(crate) refresh: RefreshMode,
    #[arg(long, value_name = "FORMAT", value_parser = parse_read_output_format, default_value = "markdown")]
    pub(crate) format: OutputFormat,
    #[arg(
        long,
        value_name = "N",
        default_value_t = 1,
        value_parser = parse_positive_usize,
        help = "Read-only anchor and bridge evidence workers for --refresh none drills. Defaults to 1."
    )]
    pub(crate) jobs: usize,
}

#[derive(Args, Debug)]
pub(crate) struct DrillSuiteCommand {
    #[command(flatten)]
    pub(crate) project: ProjectArgs,
    #[arg(
        long,
        value_name = "FILE",
        help = "JSON manifest describing the drill cases to run. Relative case project paths resolve from the manifest directory."
    )]
    pub(crate) case_file: PathBuf,
    #[arg(
        long,
        value_name = "FILE",
        help = "Optional source-truth ledger JSON to merge into the suite report after verification."
    )]
    pub(crate) ledger: Option<PathBuf>,
    #[arg(
        long,
        value_name = "DIR",
        help = "Directory where the suite report and per-repo drill artifacts are written."
    )]
    pub(crate) output_dir: PathBuf,
    #[arg(
        long,
        value_enum,
        default_value_t = RefreshMode::Full,
        long_help = DRILL_REFRESH_HELP
    )]
    pub(crate) refresh: RefreshMode,
    #[arg(long, value_name = "FORMAT", value_parser = parse_read_output_format, default_value = "json")]
    pub(crate) format: OutputFormat,
    #[arg(
        long,
        value_name = "N",
        default_value_t = 1,
        value_parser = parse_positive_usize,
        help = "Read-only case, anchor, and bridge workers for --refresh none drill suites. Defaults to 1."
    )]
    pub(crate) jobs: usize,
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
        allow_hyphen_values = true,
        value_name = "NODE_ID",
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
    #[arg(
        long,
        requires = "query",
        value_name = "N",
        help = "Resolve a query by the 1-based alternative number shown in an ambiguity error."
    )]
    pub(crate) choose: Option<usize>,
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
    #[arg(long, value_name = "FORMAT", value_parser = parse_read_output_format, default_value = "markdown")]
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
    #[arg(long, default_value_t = 120)]
    pub(crate) max_nodes: u32,
    #[arg(long)]
    pub(crate) include_tests: bool,
    #[arg(long)]
    pub(crate) show_utility_calls: bool,
    #[arg(
        long,
        help = "Hide uncertain/speculative edges plus probable or low-confidence runtime bridge edges, then remove nodes disconnected from the trail focus."
    )]
    pub(crate) hide_speculative: bool,
    #[arg(long, help = "Render a readable narrative of the trail graph.")]
    pub(crate) story: bool,
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
    #[arg(
        long,
        visible_alias = "lines",
        default_value_t = 4,
        help = "Number of surrounding context lines above and below the symbol. `--lines` is accepted as an agent-friendly alias."
    )]
    pub(crate) context: usize,
    #[arg(
        long,
        help = "Prefer the selected symbol's full function or method body, with context lines around it when source ranges are available."
    )]
    pub(crate) function_body: bool,
    #[arg(
        long,
        value_enum,
        default_value_t = RefreshMode::None,
        long_help = READ_REFRESH_HELP
    )]
    pub(crate) refresh: RefreshMode,
    #[arg(long, value_name = "FORMAT", value_parser = parse_read_output_format, default_value = "markdown")]
    pub(crate) format: OutputFormat,
    #[arg(
        long,
        value_name = "PATH",
        help = "Write command output to this file instead of stdout. The parent directory must already exist."
    )]
    pub(crate) output_file: Option<PathBuf>,
}

#[derive(Args, Debug)]
#[command(group(
    ArgGroup::new("query_input")
        .args(["query", "sql"])
        .required(true)
        .multiple(false)
))]
pub(crate) struct QueryCommand {
    #[command(flatten)]
    pub(crate) project: ProjectArgs,
    #[arg(
        value_name = "QUERY",
        group = "query_input",
        help = "CodeStory graph-query DSL, for example `search(query: 'AppController') | limit(5)`."
    )]
    pub(crate) query: Option<String>,
    #[arg(
        long,
        group = "query_input",
        value_name = "SQL",
        help = "SQL is not supported; this flag returns targeted guidance instead of a parser-shaped error."
    )]
    pub(crate) sql: Option<String>,
    #[arg(
        long,
        value_enum,
        default_value_t = RefreshMode::None,
        long_help = READ_REFRESH_HELP
    )]
    pub(crate) refresh: RefreshMode,
    #[arg(long, value_name = "FORMAT", value_parser = parse_read_output_format, default_value = "markdown")]
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
    #[arg(
        long,
        value_enum,
        help = "Opt into an investigation preset without changing default explore behavior."
    )]
    pub(crate) profile: Option<ExploreProfile>,
    #[arg(long, default_value_t = 2)]
    pub(crate) depth: u32,
    #[arg(long, default_value_t = 18)]
    pub(crate) max_nodes: u32,
    #[arg(
        long,
        help = "Print plain Markdown instead of opening the terminal explorer when stdout is interactive."
    )]
    pub(crate) no_tui: bool,
    #[arg(long, help = "Alias for --no-tui; useful for agent-safe plain output.")]
    pub(crate) plain: bool,
    #[arg(
        long,
        value_enum,
        default_value_t = RefreshMode::None,
        long_help = READ_REFRESH_HELP
    )]
    pub(crate) refresh: RefreshMode,
    #[arg(long, value_name = "FORMAT", value_parser = parse_read_output_format, default_value = "markdown")]
    pub(crate) format: OutputFormat,
    #[arg(
        long,
        value_name = "PATH",
        help = "Write command output to this file instead of stdout. The parent directory must already exist."
    )]
    pub(crate) output_file: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ExploreProfile {
    Architecture,
    Route,
    Bug,
    Refactor,
    TestImpact,
}

#[derive(Args, Debug)]
pub(crate) struct FilesCommand {
    #[arg(long, default_value = ".", help = "Repository root to query.")]
    pub(crate) project: PathBuf,
    #[arg(
        long,
        help = "Cache directory to use exactly as passed. If omitted, codestory-cli uses the system cache root with a per-project hashed subdirectory."
    )]
    pub(crate) cache_dir: Option<PathBuf>,
    #[arg(long, help = "Only list files whose path contains this text.")]
    pub(crate) path: Option<String>,
    #[arg(long, help = "Only list files for this indexed language.")]
    pub(crate) language: Option<String>,
    #[arg(long, value_enum, help = "Only list files with this inferred role.")]
    pub(crate) role: Option<CliFileRole>,
    #[arg(long, default_value_t = 500)]
    pub(crate) limit: u32,
    #[arg(
        long,
        value_enum,
        default_value_t = RefreshMode::None,
        long_help = READ_REFRESH_HELP
    )]
    pub(crate) refresh: RefreshMode,
    #[arg(long, value_name = "FORMAT", value_parser = parse_read_output_format, default_value = "markdown")]
    pub(crate) format: OutputFormat,
    #[arg(
        long,
        value_name = "PATH",
        help = "Write command output to this file instead of stdout. The parent directory must already exist."
    )]
    pub(crate) output_file: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum AffectedChangeSource {
    Head,
    Staged,
    Unstaged,
    Untracked,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum AffectedStdinFormat {
    Path,
    NameStatus,
}

#[derive(Args, Debug)]
pub(crate) struct AffectedCommand {
    #[command(flatten)]
    pub(crate) project: ProjectArgs,
    #[arg(
        value_name = "PATH",
        help = "Changed repo-relative path. If omitted, CodeStory reads git diff --name-status HEAD."
    )]
    pub(crate) paths: Vec<String>,
    #[arg(long, help = "Read changed paths from stdin, one path per line.")]
    pub(crate) stdin: bool,
    #[arg(
        long,
        value_enum,
        default_value_t = AffectedStdinFormat::Path,
        help = "Interpret --stdin as path-only lines or git diff --name-status rows."
    )]
    pub(crate) stdin_format: AffectedStdinFormat,
    #[arg(
        long,
        value_enum,
        default_value_t = AffectedChangeSource::Head,
        help = "Default git source when no paths/stdin are supplied."
    )]
    pub(crate) changes: AffectedChangeSource,
    #[arg(long, default_value_t = 2)]
    pub(crate) depth: u32,
    #[arg(
        long,
        help = "Filter impacted symbols by path or display name substring."
    )]
    pub(crate) filter: Option<String>,
    #[arg(
        long,
        value_enum,
        default_value_t = RefreshMode::None,
        long_help = READ_REFRESH_HELP
    )]
    pub(crate) refresh: RefreshMode,
    #[arg(long, value_name = "FORMAT", value_parser = parse_read_output_format, default_value = "markdown")]
    pub(crate) format: OutputFormat,
    #[arg(
        long,
        value_name = "PATH",
        help = "Write command output to this file instead of stdout. The parent directory must already exist."
    )]
    pub(crate) output_file: Option<PathBuf>,
}

#[derive(Args, Debug)]
pub(crate) struct BookmarkCommand {
    #[command(subcommand)]
    pub(crate) action: BookmarkAction,
}

#[derive(Subcommand, Debug)]
pub(crate) enum BookmarkAction {
    Add(BookmarkAddCommand),
    List(BookmarkListCommand),
    Remove(BookmarkRemoveCommand),
}

#[derive(Args, Debug)]
pub(crate) struct BookmarkAddCommand {
    #[command(flatten)]
    pub(crate) project: ProjectArgs,
    #[command(flatten)]
    pub(crate) target: TargetArgs,
    #[arg(long, default_value = "Investigation")]
    pub(crate) category: String,
    #[arg(long)]
    pub(crate) comment: Option<String>,
    #[arg(
        long,
        value_enum,
        default_value_t = RefreshMode::None,
        long_help = READ_REFRESH_HELP
    )]
    pub(crate) refresh: RefreshMode,
    #[arg(long, value_name = "FORMAT", value_parser = parse_read_output_format, default_value = "markdown")]
    pub(crate) format: OutputFormat,
    #[arg(
        long,
        value_name = "PATH",
        help = "Write command output to this file instead of stdout. The parent directory must already exist."
    )]
    pub(crate) output_file: Option<PathBuf>,
}

#[derive(Args, Debug)]
pub(crate) struct BookmarkListCommand {
    #[command(flatten)]
    pub(crate) project: ProjectArgs,
    #[arg(long)]
    pub(crate) category: Option<String>,
    #[arg(long, value_name = "FORMAT", value_parser = parse_read_output_format, default_value = "markdown")]
    pub(crate) format: OutputFormat,
    #[arg(
        long,
        value_name = "PATH",
        help = "Write command output to this file instead of stdout. The parent directory must already exist."
    )]
    pub(crate) output_file: Option<PathBuf>,
}

#[derive(Args, Debug)]
pub(crate) struct BookmarkRemoveCommand {
    #[command(flatten)]
    pub(crate) project: ProjectArgs,
    #[arg(value_name = "BOOKMARK_ID")]
    pub(crate) id: String,
    #[arg(long, value_name = "FORMAT", value_parser = parse_read_output_format, default_value = "markdown")]
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) readiness: Vec<ReadinessVerdictDto>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) next_commands: Vec<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct ReadyOutput {
    pub(crate) verdicts: Vec<ReadinessVerdictDto>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) number: Option<usize>,
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
    pub(crate) match_quality: SearchMatchQualityDto,
    pub(crate) resolvable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) score_breakdown: Option<RetrievalScoreBreakdownDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) duplicate_of: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) excerpt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) primary_occurrence_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) symbol_role: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) paired_refs: Vec<VerificationTargetOutput>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) verification_targets: Vec<VerificationTargetOutput>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) resolution_hints: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) why: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct VerificationTargetOutput {
    pub(crate) role: String,
    pub(crate) path: String,
    pub(crate) line: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) node_ref: Option<String>,
    pub(crate) reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct SearchOutput {
    pub(crate) query: String,
    pub(crate) retrieval: RetrievalStateDto,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) retrieval_shadow: Option<RetrievalShadowDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) freshness: Option<IndexFreshnessDto>,
    pub(crate) limit_per_source: u32,
    pub(crate) repo_text_mode: RepoTextMode,
    pub(crate) repo_text_enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) query_assessment: Option<SearchQueryAssessmentDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) search_plan: Option<SearchPlanDto>,
    pub(crate) explain: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) query_hints: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) suggestions: Vec<SearchHitOutput>,
    pub(crate) indexed_symbol_hits: Vec<SearchHitOutput>,
    pub(crate) repo_text_hits: Vec<SearchHitOutput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) repo_text_stats: Option<RepoTextScanStatsDto>,
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) verification_targets: Vec<VerificationTargetOutput>,
}

#[derive(Debug, Serialize)]
pub(crate) struct TrailJsonOutput<'a> {
    pub(crate) resolution: QueryResolutionOutput,
    pub(crate) trail: &'a TrailContextDto,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) notes: Vec<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct SnippetJsonOutput<'a> {
    pub(crate) resolution: QueryResolutionOutput,
    pub(crate) snippet: &'a SnippetContextDto,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) verification_targets: Vec<VerificationTargetOutput>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct DrillRuntimeTimingsOutput {
    pub(crate) total_ms: u64,
    pub(crate) setup_ms: u64,
    pub(crate) question_search_ms: u64,
    pub(crate) anchor_resolution_ms: u64,
    pub(crate) supplemental_search_ms: u64,
    pub(crate) bridge_evidence_ms: u64,
    pub(crate) evidence_assembly_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct DrillMechanicalOutput {
    pub(crate) before_files: u32,
    pub(crate) before_nodes: u32,
    pub(crate) before_edges: u32,
    pub(crate) before_errors: u32,
    pub(crate) after_files: u32,
    pub(crate) after_nodes: u32,
    pub(crate) after_edges: u32,
    pub(crate) after_errors: u32,
    pub(crate) refresh: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) retrieval: Option<RetrievalStateDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) sidecar_retrieval_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) freshness: Option<IndexFreshnessDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) phase_timings: Option<IndexingPhaseTimings>,
    pub(crate) drill_timings: DrillRuntimeTimingsOutput,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct DrillCommandStatusOutput {
    pub(crate) command: String,
    pub(crate) status: String,
    pub(crate) duration_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) artifact: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct DrillBridgeGraphPathOutput {
    pub(crate) mode: String,
    pub(crate) node_count: usize,
    pub(crate) edge_count: usize,
    pub(crate) truncated: bool,
    pub(crate) omitted_edge_count: u32,
    pub(crate) nodes: Vec<String>,
    pub(crate) edges: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct DrillBridgeEvidenceOutput {
    pub(crate) from_anchor: String,
    pub(crate) to_anchor: String,
    pub(crate) status: String,
    pub(crate) strategy: String,
    pub(crate) confidence: String,
    pub(crate) evidence_kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) from_node: Option<SearchHitOutput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) to_node: Option<SearchHitOutput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) graph_path: Option<DrillBridgeGraphPathOutput>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) shared_files: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) endpoint_files: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) evidence_files: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) next_commands: Vec<String>,
    pub(crate) notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct DrillBridgeOutput {
    pub(crate) evidence: DrillBridgeEvidenceOutput,
    pub(crate) command: DrillCommandStatusOutput,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct DrillAnchorConsumerOutput {
    pub(crate) name: String,
    pub(crate) kind: NodeKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) file_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) qualified_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) target_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) target_kind: Option<NodeKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) target_file_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) target_relation: Option<String>,
    pub(crate) edge_kind: codestory_contracts::api::EdgeKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) confidence: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) certainty: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct DrillAnchorTextConsumerHintOutput {
    pub(crate) name: String,
    pub(crate) kind: NodeKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) file_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) line: Option<u32>,
    pub(crate) score: f32,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct DrillAnchorConsumerSummaryOutput {
    pub(crate) caller_count: usize,
    pub(crate) consumer_count: usize,
    pub(crate) text_hint_count: usize,
    pub(crate) truncated: bool,
    pub(crate) omitted_edge_count: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) callers: Vec<DrillAnchorConsumerOutput>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) consumers: Vec<DrillAnchorConsumerOutput>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) text_consumer_hints: Vec<DrillAnchorTextConsumerHintOutput>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) notes: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct DrillAnchorTimingsOutput {
    pub(crate) total_ms: u64,
    pub(crate) search_ms: u64,
    pub(crate) resolution_ms: u64,
    pub(crate) consumer_summary_ms: u64,
    pub(crate) command_artifacts_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct DrillAnchorOutput {
    pub(crate) anchor: String,
    pub(crate) typed_hit_count: usize,
    pub(crate) chosen_anchor: Option<SearchHitOutput>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) verification_targets: Vec<VerificationTargetOutput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) consumer_summary: Option<DrillAnchorConsumerSummaryOutput>,
    pub(crate) timings: DrillAnchorTimingsOutput,
    pub(crate) commands: Vec<DrillCommandStatusOutput>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct DrillVerificationChecklistItemOutput {
    pub(crate) item: String,
    pub(crate) allowed_classifications: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct DrillAnswerQualityContractOutput {
    pub(crate) code_story_only_draft_required: bool,
    pub(crate) source_truth_verification_required: bool,
    pub(crate) pass_condition: String,
    pub(crate) score_inputs: Vec<String>,
    pub(crate) correction_buckets: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct DrillClaimLedgerEntryOutput {
    pub(crate) id: String,
    pub(crate) claim: String,
    pub(crate) expected_evidence: Vec<String>,
    pub(crate) source_truth_files: Vec<String>,
    pub(crate) pre_verification_confidence: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) classification: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) changed_after_source_read: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) correction_note: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct DrillClaimLedgerScoringOutput {
    pub(crate) status: String,
    pub(crate) pending_claim_count: u32,
    pub(crate) correct: u32,
    pub(crate) partial: u32,
    pub(crate) misleading: u32,
    pub(crate) unsupported: u32,
    pub(crate) material_revision_count: u32,
    pub(crate) score_formula: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct DrillClaimLedgerOutput {
    pub(crate) template_version: u32,
    pub(crate) instructions: Vec<String>,
    pub(crate) claims: Vec<DrillClaimLedgerEntryOutput>,
    pub(crate) scoring: DrillClaimLedgerScoringOutput,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct DrillSummaryStatsOutput {
    pub(crate) files: u32,
    pub(crate) nodes: u32,
    pub(crate) edges: u32,
    pub(crate) errors: u32,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct DrillSummaryMechanicalOutput {
    pub(crate) refresh: String,
    pub(crate) before: DrillSummaryStatsOutput,
    pub(crate) after: DrillSummaryStatsOutput,
    pub(crate) index_ready: bool,
    pub(crate) error_delta: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) retrieval_status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) freshness_status: Option<String>,
    pub(crate) stale_file_count: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) freshness_samples: Vec<String>,
    pub(crate) phase_timing_available: bool,
    pub(crate) drill_timings: DrillRuntimeTimingsOutput,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct DrillSummaryAnchorStatusOutput {
    pub(crate) anchor: String,
    pub(crate) status: String,
    pub(crate) typed_hit_count: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) selected: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) selected_node_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) selected_node_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) selected_kind: Option<NodeKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) selected_file_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) selected_line: Option<u32>,
    pub(crate) caller_count: usize,
    pub(crate) consumer_count: usize,
    pub(crate) text_hint_count: usize,
    pub(crate) command_count: usize,
    pub(crate) failed_command_count: usize,
    pub(crate) command_duration_ms: u64,
    pub(crate) total_duration_ms: u64,
    pub(crate) resolution_duration_ms: u64,
    pub(crate) consumer_summary_duration_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) slowest_command: Option<String>,
    pub(crate) slowest_command_ms: u64,
    pub(crate) source_truth_target_count: usize,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct DrillSummaryAnchorsOutput {
    pub(crate) requested: usize,
    pub(crate) resolved: usize,
    pub(crate) unresolved: usize,
    pub(crate) failed_command_count: usize,
    pub(crate) statuses: Vec<DrillSummaryAnchorStatusOutput>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct DrillSummaryBridgeStatusOutput {
    pub(crate) from_anchor: String,
    pub(crate) to_anchor: String,
    pub(crate) status: String,
    pub(crate) confidence: String,
    pub(crate) strategy: String,
    pub(crate) command_status: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct DrillSummaryBridgesOutput {
    pub(crate) total: usize,
    pub(crate) graph_path: usize,
    pub(crate) partial: usize,
    pub(crate) unresolved_or_error: usize,
    pub(crate) statuses: Vec<DrillSummaryBridgeStatusOutput>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct DrillSummarySourceTruthTargetOutput {
    pub(crate) path: String,
    pub(crate) role: String,
    pub(crate) rank_reason: String,
    pub(crate) check_reasons: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct DrillSummarySourceTruthOutput {
    pub(crate) required: bool,
    pub(crate) check_count: usize,
    pub(crate) pending_check_count: usize,
    pub(crate) verified_check_count: usize,
    pub(crate) target_file_count: usize,
    pub(crate) target_files: Vec<String>,
    pub(crate) target_file_details: Vec<DrillSummarySourceTruthTargetOutput>,
    pub(crate) checklist_item_count: usize,
    pub(crate) claim_count: usize,
    pub(crate) pending_claim_count: usize,
    pub(crate) verified_claim_count: usize,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct DrillSummaryOpenGapsOutput {
    pub(crate) overall_status: ClaimReadinessDto,
    pub(crate) answer_quality_status: String,
    pub(crate) safe_to_say_count: usize,
    pub(crate) inferred_claim_count: usize,
    pub(crate) needs_verification_count: usize,
    pub(crate) needs_verification_claim_count: usize,
    pub(crate) pending_claim_count: usize,
    pub(crate) pending_source_truth_check_count: usize,
    pub(crate) next_command_count: usize,
    pub(crate) open_gap_friendly: bool,
    pub(crate) status: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct DrillSummaryVerdictOutput {
    pub(crate) status: String,
    pub(crate) reason: String,
    pub(crate) next_action: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct DrillExecutionBoundaryOutput {
    pub(crate) command: String,
    pub(crate) flow: Vec<String>,
    pub(crate) source_files: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct DrillSummaryOutput {
    pub(crate) summary_version: u32,
    pub(crate) project: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) question: Option<String>,
    pub(crate) output_dir: String,
    pub(crate) full_report_json: String,
    pub(crate) full_report_markdown: String,
    pub(crate) mechanical: DrillSummaryMechanicalOutput,
    pub(crate) anchors: DrillSummaryAnchorsOutput,
    pub(crate) bridges: DrillSummaryBridgesOutput,
    pub(crate) source_truth: DrillSummarySourceTruthOutput,
    pub(crate) open_gaps: DrillSummaryOpenGapsOutput,
    pub(crate) verdict: DrillSummaryVerdictOutput,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct DrillOutput {
    pub(crate) project: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) question: Option<String>,
    pub(crate) output_dir: String,
    pub(crate) mechanical: DrillMechanicalOutput,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) question_search: Option<DrillCommandStatusOutput>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) question_supplemental_searches: Vec<DrillCommandStatusOutput>,
    pub(crate) anchors: Vec<DrillAnchorOutput>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) bridges: Vec<DrillBridgeOutput>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) execution_boundaries: Vec<DrillExecutionBoundaryOutput>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) verification_targets: Vec<VerificationTargetOutput>,
    pub(crate) evidence_packet: EvidencePacketDto,
    pub(crate) answer_quality_contract: DrillAnswerQualityContractOutput,
    pub(crate) claim_ledger_template: DrillClaimLedgerOutput,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) verification_checklist: Vec<DrillVerificationChecklistItemOutput>,
    pub(crate) next_commands: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct DrillSuiteRepoOutput {
    pub(crate) slug: String,
    pub(crate) project: String,
    pub(crate) question: String,
    pub(crate) anchors: Vec<String>,
    pub(crate) output_dir: String,
    pub(crate) artifact_extension: String,
    pub(crate) summary: DrillSummaryOutput,
    pub(crate) expectations: DrillSuiteExpectationOutput,
    pub(crate) answer_quality: DrillSuiteAnswerQualityOutput,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct DrillSuiteExpectationOutput {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) source_truth_files: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) false_claims: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) min_anchor_resolution: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) allow_partial_bridges: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct DrillSuiteLayerFindingOutput {
    pub(crate) layer: String,
    pub(crate) status: String,
    pub(crate) detail: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct DrillSuiteAnswerQualityOutput {
    pub(crate) ledger_status: String,
    pub(crate) final_answer_status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) draft_written: Option<bool>,
    pub(crate) claim_count: usize,
    pub(crate) claim_correct_count: usize,
    pub(crate) claim_partial_count: usize,
    pub(crate) claim_misleading_count: usize,
    pub(crate) claim_unsupported_count: usize,
    pub(crate) claim_unclassified_count: usize,
    pub(crate) material_revision_count: usize,
    pub(crate) expected_file_count: usize,
    pub(crate) expected_file_found_count: usize,
    pub(crate) expected_file_missing_count: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) expected_file_recall: Option<f32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) missing_expected_files: Vec<String>,
    pub(crate) forbidden_claim_count: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) forbidden_claim_hits: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) layer_findings: Vec<DrillSuiteLayerFindingOutput>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct DrillSuiteRetrievalBlockerOutput {
    pub(crate) status: String,
    pub(crate) repo_count: usize,
    pub(crate) repos: Vec<String>,
    pub(crate) next_action: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct DrillSuiteOutput {
    pub(crate) suite: String,
    pub(crate) project: String,
    pub(crate) case_file: String,
    pub(crate) output_dir: String,
    pub(crate) repo_count: usize,
    pub(crate) degraded_count: usize,
    pub(crate) blocked_count: usize,
    pub(crate) ready_count: usize,
    pub(crate) answer_ready_count: usize,
    pub(crate) answer_degraded_count: usize,
    pub(crate) answer_failed_count: usize,
    pub(crate) answer_pending_count: usize,
    pub(crate) repos: Vec<DrillSuiteRepoOutput>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) retrieval_blockers: Vec<DrillSuiteRetrievalBlockerOutput>,
    pub(crate) next_actions: Vec<String>,
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
    pub(crate) profile: ExploreProfileOutput,
    pub(crate) status: ExploreStatusOutput,
    pub(crate) search: ExploreSearchOutput,
    pub(crate) resolution: QueryResolutionOutput,
    pub(crate) navigation: NavigationOutput,
    pub(crate) relationship_evidence: ExploreRelationshipEvidenceOutput,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) route_context: Option<codestory_contracts::api::RouteEndpointMetadataDto>,
    pub(crate) source_packet: ExploreSourcePacketOutput,
    pub(crate) symbol: &'a SymbolContextDto,
    pub(crate) trail: &'a TrailContextDto,
    pub(crate) snippet: Option<&'a SnippetContextDto>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ExploreRelationshipEvidenceOutput {
    pub(crate) map_source: String,
    pub(crate) caller_scope: String,
    pub(crate) trail_nodes: usize,
    pub(crate) trail_edges: usize,
    pub(crate) incoming_references: usize,
    pub(crate) outgoing_references: usize,
    pub(crate) notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ExploreProfileOutput {
    pub(crate) requested: String,
    pub(crate) depth: u32,
    pub(crate) max_nodes: u32,
    pub(crate) caller_scope: String,
    pub(crate) notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ExploreBudgetOutput {
    pub(crate) indexed_files: u32,
    pub(crate) max_files: u32,
    pub(crate) max_nodes_for_source: u32,
    pub(crate) max_lines_per_slice: u32,
    pub(crate) max_chars_per_file: u32,
    pub(crate) max_total_chars: u32,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ExploreSourceSliceOutput {
    pub(crate) start_line: u32,
    pub(crate) end_line: u32,
    pub(crate) symbols: Vec<String>,
    pub(crate) source: Option<String>,
    pub(crate) truncated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) gap_before: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ExploreSourceFileOutput {
    pub(crate) path: String,
    pub(crate) slices: Vec<ExploreSourceSliceOutput>,
    pub(crate) truncated: bool,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ExploreSourcePacketOutput {
    pub(crate) budget: ExploreBudgetOutput,
    pub(crate) files: Vec<ExploreSourceFileOutput>,
    pub(crate) related_files: Vec<String>,
    pub(crate) truncated: bool,
    pub(crate) notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct BookmarkOutput {
    pub(crate) bookmark: BookmarkDto,
    pub(crate) stale: bool,
}

#[derive(Debug, Serialize)]
pub(crate) struct BookmarkAddOutput {
    pub(crate) category: BookmarkCategoryDto,
    pub(crate) bookmark: BookmarkOutput,
}

#[derive(Debug, Serialize)]
pub(crate) struct BookmarkListOutput {
    pub(crate) categories: Vec<BookmarkCategoryDto>,
    pub(crate) bookmarks: Vec<BookmarkOutput>,
}

#[derive(Debug, Serialize)]
pub(crate) struct BookmarkRemoveOutput {
    pub(crate) removed_id: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ExploreStatusOutput {
    pub(crate) project: String,
    pub(crate) storage_path: String,
    pub(crate) refresh: String,
    pub(crate) output_target: String,
    pub(crate) indexed_files: u32,
    pub(crate) indexed_nodes: u32,
    pub(crate) indexed_edges: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) retrieval: Option<RetrievalStateDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) freshness: Option<IndexFreshnessDto>,
    pub(crate) next_commands: Vec<String>,
    pub(crate) layer_notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ExploreSearchOutput {
    pub(crate) selector: QuerySelectorOutput,
    pub(crate) requested: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) file_filter: Option<String>,
    pub(crate) selected: SearchHitOutput,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) alternatives: Vec<SearchHitOutput>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct NavigationOutput {
    pub(crate) definition: SearchHitOutput,
    pub(crate) incoming_references: Vec<QueryItemOutput>,
    pub(crate) outgoing_references: Vec<QueryItemOutput>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct DoctorCheckOutput {
    pub(crate) name: String,
    pub(crate) status: String,
    pub(crate) message: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct DoctorSidecarStatusOutput {
    pub(crate) retrieval_mode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) degraded_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) manifest_generation: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) manifest_input_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct DoctorOutput {
    pub(crate) project: String,
    pub(crate) storage_path: String,
    pub(crate) indexed: bool,
    pub(crate) stats: codestory_contracts::api::StorageStatsDto,
    pub(crate) retrieval_mode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) degraded_reason: Option<String>,
    pub(crate) sidecar_retrieval: DoctorSidecarStatusOutput,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) retrieval: Option<RetrievalStateDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) freshness: Option<IndexFreshnessDto>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) readiness: Vec<ReadinessVerdictDto>,
    pub(crate) checks: Vec<DoctorCheckOutput>,
    pub(crate) next_commands: Vec<String>,
    pub(crate) environment: Vec<DoctorCheckOutput>,
}

#[derive(Debug)]
pub(crate) enum TargetSelection {
    Id(NodeId),
    Query {
        query: String,
        choose: Option<usize>,
    },
}

impl TargetArgs {
    pub(crate) fn selection(&self) -> anyhow::Result<TargetSelection> {
        match (&self.id, &self.query) {
            (Some(id), None) => Ok(TargetSelection::Id(NodeId(id.trim().to_string()))),
            (None, Some(query)) if !query.trim().is_empty() => Ok(TargetSelection::Query {
                query: query.trim().to_string(),
                choose: self.choose,
            }),
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

impl From<CliPacketBudget> for PacketBudgetModeDto {
    fn from(value: CliPacketBudget) -> Self {
        match value {
            CliPacketBudget::Tiny => Self::Tiny,
            CliPacketBudget::Compact => Self::Compact,
            CliPacketBudget::Standard => Self::Standard,
            CliPacketBudget::Deep => Self::Deep,
        }
    }
}

impl From<CliPacketTaskClass> for PacketTaskClassDto {
    fn from(value: CliPacketTaskClass) -> Self {
        match value {
            CliPacketTaskClass::ArchitectureExplanation => Self::ArchitectureExplanation,
            CliPacketTaskClass::BugLocalization => Self::BugLocalization,
            CliPacketTaskClass::ChangeImpact => Self::ChangeImpact,
            CliPacketTaskClass::RouteTracing => Self::RouteTracing,
            CliPacketTaskClass::SymbolOwnership => Self::SymbolOwnership,
            CliPacketTaskClass::DataFlow => Self::DataFlow,
            CliPacketTaskClass::EditPlanning => Self::EditPlanning,
        }
    }
}

impl From<CliFileRole> for IndexedFileRoleDto {
    fn from(value: CliFileRole) -> Self {
        match value {
            CliFileRole::Source => Self::Source,
            CliFileRole::Test => Self::Test,
            CliFileRole::Generated => Self::Generated,
            CliFileRole::Vendor => Self::Vendor,
            CliFileRole::Unknown => Self::Unknown,
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
        hide_speculative: cmd.hide_speculative,
        story: cmd.story,
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
    use clap::{CommandFactory, Parser};

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
        assert!(help.contains("--id <NODE_ID>"));
        assert!(help.contains("--query <QUERY>"));
        assert!(help.contains("--file <FILE>"));
        assert!(
            help.contains("<--id <NODE_ID>|--query <QUERY>>"),
            "selector help should surface the required selector group in the usage line: {help}"
        );
    }

    #[test]
    fn read_commands_explain_refresh_none_default() {
        for name in [
            "ground", "context", "search", "symbol", "trail", "snippet", "query", "explore",
            "serve",
        ] {
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
        assert!(help.contains("--why"));
        assert!(help.contains("auto"));
        assert!(help.contains("on"));
        assert!(help.contains("off"));
    }

    #[test]
    fn drill_help_exposes_deterministic_report_controls() {
        let help = render_subcommand_help("drill");
        assert!(help.contains("--anchors <ANCHORS>"));
        assert!(help.contains("--output-dir <DIR>"));
        assert!(help.contains("--label <LABEL>"));
        assert!(help.contains("--question <QUESTION>"));
        assert!(help.contains("--jobs <N>"));
        assert!(help.contains("Stored in the report only; it is not interpreted"));
        assert!(help.contains("Drill defaults to `full`"));
    }

    #[test]
    fn drill_suite_help_exposes_source_truth_ledger() {
        let help = render_subcommand_help("drill-suite");
        assert!(help.contains("--case-file <FILE>"));
        assert!(help.contains("--ledger <FILE>"));
        assert!(help.contains("--jobs <N>"));
        assert!(help.contains("source-truth ledger JSON"));
    }

    #[test]
    fn drill_jobs_parse_with_conservative_default() {
        let drill = Cli::try_parse_from([
            "codestory-cli",
            "drill",
            "--anchors",
            "A,B",
            "--output-dir",
            "target/drill/test",
        ])
        .expect("drill default jobs");
        match drill.command {
            Command::Drill(cmd) => assert_eq!(cmd.jobs, 1),
            _ => panic!("expected drill command"),
        }

        let drill = Cli::try_parse_from([
            "codestory-cli",
            "drill",
            "--anchors",
            "A,B",
            "--output-dir",
            "target/drill/test",
            "--jobs",
            "4",
        ])
        .expect("drill explicit jobs");
        match drill.command {
            Command::Drill(cmd) => assert_eq!(cmd.jobs, 4),
            _ => panic!("expected drill command"),
        }

        let invalid = Cli::try_parse_from([
            "codestory-cli",
            "drill-suite",
            "--case-file",
            "cases.json",
            "--output-dir",
            "target/drill-suite/test",
            "--jobs",
            "0",
        ]);
        assert!(invalid.is_err(), "--jobs 0 should be rejected");

        let suite = Cli::try_parse_from([
            "codestory-cli",
            "drill-suite",
            "--case-file",
            "cases.json",
            "--output-dir",
            "target/drill-suite/test",
            "--jobs",
            "3",
        ])
        .expect("drill-suite explicit jobs");
        match suite.command {
            Command::DrillSuite(cmd) => assert_eq!(cmd.jobs, 3),
            _ => panic!("expected drill-suite command"),
        }
    }

    #[test]
    fn context_help_exposes_targeted_bundle_controls() {
        let help = render_subcommand_help("context");
        assert!(help.contains("<--id <NODE_ID>|--query <QUERY>|--bookmark <BOOKMARK_ID>>"));
        assert!(help.contains("--id <NODE_ID>"));
        assert!(help.contains("--query <QUERY>"));
        assert!(help.contains("--bookmark <BOOKMARK_ID>"));
        assert!(help.contains("--bundle <DIR>"));
        assert!(!help.contains("PROMPT"));
        assert!(!help.contains("--profile"));
        assert!(!help.contains("--investigate"));
        assert!(!help.contains("Question"));
        assert!(!help.contains("--with-local-agent"));
        assert!(!help.contains("--agent-command"));
    }

    #[test]
    fn snippet_help_exposes_lines_alias_for_agent_context_guess() {
        let help = render_subcommand_help("snippet");
        assert!(help.contains("--context <CONTEXT>"));
        assert!(help.contains("[aliases: --lines]"));
        assert!(help.contains("--function-body"));
        assert!(
            help.contains("Number of surrounding context lines"),
            "snippet help should make context sizing obvious: {help}"
        );
    }

    #[test]
    fn query_help_explains_graph_dsl_and_sql_guardrail() {
        let help = render_subcommand_help("query");
        assert!(help.contains("CodeStory graph-query DSL"));
        assert!(help.contains("--sql <SQL>"));
        assert!(help.contains("SQL is not supported"));
    }

    #[test]
    fn trail_help_keeps_dot_format_discoverable() {
        let help = render_subcommand_help("trail");
        assert!(
            help.contains("dot"),
            "trail help should expose its graphviz output format: {help}"
        );
    }

    #[test]
    fn non_trail_help_does_not_advertise_dot_format() {
        for name in [
            "index", "ground", "context", "doctor", "search", "symbol", "snippet", "query",
            "explore",
        ] {
            let help = render_subcommand_help(name);
            assert!(
                !help.contains("dot"),
                "{name} help should not advertise trail-only dot output: {help}"
            );
        }
    }

    #[test]
    fn non_trail_format_parser_rejects_dot_before_runtime() {
        let error = Cli::try_parse_from([
            "codestory-cli",
            "search",
            "--query",
            "AppController",
            "--format",
            "dot",
        ])
        .expect_err("search should reject trail-only dot output");

        assert!(
            error
                .to_string()
                .contains("--format dot is only supported by `trail`"),
            "search parse error should explain the trail-only dot format: {error}"
        );
        Cli::try_parse_from([
            "codestory-cli",
            "trail",
            "--query",
            "AppController",
            "--format",
            "dot",
        ])
        .expect("trail should keep accepting dot output");
    }

    #[test]
    fn doctor_help_is_read_only_health_surface() {
        let help = render_subcommand_help("doctor");
        assert!(help.contains("--format <FORMAT>"));
        assert!(help.contains("--output-file <PATH>"));
    }

    #[test]
    fn setup_embeddings_keeps_legacy_variant_default_for_cli_compatibility() {
        let cli = Cli::try_parse_from(["codestory-cli", "setup", "embeddings"])
            .expect("setup embeddings should parse");
        match cli.command {
            Command::Setup(SetupCommand {
                action: SetupAction::Embeddings(cmd),
            }) => assert_eq!(cmd.variant, CliLlamaVariant::Vulkan),
            _ => panic!("expected setup embeddings command"),
        }
    }

    #[test]
    fn negative_node_ids_parse_without_equals_workaround() {
        let cli = Cli::try_parse_from(["codestory-cli", "symbol", "--id", "-3816661223164617416"])
            .expect("negative symbol id should parse");
        match cli.command {
            Command::Symbol(cmd) => {
                assert_eq!(cmd.target.id.as_deref(), Some("-3816661223164617416"))
            }
            _ => panic!("expected symbol command"),
        }

        let cli = Cli::try_parse_from(["codestory-cli", "context", "--id", "-3816661223164617416"])
            .expect("negative context id should parse");
        match cli.command {
            Command::Context(cmd) => assert_eq!(cmd.id.as_deref(), Some("-3816661223164617416")),
            _ => panic!("expected context command"),
        }
    }
}
