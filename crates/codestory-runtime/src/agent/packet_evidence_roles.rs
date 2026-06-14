use crate::agent::packet_scoring::{
    normalize_identifier, packet_display_name_is_test_like, packet_display_path,
};
use crate::retrieval_file_role_from_path;
use codestory_contracts::api::{AgentCitationDto, NodeKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum PacketEvidenceRole {
    SqlTableDefinition,
    SqlRelationshipConstraint,
    SqlSchemaFile,
    TestsAndRegressionCoverage,
    SourceGroupConfiguration,
    IndexingWorkQueue,
    InterceptorManagement,
    RequestDispatch,
    TransportAdapter,
    ClientFactory,
    EventLoop,
    NetworkCommandInput,
    CommandDispatch,
    ArgumentPlanning,
    SearchExecutionUnit,
    CandidateFileConstruction,
    SearchDriver,
    CommandEntrypoint,
    EventOutputProcessing,
    AppServerRequestProtocol,
    RuntimeOrchestration,
    WorkspaceDiscoveryAndPlanning,
    SnapshotRefresh,
    PersistenceAndSearchProjection,
    SymbolExtraction,
    RouteHandling,
    CollectionConfiguration,
    SourceEvidence,
}

impl PacketEvidenceRole {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::SqlTableDefinition => "sql table definition",
            Self::SqlRelationshipConstraint => "sql relationship constraint",
            Self::SqlSchemaFile => "sql schema file",
            Self::TestsAndRegressionCoverage => "tests and regression coverage",
            Self::SourceGroupConfiguration => "source-group configuration",
            Self::IndexingWorkQueue => "indexing work queue",
            Self::InterceptorManagement => "interceptor management",
            Self::RequestDispatch => "request dispatch",
            Self::TransportAdapter => "transport adapter",
            Self::ClientFactory => "client factory",
            Self::EventLoop => "event loop",
            Self::NetworkCommandInput => "network command input",
            Self::CommandDispatch => "command dispatch",
            Self::ArgumentPlanning => "argument planning",
            Self::SearchExecutionUnit => "search worker",
            Self::CandidateFileConstruction => "candidate file construction",
            Self::SearchDriver => "search driver",
            Self::CommandEntrypoint => "command entrypoint",
            Self::EventOutputProcessing => "event output processing",
            Self::AppServerRequestProtocol => "app-server request protocol",
            Self::RuntimeOrchestration => "runtime orchestration",
            Self::WorkspaceDiscoveryAndPlanning => "workspace discovery and planning",
            Self::SnapshotRefresh => "snapshot refresh",
            Self::PersistenceAndSearchProjection => "persistence and search projection",
            Self::SymbolExtraction => "symbol extraction",
            Self::RouteHandling => "route handling",
            Self::CollectionConfiguration => "collection configuration",
            Self::SourceEvidence => "source evidence",
        }
    }

    pub(crate) fn is_low_priority_cap_role(self) -> bool {
        matches!(self, Self::TestsAndRegressionCoverage)
    }
}

