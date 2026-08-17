use crate::packet_evidence_carriers::citation_owns_indexing_entrypoint;
use crate::packet_scoring::{
    normalize_identifier, packet_display_name_is_test_like, packet_display_path,
};
use crate::text::retrieval_file_role_from_path;
use codestory_contracts::api::{AgentCitationDto, NodeKind, SearchHitOrigin};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PacketEvidenceRole {
    SqlTableDefinition,
    SqlRelationshipConstraint,
    SqlSchemaFile,
    TestsAndRegressionCoverage,
    IndexInputConfiguration,
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

pub fn packet_citation_owns_request_pipeline(citation: &AgentCitationDto) -> bool {
    matches!(citation.kind, NodeKind::FUNCTION | NodeKind::METHOD)
        && crate::text::terminal_symbol_segment(&citation.display_name) == "request"
}

pub fn packet_citation_owns_interceptor_management(citation: &AgentCitationDto) -> bool {
    let owner_kind = matches!(citation.kind, NodeKind::STRUCT | NodeKind::CLASS)
        || (citation.kind == NodeKind::METHOD
            && matches!(
                crate::text::terminal_symbol_segment(&citation.display_name).as_str(),
                "constructor" | "init" | "new"
            ));
    if !owner_kind {
        return false;
    }
    let display = normalize_identifier(&citation.display_name);
    display.contains("interceptor")
        && ["manager", "registry", "collection", "chain"]
            .iter()
            .any(|owner| display.contains(owner))
}

pub fn packet_citation_owns_transport_adapter(citation: &AgentCitationDto) -> bool {
    if !matches!(
        citation.kind,
        NodeKind::FUNCTION | NodeKind::METHOD | NodeKind::CLASS | NodeKind::STRUCT
    ) {
        return false;
    }
    let display = normalize_identifier(&citation.display_name);
    if !display.contains("adapter") {
        return false;
    }
    let terminal = normalize_identifier(&crate::text::terminal_symbol_segment(
        &citation.display_name,
    ));
    if matches!(citation.kind, NodeKind::CLASS | NodeKind::STRUCT) {
        // A type whose name merely ends in "adapter" is `ArrayAdapter`, `ListAdapter`,
        // `RecyclerViewAdapter` — the most populated class-name suffix in mobile and UI code, and
        // none of them is a transport. The requirements that list this role scope themselves with a
        // word list that also contains "adapter", so accepting the suffix alone let one word
        // satisfy both of their factors. The transport has to be named beside it, the way a real
        // one is named for the protocol or the socket it speaks over.
        return terminal.ends_with("adapter")
            && [
                "http",
                "https",
                "xhr",
                "fetch",
                "transport",
                "request",
                "client",
                "socket",
                "net",
            ]
            .iter()
            .any(|transport| display.contains(transport));
    }
    [
        "select", "get", "resolve", "choose", "create", "build", "send",
    ]
    .iter()
    .any(|operation| terminal.starts_with(operation))
}

impl PacketEvidenceRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SqlTableDefinition => "sql table definition",
            Self::SqlRelationshipConstraint => "sql relationship constraint",
            Self::SqlSchemaFile => "sql schema file",
            Self::TestsAndRegressionCoverage => "tests and regression coverage",
            Self::IndexInputConfiguration => "index input configuration",
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

    pub fn is_low_priority_cap_role(self) -> bool {
        matches!(self, Self::TestsAndRegressionCoverage)
    }
}

