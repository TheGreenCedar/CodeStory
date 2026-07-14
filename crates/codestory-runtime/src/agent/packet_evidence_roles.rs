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
    BufferedIo,
    CollectionConfiguration,
    SourceEvidence,
}

pub(crate) fn packet_citation_owns_request_pipeline(citation: &AgentCitationDto) -> bool {
    matches!(citation.kind, NodeKind::FUNCTION | NodeKind::METHOD)
        && crate::terminal_symbol_segment(&citation.display_name) == "request"
}

pub(crate) fn packet_citation_owns_interceptor_management(citation: &AgentCitationDto) -> bool {
    if !matches!(citation.kind, NodeKind::STRUCT | NodeKind::CLASS) {
        return false;
    }
    let display = normalize_identifier(&citation.display_name);
    display.contains("interceptor")
        && ["manager", "registry", "collection", "chain"]
            .iter()
            .any(|owner| display.contains(owner))
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
            Self::SearchExecutionUnit => "search execution unit",
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
            Self::BufferedIo => "buffered io",
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
    } else if path.ends_with(".sql") && display_is_sql_relationship_constraint(&normalized_display)
    {
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
        || (normalized_display.contains("task")
            && normalized_display.contains("indexer")
            && normalized_display.contains("queue"))
        || normalized_display.contains("indexercommand")
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
    } else if (display.contains("event") && display.contains("processor"))
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
    } else if packet_display_is_runtime_formatting_arg_store(&normalized_display) {
        Some(PacketEvidenceRole::SourceEvidence)
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
        || packet_display_or_path_is_route_dispatch(&normalized_display, &path)
        || packet_path_is_route_like(&path)
    {
        Some(PacketEvidenceRole::RouteHandling)
    } else if packet_display_or_path_is_buffered_io(&normalized_display, &path) {
        Some(PacketEvidenceRole::BufferedIo)
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

fn packet_display_is_runtime_formatting_arg_store(normalized_display: &str) -> bool {
    normalized_display.contains("formatargstore")
}

fn display_is_sql_relationship_constraint(normalized_display: &str) -> bool {
    normalized_display == "foreignkey"
        || normalized_display == "references"
        || normalized_display.contains("foreignkey")
        || normalized_display.contains("references")
        || (normalized_display.contains("constraint")
            && (normalized_display.contains("foreign") || normalized_display.contains("refer")))
}

fn packet_display_or_path_is_route_dispatch(normalized_display: &str, path: &str) -> bool {
    if normalized_display.contains("add") && normalized_display.contains("route") {
        return true;
    }
    if normalized_display.contains("handle")
        && (normalized_display.contains("request") || normalized_display.contains("http"))
    {
        return true;
    }
    if normalized_display.contains("combine") && normalized_display.contains("handler") {
        return true;
    }
    normalized_display.ends_with("next") && packet_file_stem(path).contains("context")
}

fn packet_display_or_path_is_buffered_io(normalized_display: &str, path: &str) -> bool {
    let file_stem = packet_file_stem(path);
    let display_has_buffer = normalized_display.contains("buffer");
    let display_has_io_peer = normalized_display.contains("source")
        || normalized_display.contains("sink")
        || normalized_display.contains("read")
        || normalized_display.contains("write")
        || normalized_display.contains("emit")
        || normalized_display.contains("flush");
    if display_has_buffer && (display_has_io_peer || file_stem.contains("buffer")) {
        return true;
    }
    if matches!(
        file_stem.as_str(),
        "buffer" | "bufferedsource" | "bufferedsink"
    ) {
        return true;
    }
    matches!(normalized_display, "source" | "sink")
        && matches!(file_stem.as_str(), "source" | "sink")
}

fn packet_file_stem(path: &str) -> String {
    let file_name = path
        .rsplit(['/', '\\'])
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or(path);
    file_name
        .split('.')
        .next()
        .map(normalize_identifier)
        .unwrap_or_default()
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

#[cfg(test)]
mod tests {
    use super::*;
    use codestory_contracts::api::{NodeId, NodeKind, RetrievalScoreBreakdownDto, SearchHitOrigin};

    fn citation(display_name: &str, file_path: &str) -> AgentCitationDto {
        AgentCitationDto {
            node_id: NodeId(display_name.to_string()),
            display_name: display_name.to_string(),
            kind: NodeKind::FUNCTION,
            file_path: Some(file_path.to_string()),
            line: Some(1),
            score: 1.0,
            origin: SearchHitOrigin::IndexedSymbol,
            resolvable: true,
            subgraph_id: None,
            evidence_edge_ids: Vec::new(),
            retrieval_score_breakdown: Some(RetrievalScoreBreakdownDto {
                lexical: 1.0,
                semantic: 0.0,
                graph: 0.0,
                total: 1.0,
                tier_cap: None,
                boosts: Vec::new(),
                dampening: Vec::new(),
                final_rank_reason: None,
                provenance: Vec::new(),
            }),
            evidence_tier: None,
            evidence_producer: None,
            resolution_status: None,
            loss_reason: None,
            coverage_role: None,
            eligible_for_sufficiency: None,
        }
    }

    #[test]
    fn buffered_io_role_matches_api_peers_without_path_literals() {
        assert_eq!(
            packet_evidence_role(&citation(
                "BufferedReaderImpl",
                "src/io/buffered_reader_impl.kt"
            )),
            Some(PacketEvidenceRole::BufferedIo)
        );
        assert_eq!(
            packet_evidence_role(&citation("Buffer", "src/io/buffer.kt")),
            Some(PacketEvidenceRole::BufferedIo)
        );
        assert_eq!(
            packet_evidence_role(&citation("Source", "src/io/source.kt")),
            Some(PacketEvidenceRole::BufferedIo)
        );
    }

    #[test]
    fn route_role_matches_dispatch_shapes_without_path_literals() {
        assert_eq!(
            packet_evidence_role(&citation("Server.handleHttpRequest", "src/http/server.go")),
            Some(PacketEvidenceRole::RouteHandling)
        );
        assert_eq!(
            packet_evidence_role(&citation("node.addRoute", "src/tree.go")),
            Some(PacketEvidenceRole::RouteHandling)
        );
        assert_eq!(
            packet_evidence_role(&citation("RequestContext.Next", "src/context.go")),
            Some(PacketEvidenceRole::RouteHandling)
        );
    }

    #[test]
    fn runtime_format_arg_store_is_source_evidence_not_persistence() {
        assert_eq!(
            packet_evidence_role(&citation("format_arg_store", "include/fmt/base.h")),
            Some(PacketEvidenceRole::SourceEvidence)
        );
        assert_eq!(
            packet_evidence_role(&citation("dynamic_format_arg_store", "include/fmt/args.h")),
            Some(PacketEvidenceRole::SourceEvidence)
        );
    }

    #[test]
    fn sql_relationship_role_matches_reference_and_constraint_anchors() {
        for display_name in [
            "FOREIGN KEY",
            "REFERENCES",
            "CONSTRAINT fk_child_parent FOREIGN KEY",
            "fk_order_customer references",
        ] {
            assert_eq!(
                packet_evidence_role(&citation(display_name, "db/schema.sql")),
                Some(PacketEvidenceRole::SqlRelationshipConstraint),
                "expected SQL relationship role for {display_name}"
            );
        }

        assert_eq!(
            packet_evidence_role(&citation("CHECK constraint", "db/schema.sql")),
            Some(PacketEvidenceRole::SqlSchemaFile)
        );
    }
}