pub(crate) fn packet_evidence_role(citation: &AgentCitationDto) -> Option<PacketEvidenceRole> {
    let display = citation.display_name.to_ascii_lowercase();
    let normalized_display = normalize_identifier(&citation.display_name);
    let path = citation
        .file_path
        .as_deref()
        .map(packet_display_path)
        .unwrap_or_default()
        .to_ascii_lowercase();

    if path.ends_with(".sql") && normalized_display.starts_with("createtable") {
        Some(PacketEvidenceRole::SqlTableDefinition)
    } else if path.ends_with(".sql") && normalized_display == "foreignkey" {
        Some(PacketEvidenceRole::SqlRelationshipConstraint)
    } else if path.ends_with(".sql") {
        Some(PacketEvidenceRole::SqlSchemaFile)
    } else if path_contains_test_segment(&path)
        || path.ends_with("_test.go")
        || path.ends_with(".test.ts")
        || packet_display_name_is_test_like(&display)
    {
        Some(PacketEvidenceRole::TestsAndRegressionCoverage)
    } else if normalized_display.contains("sourcegroup")
        || path.contains("source_group")
        || path.contains("sourcegroup")
    {
        Some(PacketEvidenceRole::SourceGroupConfiguration)
    } else if normalized_display.contains("buildindex")
        || normalized_display.contains("taskfillindexercommandsqueue")
        || normalized_display.contains("indexercommand")
        || normalized_display.contains("javaindexer")
        || path.contains("/data/indexer/")
    {
        Some(PacketEvidenceRole::IndexingWorkQueue)
    } else if normalized_display.contains("interceptor") || path.contains("interceptor") {
        Some(PacketEvidenceRole::InterceptorManagement)
    } else if (normalized_display.contains("dispatch")
        || path.contains("/dispatch")
        || path.contains("_dispatch"))
        && !normalized_display.contains("event")
    {
        Some(PacketEvidenceRole::RequestDispatch)
    } else if path.contains("/adapters/") || normalized_display.contains("adapter") {
        Some(PacketEvidenceRole::TransportAdapter)
    } else if (normalized_display.contains("factory") || normalized_display.contains("create"))
        && (normalized_display.contains("client") || normalized_display.contains("instance"))
    {
        Some(PacketEvidenceRole::ClientFactory)
    } else if normalized_display.contains("eventloop")
        || normalized_display.contains("event_loop")
        || (normalized_display.contains("event") && normalized_display.contains("poll"))
        || (normalized_display.contains("event") && normalized_display.contains("dispatch"))
        || path.contains("/event/")
        || path.contains("/events/")
    {
        Some(PacketEvidenceRole::EventLoop)
    } else if (normalized_display.contains("read")
        || normalized_display.contains("input")
        || normalized_display.contains("receive"))
        && (normalized_display.contains("client")
            || normalized_display.contains("socket")
            || normalized_display.contains("network")
            || path.contains("/network"))
    {
        Some(PacketEvidenceRole::NetworkCommandInput)
    } else if normalized_display.contains("command")
        && (normalized_display.contains("dispatch")
            || normalized_display.contains("handler")
            || normalized_display.contains("process")
            || normalized_display.contains("execute"))
    {
        Some(PacketEvidenceRole::CommandDispatch)
    } else if (normalized_display.contains("args")
        || normalized_display.contains("flags")
        || path.contains("/flags/"))
        && (normalized_display.contains("plan")
            || normalized_display.contains("parse")
            || normalized_display.contains("build")
            || normalized_display.contains("walk")
            || normalized_display.contains("matcher")
            || normalized_display.contains("searcher")
            || normalized_display.contains("printer")
            || path.contains("/flags/"))
    {
        Some(PacketEvidenceRole::ArgumentPlanning)
    } else if normalized_display.contains("search")
        && (normalized_display.contains("worker")
            || normalized_display.contains("runner")
            || normalized_display.contains("executor"))
    {
        Some(PacketEvidenceRole::SearchExecutionUnit)
    } else if normalized_display.contains("candidate")
        && (normalized_display.contains("file") || normalized_display.contains("source"))
    {
        Some(PacketEvidenceRole::CandidateFileConstruction)
    } else if normalized_display.contains("search")
        && (normalized_display.contains("driver")
            || normalized_display.contains("entrypoint")
            || normalized_display.contains("parallel")
            || display_is_command_entrypoint(&citation.display_name, &normalized_display, &path))
    {
        Some(PacketEvidenceRole::SearchDriver)
    } else if display_is_command_entrypoint(&citation.display_name, &normalized_display, &path) {
        Some(PacketEvidenceRole::CommandEntrypoint)
    } else if display.contains("eventprocessor")
        || display.contains("event_processor")
        || display.contains("jsonl")
        || path.contains("event_processor")
        || path.contains("_events")
        || path.contains("-events")
        || path.contains("jsonl")
    {
        Some(PacketEvidenceRole::EventOutputProcessing)
    } else if (display.contains("thread") || display.contains("turn"))
        && display.contains("startparams")
        || path.contains("/protocol/")
    {
        Some(PacketEvidenceRole::AppServerRequestProtocol)
    } else if display.contains("run_exec")
        || display.contains("run_main")
        || display.contains("service")
        || display.contains("orchestrat")
        || display.contains("runtime")
        || path.contains("runtime")
    {
        Some(PacketEvidenceRole::RuntimeOrchestration)
    } else if display.contains("manifest") || display.contains("plan") || path.contains("workspace")
    {
        Some(PacketEvidenceRole::WorkspaceDiscoveryAndPlanning)
    } else if display.contains("snapshot") || display.contains("refresh") {
        Some(PacketEvidenceRole::SnapshotRefresh)
    } else if display.contains("projection")
        || display.contains("persist")
        || display.contains("storage")
        || display.contains("store")
        || path.contains("store")
    {
        Some(PacketEvidenceRole::PersistenceAndSearchProjection)
    } else if display.contains("indexer")
        || display.contains("index_file")
        || display.contains("symbol")
        || path.contains("indexer")
    {
        Some(PacketEvidenceRole::SymbolExtraction)
    } else if display.contains("route")
        || display.contains("router")
        || packet_path_is_route_like(&path)
    {
        Some(PacketEvidenceRole::RouteHandling)
    } else if path.contains("/collections/") {
        Some(PacketEvidenceRole::CollectionConfiguration)
    } else if matches!(citation.kind, NodeKind::FUNCTION | NodeKind::METHOD)
        && retrieval_file_role_from_path(&path) == crate::RetrievalFileRole::Source
    {
        Some(PacketEvidenceRole::SourceEvidence)
    } else {
        None
    }
}