pub fn packet_evidence_role(citation: &AgentCitationDto) -> Option<PacketEvidenceRole> {
    if citation.kind == NodeKind::FILE && citation.origin == SearchHitOrigin::TextMatch {
        return None;
    }
    let display = citation.display_name.to_ascii_lowercase();
    let normalized_display = normalize_identifier(&citation.display_name);
    let display_tokens = crate::text::symbol_query_tokens(&citation.display_name);
    let names = |token: &str| display_tokens.iter().any(|value| value == token);
    let path = citation
        .file_path
        .as_deref()
        .map(packet_display_path)
        .unwrap_or_default()
        .to_ascii_lowercase();
    let behavioral_node = matches!(
        citation.kind,
        NodeKind::FUNCTION | NodeKind::METHOD | NodeKind::MACRO
    );

    if path.ends_with(".sql")
        && (normalized_display.starts_with("createtable")
            || (citation.kind == NodeKind::CLASS
                && citation
                    .evidence_producer
                    .as_deref()
                    .is_some_and(|producer| producer.contains("structural_sql"))))
    {
        Some(PacketEvidenceRole::SqlTableDefinition)
    } else if path.ends_with(".sql")
        && display_is_sql_relationship_constraint(&citation.display_name)
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
    } else if packet_citation_owns_indexer_source_extraction(citation, &path) {
        Some(PacketEvidenceRole::SymbolExtraction)
    } else if citation_owns_indexing_entrypoint(citation)
        || (names("task") && names("indexer") && names("queue"))
        || (names("indexer") && names("command"))
        // The mirror of the `search` + `entrypoint` clause below. Until this existed an indexing
        // entrypoint could only be recognised by the directory it sat in, so `runtime/` handed the
        // role to everything filed there and to nothing that named itself.
        || (normalized_display.contains("index") && normalized_display.contains("entrypoint"))
    {
        Some(PacketEvidenceRole::IndexingWorkQueue)
    } else if normalized_display.contains("interceptor") || path.contains("interceptor") {
        Some(PacketEvidenceRole::InterceptorManagement)
    } else if (behavioral_node && display_is_process_transport_dispatch(&normalized_display))
        || ((normalized_display.contains("dispatch")
            || path.contains("/dispatch")
            || path.contains("_dispatch"))
            && !normalized_display.contains("event"))
    {
        Some(PacketEvidenceRole::RequestDispatch)
    } else if packet_citation_owns_transport_adapter(citation) {
        Some(PacketEvidenceRole::TransportAdapter)
    } else if (normalized_display.contains("factory") || normalized_display.contains("create"))
        && (normalized_display.contains("client") || normalized_display.contains("instance"))
    {
        Some(PacketEvidenceRole::ClientFactory)
    } else if normalized_display.contains("eventloop")
        || normalized_display.contains("event_loop")
        || (normalized_display.contains("event") && normalized_display.contains("poll"))
        || (normalized_display.contains("event") && normalized_display.contains("dispatch"))
        || (normalized_display.contains("events")
            && normalized_display.contains("process")
            && !normalized_display.contains("processor"))
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
    } else if citation_owns_search_execution(
        &citation.display_name,
        &display_tokens,
        behavioral_node,
        &normalized_display,
        &path,
    ) {
        Some(PacketEvidenceRole::SearchExecutionUnit)
    } else if normalized_display.contains("candidate")
        && (normalized_display.contains("file") || normalized_display.contains("source"))
    {
        Some(PacketEvidenceRole::CandidateFileConstruction)
    } else if behavioral_node
        && normalized_display.contains("search")
        && (normalized_display.contains("driver")
            || normalized_display.contains("entrypoint")
            || normalized_display.contains("parallel")
            || names("run")
            || display_is_command_entrypoint(&citation.display_name, &normalized_display, &path))
    {
        Some(PacketEvidenceRole::SearchDriver)
    } else if behavioral_node
        && (display_is_command_entrypoint(&citation.display_name, &normalized_display, &path)
            || display_is_process_transport_entrypoint(&normalized_display))
    {
        Some(PacketEvidenceRole::CommandEntrypoint)
    } else if names("event")
        && display_tokens.iter().any(|token| {
            matches!(
                token.as_str(),
                "output" | "serialize" | "serializer" | "write" | "writer" | "emit" | "emitter"
            )
        })
    {
        Some(PacketEvidenceRole::EventOutputProcessing)
    } else if ((display.contains("thread") || display.contains("turn"))
        && display.contains("startparams"))
        // The name each ecosystem gives the server-to-application gateway. Without these the role
        // was reachable only through a `protocol/` directory, which meant it was granted to every
        // symbol filed there and to no symbol that actually says what it is.
        || normalized_display.contains("wsgi")
        || normalized_display.contains("asgi")
        || normalized_display.contains("servlet")
        || path.contains("/protocol/")
    {
        Some(PacketEvidenceRole::AppServerRequestProtocol)
    } else if behavioral_node
        && (display.contains("service")
            || display.contains("orchestrat")
            || display.contains("runtime")
            || path.contains("runtime"))
    {
        Some(PacketEvidenceRole::RuntimeOrchestration)
    } else if display.contains("manifest") || display.contains("plan") || path.contains("workspace")
    {
        Some(PacketEvidenceRole::WorkspaceDiscoveryAndPlanning)
    } else if display.contains("snapshot") || display.contains("refresh") {
        Some(PacketEvidenceRole::SnapshotRefresh)
    } else if packet_display_is_runtime_formatting_arg_store(&citation.display_name) {
        Some(PacketEvidenceRole::SourceEvidence)
    } else if behavioral_node
        && (display.contains("projection")
            || display.contains("persist")
            || display.contains("storage")
            || display.contains("store")
            || path.contains("store"))
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
        && retrieval_file_role_from_path(&path) == crate::text::RetrievalFileRole::Source
    {
        Some(PacketEvidenceRole::SourceEvidence)
    } else {
        None
    }
}

fn citation_owns_search_execution(
    display_name: &str,
    tokens: &[String],
    behavioral_node: bool,
    normalized_display: &str,
    path: &str,
) -> bool {
    if !behavioral_node
        || display_is_command_entrypoint(display_name, normalized_display, path)
        || (tokens.iter().any(|token| token == "run")
            && tokens.iter().any(|token| token == "search"))
        || tokens.last().is_some_and(|token| {
            matches!(
                token.as_str(),
                "config"
                    | "configuration"
                    | "id"
                    | "key"
                    | "location"
                    | "metadata"
                    | "options"
                    | "status"
            )
        })
    {
        return false;
    }
    let subject = tokens.iter().any(|token| {
        matches!(
            token.as_str(),
            "candidate" | "packet" | "retrieval" | "search" | "semantic" | "sidecar"
        )
    });
    let action = tokens.iter().any(|token| {
        matches!(
            token.as_str(),
            "batch"
                | "execute"
                | "expand"
                | "fuse"
                | "fused"
                | "query"
                | "rank"
                | "retrieve"
                | "run"
                | "scan"
                | "search"
                | "worker"
                | "runner"
                | "executor"
        )
    });
    let distinct_action = tokens.iter().any(|token| {
        matches!(
            token.as_str(),
            "batch"
                | "execute"
                | "expand"
                | "fuse"
                | "fused"
                | "query"
                | "rank"
                | "retrieve"
                | "run"
                | "scan"
                | "worker"
                | "runner"
                | "executor"
        )
    }) || (tokens.iter().any(|token| token == "search")
        && tokens.iter().any(|token| {
            matches!(
                token.as_str(),
                "candidate" | "packet" | "retrieval" | "semantic" | "sidecar"
            )
        }));
    subject && action && distinct_action
}

/// A per-source indexing action implemented by the indexer is extraction work, even though the
/// same callable shape is also a valid indexing entrypoint in a service or work queue. Repository,
/// project, and workspace actions stay entrypoints; only concrete source units take this narrower
/// role.
fn packet_citation_owns_indexer_source_extraction(citation: &AgentCitationDto, path: &str) -> bool {
    if !matches!(
        citation.kind,
        NodeKind::FUNCTION | NodeKind::METHOD | NodeKind::MACRO
    ) || !path
        .split('/')
        .any(|segment| matches!(segment, "indexer" | "codestory-indexer"))
    {
        return false;
    }

    let terminal = citation
        .display_name
        .rsplit([':', '.', '/', '\\'])
        .next()
        .unwrap_or(&citation.display_name);
    let tokens = crate::text::symbol_query_tokens(terminal);
    let action_width = match tokens.as_slice() {
        [action, ..] if matches!(action.as_str(), "index" | "reindex") => 1,
        [prefix, action, ..] if prefix == "re" && action == "index" => 2,
        _ => return false,
    };
    let object_tokens = &tokens[action_width..];
    if object_tokens.iter().any(|object| {
        matches!(
            object.as_str(),
            "project" | "projects" | "repository" | "repositories" | "workspace" | "workspaces"
        )
    }) {
        return false;
    }
    object_tokens.iter().any(|object| {
        matches!(
            object.as_str(),
            "file"
                | "files"
                | "document"
                | "documents"
                | "source"
                | "sources"
                | "record"
                | "records"
        )
    })
}