pub(crate) fn packet_claim_key_for_citation(
    role: PacketEvidenceRole,
    citation: &AgentCitationDto,
) -> String {
    format!(
        "{}:{}",
        role.as_str(),
        normalize_identifier(&citation.display_name)
    )
}

fn packet_path_is_route_like(path: &str) -> bool {
    let normalized_path = packet_display_path(path).replace('\\', "/");
    normalized_path.contains("/routes/")
        || normalized_path.contains("/router/")
        || normalized_path.contains("/controllers/")
        || normalized_path.contains("/views/")
        || normalized_path.contains("/pages/")
        || normalized_path.contains("/app/")
        || normalized_path.contains("/route.")
        || normalized_path.ends_with("/route.ts")
        || normalized_path.ends_with("/route.tsx")
}

fn display_is_command_entrypoint(display: &str, normalized_display: &str, path: &str) -> bool {
    if normalized_display == "main" || display.ends_with("::main") {
        return true;
    }
    if display.starts_with("Cli")
        && display
            .chars()
            .nth(3)
            .is_some_and(|ch| ch.is_uppercase() || ch == '_')
    {
        return true;
    }
    if display.contains("::Cli") || display.contains("::cli") {
        return true;
    }
    let normalized_path = packet_display_path(path).replace('\\', "/");
    if normalized_path.ends_with("/main.rs") && normalized_display == "main" {
        return true;
    }
    let lower = display.to_ascii_lowercase();
    lower.contains("commands") && !lower.contains("process")
}

fn path_contains_test_segment(path: &str) -> bool {
    path.starts_with("test/")
        || path.starts_with("tests/")
        || path.contains("/test/")
        || path.contains("/tests/")
        || path.contains("-test-")
        || path.contains("_test_")
        || path.contains("_tests.")
        || path.starts_with("test\\")
        || path.starts_with("tests\\")
        || path.contains("\\test\\")
        || path.contains("\\tests\\")
}