pub fn packet_claim_key_for_citation(
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

fn packet_display_is_runtime_formatting_arg_store(display: &str) -> bool {
    let tokens = crate::text::symbol_query_tokens(display);
    tokens
        .iter()
        .any(|token| matches!(token.as_str(), "format" | "formatting"))
        && tokens
            .iter()
            .any(|token| matches!(token.as_str(), "arg" | "args" | "argument" | "arguments"))
        && tokens
            .iter()
            .any(|token| matches!(token.as_str(), "store" | "storage"))
}

fn display_is_sql_relationship_constraint(display: &str) -> bool {
    let tokens = crate::text::symbol_query_tokens(display);
    let has = |needle: &str| tokens.iter().any(|token| token == needle);
    (has("foreign") && has("key"))
        || has("reference")
        || has("references")
        || (has("constraint") && (has("foreign") || has("referential")))
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
    if display
        .split("::")
        .any(|segment| matches!(segment, "Cli" | "cli"))
    {
        return true;
    }
    let normalized_path = packet_display_path(path).replace('\\', "/");
    if normalized_path.ends_with("/main.rs") && normalized_display == "main" {
        return true;
    }
    let lower = display.to_ascii_lowercase();
    lower.contains("commands") && !lower.contains("process")
}

fn display_is_process_transport_dispatch(normalized_display: &str) -> bool {
    normalized_display_starts_with_any(normalized_display, &["spawn", "launch", "handoff"])
        && normalized_display_contains_any(normalized_display, &["stdio", "stdin", "stdout", "ipc"])
        && normalized_display_contains_any(normalized_display, &["runtime", "server", "process"])
}

fn display_is_process_transport_entrypoint(normalized_display: &str) -> bool {
    normalized_display_starts_with_any(normalized_display, &["run", "serve", "start"])
        && normalized_display_contains_any(normalized_display, &["stdio", "stdin", "stdout", "ipc"])
        && normalized_display_contains_any(normalized_display, &["runtime", "server", "process"])
}

fn normalized_display_starts_with_any(normalized_display: &str, needles: &[&str]) -> bool {
    needles
        .iter()
        .any(|needle| normalized_display.starts_with(needle))
}

fn normalized_display_contains_any(normalized_display: &str, needles: &[&str]) -> bool {
    needles
        .iter()
        .any(|needle| normalized_display.contains(needle))
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
            target: None,
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
    fn whole_file_text_match_cannot_claim_a_symbol_role() {
        let mut citation = citation("request_dispatch", "src/request_dispatch.rs");
        citation.kind = NodeKind::FILE;
        citation.origin = SearchHitOrigin::TextMatch;

        assert_eq!(packet_evidence_role(&citation), None);
    }

    #[test]
    fn behavioral_roles_reject_named_types_and_runtime_variables() {
        for (name, path, kind, rejected_role) in [
            (
                "CliErrorBody",
                "src/cli/errors.rs",
                NodeKind::STRUCT,
                PacketEvidenceRole::CommandEntrypoint,
            ),
            (
                "runtime_path",
                "src/runtime/config.rs",
                NodeKind::VARIABLE,
                PacketEvidenceRole::RuntimeOrchestration,
            ),
            (
                "CompilationDatabase",
                "src/store/database.rs",
                NodeKind::CLASS,
                PacketEvidenceRole::PersistenceAndSearchProjection,
            ),
        ] {
            let mut candidate = citation(name, path);
            candidate.kind = kind;
            assert_ne!(packet_evidence_role(&candidate), Some(rejected_role));
        }
    }

    #[test]
    fn cli_entrypoint_detection_requires_a_complete_segment() {
        assert_ne!(
            packet_evidence_role(&citation(
                "ScenarioRunner::client_death",
                "src/scenario_runner.rs"
            )),
            Some(PacketEvidenceRole::CommandEntrypoint)
        );
        assert_eq!(
            packet_evidence_role(&citation("Application::Cli::run", "src/commands.rs")),
            Some(PacketEvidenceRole::CommandEntrypoint)
        );
    }

    #[test]
    fn process_transport_symbols_have_behavioral_dispatch_and_entrypoint_roles() {
        assert_eq!(
            packet_evidence_role(&citation("spawnStdioRuntime", "src/plugin/launcher.js")),
            Some(PacketEvidenceRole::RequestDispatch)
        );
        assert_eq!(
            packet_evidence_role(&citation("run_stdio_server", "src/transport.rs")),
            Some(PacketEvidenceRole::CommandEntrypoint)
        );
        assert!(
            !matches!(
                packet_evidence_role(&citation("stdioRuntimeConfig", "src/config.rs")),
                Some(PacketEvidenceRole::RequestDispatch | PacketEvidenceRole::CommandEntrypoint)
            ),
            "configuration names must not claim process-launch behavior"
        );
    }

    #[test]
    fn search_execution_role_requires_behavioral_action_and_subject() {
        for name in [
            "LiveSidecarSearch::semantic_search",
            "AppController::search_packet_fused_batch",
            "candidate_search_executor",
        ] {
            assert_eq!(
                packet_evidence_role(&citation(name, "src/retrieval/search.rs")),
                Some(PacketEvidenceRole::SearchExecutionUnit),
                "{name}"
            );
        }
        for name in [
            "search_hit_location_key",
            "SearchStatus",
            "semantic_search_config",
        ] {
            assert_ne!(
                packet_evidence_role(&citation(name, "src/retrieval/search.rs")),
                Some(PacketEvidenceRole::SearchExecutionUnit),
                "{name} is search data, not execution"
            );
        }
        assert_eq!(
            packet_evidence_role(&citation("run_search", "src/commands.rs")),
            Some(PacketEvidenceRole::SearchDriver),
            "the command entrypoint remains the driver instead of being consumed by dispatch"
        );
    }

    #[test]
    fn indexing_entrypoint_role_recognizes_generic_run_shapes() {
        for (name, kind) in [
            ("run_index", NodeKind::FUNCTION),
            (
                "IndexService::run_indexing_blocking_without_runtime_refresh",
                NodeKind::METHOD,
            ),
            ("BuildIndex::run", NodeKind::METHOD),
            ("index_file", NodeKind::FUNCTION),
            ("build_index", NodeKind::FUNCTION),
            ("create_index", NodeKind::FUNCTION),
            ("write_index", NodeKind::FUNCTION),
            ("persist_index", NodeKind::FUNCTION),
            ("rebuild_index", NodeKind::FUNCTION),
            ("reindex_files", NodeKind::FUNCTION),
            ("re_index_files", NodeKind::FUNCTION),
            ("IndexWriter::write", NodeKind::METHOD),
            ("SearchIndex::build", NodeKind::METHOD),
        ] {
            let mut candidate = citation(name, "src/services.rs");
            candidate.kind = kind;
            assert_eq!(
                packet_evidence_role(&candidate),
                Some(PacketEvidenceRole::IndexingWorkQueue),
                "{name}",
            );
        }
        for name in [
            "run_indexed_query",
            "SearchIndex::execute_query",
            "IndexReader::read",
            "IndexReader::read_index",
            "IndexLookup::run",
            "execute_index_query",
            "query_index",
            "search_index",
            "lookup_index",
            "scan_index",
            "fetch_index",
            "get_index",
            "list_index",
            "inspect_index",
            "index",
            "cache_index",
            "build_files",
            "create_files",
        ] {
            let mut candidate = citation(name, "src/search.rs");
            candidate.kind = NodeKind::METHOD;
            assert_ne!(
                packet_evidence_role(&candidate),
                Some(PacketEvidenceRole::IndexingWorkQueue),
                "{name} must remain a read-side role",
            );
        }
    }

    #[test]
    fn indexing_entrypoint_role_preserves_indexer_owned_source_extraction() {
        for name in [
            "index_file",
            "indexFile",
            "reindex_files",
            "re_index_sources",
            "index_structural_file",
            "index_structural_source",
            "index_template_file",
            "index_text_only_file",
            "index_openapi_schema_file",
        ] {
            assert_eq!(
                packet_evidence_role(&citation(
                    name,
                    "crates/codestory-indexer/src/extraction.rs",
                )),
                Some(PacketEvidenceRole::SymbolExtraction),
                "{name} is per-source extraction when the indexer owns it",
            );
        }

        assert_eq!(
            packet_evidence_role(&citation(
                "index_file",
                "crates/codestory-runtime/src/services.rs"
            )),
            Some(PacketEvidenceRole::IndexingWorkQueue),
            "the same callable shape remains an entrypoint when a runtime service owns it",
        );
        for name in [
            "index_project",
            "index_repository",
            "index_workspace",
            "WorkspaceIndexer::run",
        ] {
            assert_eq!(
                packet_evidence_role(&citation(name, "crates/codestory-indexer/src/lib.rs")),
                Some(PacketEvidenceRole::IndexingWorkQueue),
                "{name} describes project/work-queue scope, not per-source extraction",
            );
        }
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

        let mut structural_table = citation("public.Invoice", "db/schema.sql");
        structural_table.kind = NodeKind::CLASS;
        structural_table.evidence_producer =
            Some("verified_structural_sql_collector_source_read".to_string());
        assert_eq!(
            packet_evidence_role(&structural_table),
            Some(PacketEvidenceRole::SqlTableDefinition)
        );
    }

    #[test]
    fn transport_adapter_role_requires_a_behavior_owner() {
        assert_eq!(
            packet_evidence_role(&citation("selectAdapter", "src/client/adapters/select.ts")),
            Some(PacketEvidenceRole::TransportAdapter)
        );
        assert_eq!(
            packet_evidence_role(&citation(
                "isResolvedHandle",
                "src/client/adapters/select.ts"
            )),
            Some(PacketEvidenceRole::SourceEvidence)
        );
        assert_eq!(
            packet_evidence_role(&citation("TargetAdapter", "src/client/target.ts")),
            Some(PacketEvidenceRole::SourceEvidence)
        );
        for display_name in ["AdapterOptions", "HttpAdapterConfig"] {
            let mut citation = citation(display_name, "src/client/config.ts");
            citation.kind = NodeKind::CLASS;
            assert_eq!(
                packet_evidence_role(&citation),
                None,
                "configuration type must not own transport behavior: {display_name}"
            );
        }
    }
}
