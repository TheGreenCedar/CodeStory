//! Generic packet flow requirements shared by planning, probes, and sufficiency.

use crate::agent::packet_evidence_carriers::{
    citation_owns_buffer_read_write, citation_owns_buffer_storage,
    citation_owns_client_request_finalization, citation_owns_client_request_method,
    citation_owns_client_response_materialization, citation_owns_css_animation_entrypoint,
    citation_owns_css_animation_structure, citation_owns_css_structure,
    citation_owns_form_custom_validation, citation_owns_form_native_constraint,
    citation_owns_form_submit_guard, citation_owns_format_arguments,
    citation_owns_formatter_fallback, citation_owns_hook_cache_helper,
    citation_owns_hook_key_serialization, citation_owns_hook_mutation_flow,
    citation_owns_hook_public_export, citation_owns_html_app_shell,
    citation_owns_log_handler_processing, citation_owns_log_record_creation,
    citation_owns_mapper_configuration, citation_owns_mapper_execution,
    citation_owns_shell_completion, citation_owns_shell_function_dispatch,
    citation_owns_shell_installer_bootstrap, citation_owns_site_lifecycle,
    citation_owns_site_terminal, flow_belongs_to_client_request, flow_belongs_to_command_dispatch,
    flow_belongs_to_command_server, flow_belongs_to_event_loop, flow_belongs_to_indexing,
    flow_belongs_to_network_input, flow_belongs_to_request_terminal, flow_belongs_to_search,
    flow_belongs_to_server_request, flow_belongs_to_sql_schema, flow_belongs_to_url_session,
};
use crate::agent::packet_evidence_roles::{
    PacketEvidenceRole, packet_citation_owns_interceptor_management, packet_evidence_role,
};
use crate::agent::packet_terms::{
    packet_terms_have_any, packet_terms_indicate_buffered_io_flow,
    packet_terms_indicate_client_send_flow, packet_terms_indicate_command_dispatch_flow,
    packet_terms_indicate_command_event_loop_flow,
    packet_terms_indicate_command_server_bootstrap_flow,
    packet_terms_indicate_event_loop_command_flow, packet_terms_indicate_form_validation_flow,
    packet_terms_indicate_hook_cache_flow, packet_terms_indicate_html_css_template_structure_flow,
    packet_terms_indicate_indexing_flow, packet_terms_indicate_log_record_handler_flow,
    packet_terms_indicate_mapper_configuration_plan_flow,
    packet_terms_indicate_network_command_input_flow, packet_terms_indicate_request_dispatch_flow,
    packet_terms_indicate_runtime_formatting_flow, packet_terms_indicate_search_execution_flow,
    packet_terms_indicate_server_request_dispatch_flow,
    packet_terms_indicate_shell_install_dispatch_flow, packet_terms_indicate_site_build_phase_flow,
    packet_terms_indicate_sql_schema_flow, packet_terms_indicate_stylesheet_animation_flow,
    packet_terms_indicate_url_session_request_flow,
};
use codestory_contracts::api::{AgentCitationDto, PacketTaskClassDto};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum FlowRole {
    Entrypoint,
    Registration,
    Configuration,
    StateOrStorage,
    Dispatch,
    TransformOrValidate,
    TerminalBoundary,
    ErrorOrFallback,
}

impl FlowRole {
    #[cfg(test)]
    pub(crate) const fn role_id(self) -> &'static str {
        match self {
            Self::Entrypoint => "entrypoint",
            Self::Registration => "registration",
            Self::Configuration => "configuration",
            Self::StateOrStorage => "state_or_storage",
            Self::Dispatch => "dispatch",
            Self::TransformOrValidate => "transform_or_validate",
            Self::TerminalBoundary => "terminal_boundary",
            Self::ErrorOrFallback => "error_or_fallback",
        }
    }

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Entrypoint => "entrypoint",
            Self::Registration => "registration",
            Self::Configuration => "configuration",
            Self::StateOrStorage => "state/storage",
            Self::Dispatch => "dispatch",
            Self::TransformOrValidate => "transform/validate",
            Self::TerminalBoundary => "terminal boundary",
            Self::ErrorOrFallback => "error/fallback",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CoverageMode {
    RequiresResolvedSourceOrGraph,
    AllowsSourceRange,
    AllowsLexicalSource,
    DiagnosticOnly,
}

/// What a packet must actually have *cited* for a requirement to count as covered.
///
/// A requirement's `FlowRole` describes where it sits in a flow; it is a label, not a test. Two
/// requirements in one flow may share a role, so matching on the role alone let evidence for one
/// close the other. An evidence predicate belongs to a single requirement and reads only the
/// citation, never the claim's wording.
#[derive(Debug, Clone, Copy)]
pub(crate) enum EvidencePredicate {
    /// Covered by a citation the evidence-role classifier places in this part of the flow *and*
    /// that belongs to the subsystem this flow is about.
    ///
    /// The role alone is not enough. The classifier answers a ranking question — "what kind of
    /// evidence is this" — and much of it reads the path: every symbol under `runtime/` is runtime
    /// orchestration, every symbol under `app/` or `views/` is route handling, every symbol under
    /// `flags/` is argument planning. Without the subsystem factor a requirement inherited every
    /// symbol filed in those directories, so `renderChart` in `src/views/` proved a server's
    /// request entrypoint and `Store.delete` proved an indexer's persistence step.
    CitedRoles {
        subsystem: fn(&AgentCitationDto) -> bool,
        roles: &'static [PacketEvidenceRole],
    },
    /// Covered by a citation that passes a structural ownership check, used where the evidence
    /// role is too coarse to separate a requirement from its siblings. The carriers carry their own
    /// subsystem factor.
    CitedCarrier(fn(&AgentCitationDto) -> bool),
}

impl EvidencePredicate {
    pub(crate) fn citation_proves(self, citation: &AgentCitationDto) -> bool {
        match self {
            Self::CitedRoles { subsystem, roles } => {
                subsystem(citation)
                    && packet_evidence_role(citation).is_some_and(|role| roles.contains(&role))
                    && role_survives_without_its_path(citation, roles)
            }
            Self::CitedCarrier(carrier) => carrier(citation),
        }
    }
}

/// Whether the citation still earns one of `roles` once its path is taken away.
///
/// A path says where a symbol was filed. It cannot say what the symbol does, and the shared role
/// classifier reads it anyway: anything under `runtime/` is runtime orchestration, anything under
/// `app/`, `views/` or `pages/` is route handling, anything under `flags/` is argument planning,
/// anything under `protocol/` is the app-server request protocol. A requirement that took any role
/// the classifier produced therefore inherited every symbol filed in those directories — a symbol
/// named `request` in `src/runtime/` closed a server's dispatch step, and one named `handler` in
/// `app/views/` closed its entrypoint.
///
/// The **file name** is a path segment like any other and the classifier reads it the same way, so
/// stripping only the directories left the defect one level down: `runtime.c`, `store.ts`,
/// `signal_dispatch.rs`, `*_events.jsonl` and a `buffer` stem each still handed out a role on their
/// own, which is how `tooltipHandler` in `src/os/runtime.c` proved a server's dispatch step and
/// `SnapshotDiffViewer` in `src/ui/store.ts` proved an indexer's persistence step. So the whole
/// path goes, down to the extension.
///
/// Asking the question a second time against the bare extension makes the path a purely
/// *narrowing* factor. A `tests/` path still classifies as test coverage on the first question and
/// still fails there; the extension is still present for the `.sql` roles, which are the one place
/// a file genuinely is the evidence. Nothing else about the path can grant a role, and because the
/// full-path answer must match first, this can only reject citations that question already
/// accepted — never admit new ones.
fn role_survives_without_its_path(
    citation: &AgentCitationDto,
    roles: &[PacketEvidenceRole],
) -> bool {
    let Some(path) = citation.file_path.as_deref() else {
        return true;
    };
    let file_name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let extension = match file_name.rfind('.') {
        Some(index) => &file_name[index..],
        None => "",
    };
    let mut without_path = citation.clone();
    without_path.file_path = Some(extension.to_string());
    packet_evidence_role(&without_path).is_some_and(|role| roles.contains(&role))
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct FlowRequirement {
    pub id: &'static str,
    pub role: FlowRole,
    pub query_seeds: &'static [&'static str],
    pub coverage_mode: CoverageMode,
    pub evidence: EvidencePredicate,
}

impl FlowRequirement {
    #[cfg(test)]
    pub(crate) const fn role_id(&self) -> &'static str {
        self.role.role_id()
    }
}

pub(crate) fn packet_flow_requirements_for_terms(
    terms: &[String],
    task_class: PacketTaskClassDto,
) -> Vec<FlowRequirement> {
    if !matches!(
        task_class,
        PacketTaskClassDto::ArchitectureExplanation
            | PacketTaskClassDto::DataFlow
            | PacketTaskClassDto::ChangeImpact
            | PacketTaskClassDto::RouteTracing
            | PacketTaskClassDto::EditPlanning
    ) {
        return Vec::new();
    }

    let mut requirements = Vec::new();
    if packet_terms_indicate_indexing_flow(terms) {
        requirements.extend_from_slice(INDEXING_FLOW);
    }
    let server_request_dispatch = packet_terms_indicate_server_request_dispatch_flow(terms);
    let client_request_dispatch = packet_terms_indicate_request_dispatch_flow(terms);
    if server_request_dispatch {
        requirements.extend_from_slice(SERVER_REQUEST_DISPATCH_FLOW);
    } else if client_request_dispatch {
        requirements.extend_from_slice(CLIENT_REQUEST_DISPATCH_FLOW);
        if packet_terms_have_any(terms, &["interceptor", "interceptors"]) {
            requirements.push(REQUEST_INTERCEPTOR_REQUIREMENT);
        }
    }
    if packet_terms_indicate_client_send_flow(terms) {
        push_client_send_requirements_for_terms(terms, &mut requirements);
    }
    if packet_terms_indicate_hook_cache_flow(terms) {
        push_hook_cache_requirements_for_terms(terms, &mut requirements);
    }
    if packet_terms_indicate_event_loop_command_flow(terms) {
        push_command_loop_requirements_for_terms(terms, &mut requirements);
    }
    if packet_terms_indicate_url_session_request_flow(terms) {
        requirements.extend_from_slice(URL_SESSION_FLOW);
    }
    if packet_terms_indicate_sql_schema_flow(terms) {
        requirements.extend_from_slice(SQL_SCHEMA_FLOW);
    }
    if packet_terms_indicate_html_css_template_structure_flow(terms) {
        requirements.extend_from_slice(HTML_CSS_FLOW);
    }
    if packet_terms_indicate_stylesheet_animation_flow(terms) {
        requirements.extend_from_slice(CSS_ANIMATION_FLOW);
    }
    if packet_terms_indicate_form_validation_flow(terms) {
        requirements.extend_from_slice(FORM_VALIDATION_FLOW);
    }
    if packet_terms_indicate_shell_install_dispatch_flow(terms) {
        requirements.extend_from_slice(SHELL_INSTALL_FLOW);
    }
    if packet_terms_indicate_buffered_io_flow(terms) {
        requirements.extend_from_slice(BUFFERED_IO_FLOW);
    }
    if packet_terms_indicate_log_record_handler_flow(terms) {
        requirements.extend_from_slice(LOG_HANDLER_FLOW);
    }
    if packet_terms_indicate_site_build_phase_flow(terms) {
        requirements.extend_from_slice(SITE_BUILD_FLOW);
    }
    if packet_terms_indicate_mapper_configuration_plan_flow(terms) {
        requirements.extend_from_slice(MAPPER_PLAN_FLOW);
    }
    if packet_terms_indicate_runtime_formatting_flow(terms) {
        requirements.extend_from_slice(RUNTIME_FORMATTING_FLOW);
    }
    if packet_terms_indicate_search_execution_flow(terms) {
        requirements.extend_from_slice(SEARCH_EXECUTION_FLOW);
    }
    dedupe_requirements(requirements)
}

pub(crate) fn packet_flow_requirement_queries_for_terms(
    terms: &[String],
    task_class: PacketTaskClassDto,
) -> Vec<String> {
    let mut queries = Vec::new();
    for requirement in packet_flow_requirements_for_terms(terms, task_class) {
        let _role = requirement.role;
        let _requires_source = matches!(
            requirement.coverage_mode,
            CoverageMode::RequiresResolvedSourceOrGraph
                | CoverageMode::AllowsSourceRange
                | CoverageMode::AllowsLexicalSource
        );
        for seed in requirement.query_seeds {
            if !queries.iter().any(|query| query == seed) {
                queries.push((*seed).to_string());
            }
        }
    }
    queries
}

fn dedupe_requirements(requirements: Vec<FlowRequirement>) -> Vec<FlowRequirement> {
    let mut deduped = Vec::new();
    for requirement in requirements {
        if !deduped
            .iter()
            .any(|existing: &FlowRequirement| existing.id == requirement.id)
        {
            deduped.push(requirement);
        }
    }
    deduped
}

fn push_command_loop_requirements_for_terms(
    terms: &[String],
    requirements: &mut Vec<FlowRequirement>,
) {
    if packet_terms_indicate_command_server_bootstrap_flow(terms) {
        requirements.push(COMMAND_SERVER_BOOTSTRAP_REQUIREMENT);
    }
    if packet_terms_indicate_command_event_loop_flow(terms) {
        requirements.push(COMMAND_EVENT_LOOP_REQUIREMENT);
    }
    if packet_terms_indicate_network_command_input_flow(terms) {
        requirements.push(COMMAND_NETWORK_INPUT_REQUIREMENT);
    }
    if packet_terms_indicate_command_dispatch_flow(terms) {
        requirements.push(COMMAND_DISPATCH_REQUIREMENT);
    }
}

fn push_client_send_requirements_for_terms(
    terms: &[String],
    requirements: &mut Vec<FlowRequirement>,
) {
    let has_any = |needles: &[&str]| packet_terms_have_any(terms, needles);
    if has_any(&[
        "top", "level", "public", "facade", "expose", "exposes", "api", "package",
    ]) {
        requirements.push(CLIENT_PUBLIC_FACADE_REQUIREMENT);
    }
    if has_any(&[
        "convenience",
        "conveniences",
        "method",
        "methods",
        "interface",
        "interfaces",
        "helper",
        "helpers",
    ]) && has_any(&["client", "clients", "http", "httpclient"])
    {
        requirements.push(CLIENT_INTERFACE_HELPERS_REQUIREMENT);
    }
    if has_any(&[
        "finalize",
        "finalizes",
        "finalized",
        "finalization",
        "body",
        "bodies",
        "prepare",
        "prepares",
        "prepared",
    ]) {
        requirements.push(CLIENT_REQUEST_FINALIZATION_REQUIREMENT);
    }
    if has_any(&["send", "sending", "sent"])
        || (has_any(&["transport", "transports"]) && has_any(&["implementation", "implements"]))
    {
        requirements.push(CLIENT_TRANSPORT_SEND_REQUIREMENT);
    }
    if has_any(&[
        "response",
        "responses",
        "materialize",
        "materializes",
        "materialization",
        "stream",
        "boundary",
    ]) {
        requirements.push(CLIENT_RESPONSE_MATERIALIZATION_REQUIREMENT);
    }
    if requirements
        .iter()
        .all(|requirement| !requirement.id.starts_with("client_"))
    {
        requirements.push(CLIENT_TRANSPORT_SEND_REQUIREMENT);
    }
}

fn push_hook_cache_requirements_for_terms(
    terms: &[String],
    requirements: &mut Vec<FlowRequirement>,
) {
    let has_any = |needles: &[&str]| packet_terms_have_any(terms, needles);
    requirements.push(HOOK_PUBLIC_EXPORT_REQUIREMENT);
    if has_any(&["serialize", "serializes", "serialized", "key", "keys"]) {
        requirements.push(HOOK_KEY_SERIALIZATION_REQUIREMENT);
    }
    if has_any(&["cache", "caches", "caching", "helper", "helpers"]) {
        requirements.push(HOOK_CACHE_HELPER_REQUIREMENT);
    }
    if has_any(&["mutate", "mutates", "mutation", "mutations"]) {
        requirements.push(HOOK_MUTATION_FLOW_REQUIREMENT);
    }
}

const INDEXING_FLOW: &[FlowRequirement] = &[
    FlowRequirement {
        id: "indexing_entrypoint",
        role: FlowRole::Entrypoint,
        query_seeds: &["indexing entrypoint"],
        coverage_mode: CoverageMode::RequiresResolvedSourceOrGraph,
        evidence: EvidencePredicate::CitedRoles {
            subsystem: flow_belongs_to_indexing,
            roles: &[
                PacketEvidenceRole::IndexingWorkQueue,
                PacketEvidenceRole::CommandEntrypoint,
                PacketEvidenceRole::RuntimeOrchestration,
            ],
        },
    },
    FlowRequirement {
        id: "indexing_storage",
        role: FlowRole::StateOrStorage,
        query_seeds: &["file discovery", "symbol extraction", "storage persistence"],
        coverage_mode: CoverageMode::AllowsSourceRange,
        evidence: EvidencePredicate::CitedRoles {
            subsystem: flow_belongs_to_indexing,
            roles: &[
                PacketEvidenceRole::PersistenceAndSearchProjection,
                PacketEvidenceRole::SymbolExtraction,
                PacketEvidenceRole::SnapshotRefresh,
                PacketEvidenceRole::WorkspaceDiscoveryAndPlanning,
                PacketEvidenceRole::CandidateFileConstruction,
            ],
        },
    },
];

const SERVER_REQUEST_DISPATCH_FLOW: &[FlowRequirement] = &[
    FlowRequirement {
        id: "request_entrypoint",
        role: FlowRole::Registration,
        query_seeds: &["request entrypoint", "route registration"],
        coverage_mode: CoverageMode::RequiresResolvedSourceOrGraph,
        evidence: EvidencePredicate::CitedRoles {
            subsystem: flow_belongs_to_server_request,
            roles: &[
                PacketEvidenceRole::RouteHandling,
                PacketEvidenceRole::AppServerRequestProtocol,
            ],
        },
    },
    FlowRequirement {
        id: "request_dispatch",
        role: FlowRole::Dispatch,
        query_seeds: &["request dispatch", "handler dispatch", "transport adapter"],
        coverage_mode: CoverageMode::RequiresResolvedSourceOrGraph,
        evidence: EvidencePredicate::CitedRoles {
            subsystem: flow_belongs_to_server_request,
            roles: &[
                PacketEvidenceRole::RequestDispatch,
                PacketEvidenceRole::CommandDispatch,
                PacketEvidenceRole::RuntimeOrchestration,
            ],
        },
    },
    FlowRequirement {
        id: "request_terminal",
        role: FlowRole::TerminalBoundary,
        query_seeds: &["response finalization", "transport send"],
        coverage_mode: CoverageMode::AllowsSourceRange,
        evidence: EvidencePredicate::CitedRoles {
            subsystem: flow_belongs_to_request_terminal,
            roles: &[
                PacketEvidenceRole::TransportAdapter,
                PacketEvidenceRole::EventOutputProcessing,
                PacketEvidenceRole::BufferedIo,
            ],
        },
    },
];

const CLIENT_REQUEST_DISPATCH_FLOW: &[FlowRequirement] = &[
    FlowRequirement {
        id: "request_entrypoint",
        role: FlowRole::Entrypoint,
        query_seeds: &["default instance", "request method", "request entrypoint"],
        coverage_mode: CoverageMode::RequiresResolvedSourceOrGraph,
        evidence: EvidencePredicate::CitedRoles {
            subsystem: flow_belongs_to_client_request,
            roles: &[
                PacketEvidenceRole::ClientFactory,
                PacketEvidenceRole::CommandEntrypoint,
            ],
        },
    },
    FlowRequirement {
        id: "request_dispatch",
        role: FlowRole::Dispatch,
        query_seeds: &["request dispatch", "adapters", "transport adapter"],
        coverage_mode: CoverageMode::RequiresResolvedSourceOrGraph,
        evidence: EvidencePredicate::CitedRoles {
            subsystem: flow_belongs_to_client_request,
            roles: &[PacketEvidenceRole::RequestDispatch],
        },
    },
    FlowRequirement {
        id: "request_terminal",
        role: FlowRole::TerminalBoundary,
        query_seeds: &["response finalization", "transport send"],
        coverage_mode: CoverageMode::AllowsSourceRange,
        evidence: EvidencePredicate::CitedRoles {
            subsystem: flow_belongs_to_request_terminal,
            roles: &[PacketEvidenceRole::TransportAdapter],
        },
    },
];

const REQUEST_INTERCEPTOR_REQUIREMENT: FlowRequirement = FlowRequirement {
    id: "request_interceptor_management",
    role: FlowRole::Dispatch,
    query_seeds: &["interceptor handlers", "request interceptor"],
    coverage_mode: CoverageMode::RequiresResolvedSourceOrGraph,
    evidence: EvidencePredicate::CitedCarrier(packet_citation_owns_interceptor_management),
};

const URL_SESSION_FLOW: &[FlowRequirement] = &[
    FlowRequirement {
        id: "session_request",
        role: FlowRole::Entrypoint,
        query_seeds: &["session request creation", "request task resume"],
        coverage_mode: CoverageMode::RequiresResolvedSourceOrGraph,
        evidence: EvidencePredicate::CitedRoles {
            subsystem: flow_belongs_to_url_session,
            roles: &[
                PacketEvidenceRole::ClientFactory,
                PacketEvidenceRole::AppServerRequestProtocol,
                PacketEvidenceRole::CommandEntrypoint,
            ],
        },
    },
    FlowRequirement {
        id: "session_callbacks",
        role: FlowRole::Dispatch,
        query_seeds: &["session delegate callbacks", "data request validation"],
        coverage_mode: CoverageMode::AllowsSourceRange,
        evidence: EvidencePredicate::CitedRoles {
            subsystem: flow_belongs_to_url_session,
            roles: &[
                PacketEvidenceRole::RequestDispatch,
                PacketEvidenceRole::EventLoop,
                PacketEvidenceRole::RouteHandling,
                PacketEvidenceRole::TransportAdapter,
            ],
        },
    },
];

const CLIENT_PUBLIC_FACADE_REQUIREMENT: FlowRequirement = FlowRequirement {
    id: "client_public_facade",
    role: FlowRole::Entrypoint,
    query_seeds: &["http top level helper", "public client facade"],
    coverage_mode: CoverageMode::RequiresResolvedSourceOrGraph,
    evidence: EvidencePredicate::CitedRoles {
        subsystem: flow_belongs_to_client_request,
        roles: &[PacketEvidenceRole::ClientFactory],
    },
};

const CLIENT_INTERFACE_HELPERS_REQUIREMENT: FlowRequirement = FlowRequirement {
    id: "client_interface_helpers",
    role: FlowRole::Entrypoint,
    query_seeds: &["client convenience method", "client interface helper"],
    coverage_mode: CoverageMode::RequiresResolvedSourceOrGraph,
    evidence: EvidencePredicate::CitedCarrier(citation_owns_client_request_method),
};

const CLIENT_REQUEST_FINALIZATION_REQUIREMENT: FlowRequirement = FlowRequirement {
    id: "client_request_finalization",
    role: FlowRole::TransformOrValidate,
    query_seeds: &["request finalization", "transport-ready request object"],
    coverage_mode: CoverageMode::RequiresResolvedSourceOrGraph,
    evidence: EvidencePredicate::CitedCarrier(citation_owns_client_request_finalization),
};

const CLIENT_TRANSPORT_SEND_REQUIREMENT: FlowRequirement = FlowRequirement {
    id: "client_transport_send",
    role: FlowRole::Dispatch,
    query_seeds: &["transport send", "client send implementation"],
    coverage_mode: CoverageMode::RequiresResolvedSourceOrGraph,
    evidence: EvidencePredicate::CitedRoles {
        subsystem: flow_belongs_to_client_request,
        roles: &[
            PacketEvidenceRole::TransportAdapter,
            PacketEvidenceRole::RequestDispatch,
        ],
    },
};

const CLIENT_RESPONSE_MATERIALIZATION_REQUIREMENT: FlowRequirement = FlowRequirement {
    id: "client_response_materialization",
    role: FlowRole::TerminalBoundary,
    query_seeds: &["request response", "response stream boundary"],
    coverage_mode: CoverageMode::RequiresResolvedSourceOrGraph,
    evidence: EvidencePredicate::CitedCarrier(citation_owns_client_response_materialization),
};

const HOOK_PUBLIC_EXPORT_REQUIREMENT: FlowRequirement = FlowRequirement {
    id: "hook_public_export",
    role: FlowRole::Entrypoint,
    query_seeds: &["public hook export", "hook argument wrapper"],
    coverage_mode: CoverageMode::AllowsSourceRange,
    evidence: EvidencePredicate::CitedCarrier(citation_owns_hook_public_export),
};

const HOOK_KEY_SERIALIZATION_REQUIREMENT: FlowRequirement = FlowRequirement {
    id: "hook_key_serialization",
    role: FlowRole::TransformOrValidate,
    query_seeds: &["key serialization", "serialize hook key"],
    coverage_mode: CoverageMode::AllowsSourceRange,
    evidence: EvidencePredicate::CitedCarrier(citation_owns_hook_key_serialization),
};

const HOOK_CACHE_HELPER_REQUIREMENT: FlowRequirement = FlowRequirement {
    id: "hook_cache_helper",
    role: FlowRole::StateOrStorage,
    query_seeds: &["cache helper", "cache state helper"],
    coverage_mode: CoverageMode::AllowsSourceRange,
    evidence: EvidencePredicate::CitedCarrier(citation_owns_hook_cache_helper),
};

const HOOK_MUTATION_FLOW_REQUIREMENT: FlowRequirement = FlowRequirement {
    id: "hook_mutation_flow",
    role: FlowRole::Dispatch,
    query_seeds: &["mutation helper", "mutate dispatch"],
    coverage_mode: CoverageMode::AllowsSourceRange,
    evidence: EvidencePredicate::CitedCarrier(citation_owns_hook_mutation_flow),
};

const COMMAND_SERVER_BOOTSTRAP_REQUIREMENT: FlowRequirement = FlowRequirement {
    id: "command_server_bootstrap",
    role: FlowRole::Entrypoint,
    query_seeds: &["server bootstrap", "command server entrypoint"],
    coverage_mode: CoverageMode::RequiresResolvedSourceOrGraph,
    evidence: EvidencePredicate::CitedRoles {
        subsystem: flow_belongs_to_command_server,
        roles: &[
            PacketEvidenceRole::CommandEntrypoint,
            PacketEvidenceRole::RuntimeOrchestration,
        ],
    },
};

const COMMAND_EVENT_LOOP_REQUIREMENT: FlowRequirement = FlowRequirement {
    id: "command_event_loop",
    role: FlowRole::Dispatch,
    query_seeds: &["event loop", "event loop source"],
    coverage_mode: CoverageMode::RequiresResolvedSourceOrGraph,
    evidence: EvidencePredicate::CitedRoles {
        subsystem: flow_belongs_to_event_loop,
        roles: &[PacketEvidenceRole::EventLoop],
    },
};

const COMMAND_NETWORK_INPUT_REQUIREMENT: FlowRequirement = FlowRequirement {
    id: "command_network_input",
    role: FlowRole::Dispatch,
    query_seeds: &["network input", "network command input"],
    coverage_mode: CoverageMode::RequiresResolvedSourceOrGraph,
    evidence: EvidencePredicate::CitedRoles {
        subsystem: flow_belongs_to_network_input,
        roles: &[PacketEvidenceRole::NetworkCommandInput],
    },
};

const COMMAND_DISPATCH_REQUIREMENT: FlowRequirement = FlowRequirement {
    id: "command_dispatch",
    role: FlowRole::Dispatch,
    query_seeds: &["command dispatch", "command table dispatch"],
    coverage_mode: CoverageMode::RequiresResolvedSourceOrGraph,
    evidence: EvidencePredicate::CitedRoles {
        subsystem: flow_belongs_to_command_dispatch,
        roles: &[
            PacketEvidenceRole::CommandDispatch,
            PacketEvidenceRole::RequestDispatch,
        ],
    },
};

const SQL_SCHEMA_FLOW: &[FlowRequirement] = &[
    FlowRequirement {
        id: "sql_tables",
        role: FlowRole::StateOrStorage,
        query_seeds: &["sql table definitions", "CREATE TABLE"],
        coverage_mode: CoverageMode::AllowsLexicalSource,
        evidence: EvidencePredicate::CitedRoles {
            subsystem: flow_belongs_to_sql_schema,
            roles: &[PacketEvidenceRole::SqlTableDefinition],
        },
    },
    FlowRequirement {
        id: "sql_relationships",
        role: FlowRole::Configuration,
        query_seeds: &["foreign key relationships", "schema constraints"],
        coverage_mode: CoverageMode::AllowsLexicalSource,
        evidence: EvidencePredicate::CitedRoles {
            subsystem: flow_belongs_to_sql_schema,
            roles: &[PacketEvidenceRole::SqlRelationshipConstraint],
        },
    },
];

const HTML_CSS_FLOW: &[FlowRequirement] = &[
    FlowRequirement {
        id: "html_app_shell",
        role: FlowRole::Entrypoint,
        query_seeds: &["html app shell", "module script entry"],
        coverage_mode: CoverageMode::AllowsLexicalSource,
        evidence: EvidencePredicate::CitedCarrier(citation_owns_html_app_shell),
    },
    FlowRequirement {
        id: "css_structure",
        role: FlowRole::Configuration,
        query_seeds: &[
            "css theme defaults",
            "css layout selectors",
            "interactive element styles",
        ],
        coverage_mode: CoverageMode::AllowsLexicalSource,
        evidence: EvidencePredicate::CitedCarrier(citation_owns_css_structure),
    },
];

const CSS_ANIMATION_FLOW: &[FlowRequirement] = &[
    FlowRequirement {
        id: "css_animation_entrypoint",
        role: FlowRole::Entrypoint,
        query_seeds: &["animation stylesheet entrypoint", "css animation imports"],
        coverage_mode: CoverageMode::AllowsLexicalSource,
        evidence: EvidencePredicate::CitedCarrier(citation_owns_css_animation_entrypoint),
    },
    FlowRequirement {
        id: "css_animation_structure",
        role: FlowRole::Configuration,
        query_seeds: &[
            "css animation variables",
            "css animation base class",
            "css animation keyframes",
        ],
        coverage_mode: CoverageMode::AllowsLexicalSource,
        evidence: EvidencePredicate::CitedCarrier(citation_owns_css_animation_structure),
    },
];

const FORM_VALIDATION_FLOW: &[FlowRequirement] = &[
    FlowRequirement {
        id: "form_native_constraints",
        role: FlowRole::TransformOrValidate,
        query_seeds: &[
            "native form constraints",
            "constraint validation",
            "validity state",
        ],
        coverage_mode: CoverageMode::AllowsLexicalSource,
        evidence: EvidencePredicate::CitedCarrier(citation_owns_form_native_constraint),
    },
    FlowRequirement {
        id: "form_custom_validation",
        role: FlowRole::TransformOrValidate,
        query_seeds: &["custom validation", "custom error rendering"],
        coverage_mode: CoverageMode::AllowsLexicalSource,
        evidence: EvidencePredicate::CitedCarrier(citation_owns_form_custom_validation),
    },
    FlowRequirement {
        id: "form_submit_guard",
        role: FlowRole::TerminalBoundary,
        query_seeds: &["submit prevent default", "submit invalid guard"],
        coverage_mode: CoverageMode::AllowsLexicalSource,
        evidence: EvidencePredicate::CitedCarrier(citation_owns_form_submit_guard),
    },
];

const SHELL_INSTALL_FLOW: &[FlowRequirement] = &[
    FlowRequirement {
        id: "shell_installer_bootstrap",
        role: FlowRole::Entrypoint,
        query_seeds: &["shell installer bootstrap", "install download helpers"],
        coverage_mode: CoverageMode::AllowsLexicalSource,
        evidence: EvidencePredicate::CitedCarrier(citation_owns_shell_installer_bootstrap),
    },
    FlowRequirement {
        id: "shell_function_dispatch",
        role: FlowRole::Dispatch,
        query_seeds: &["shell function dispatch", "conditional version use"],
        coverage_mode: CoverageMode::AllowsLexicalSource,
        evidence: EvidencePredicate::CitedCarrier(citation_owns_shell_function_dispatch),
    },
    FlowRequirement {
        id: "shell_completion",
        role: FlowRole::TerminalBoundary,
        query_seeds: &["shell completion"],
        coverage_mode: CoverageMode::DiagnosticOnly,
        evidence: EvidencePredicate::CitedCarrier(citation_owns_shell_completion),
    },
];

const BUFFERED_IO_FLOW: &[FlowRequirement] = &[
    FlowRequirement {
        id: "buffered_storage",
        role: FlowRole::StateOrStorage,
        query_seeds: &["buffer storage", "source sink buffer"],
        coverage_mode: CoverageMode::AllowsSourceRange,
        evidence: EvidencePredicate::CitedCarrier(citation_owns_buffer_storage),
    },
    FlowRequirement {
        id: "buffered_read_write",
        role: FlowRole::Dispatch,
        query_seeds: &["source read buffer", "sink write buffer"],
        coverage_mode: CoverageMode::RequiresResolvedSourceOrGraph,
        evidence: EvidencePredicate::CitedCarrier(citation_owns_buffer_read_write),
    },
];

const LOG_HANDLER_FLOW: &[FlowRequirement] = &[
    FlowRequirement {
        id: "logger_event",
        role: FlowRole::Entrypoint,
        query_seeds: &["logger record", "record creation"],
        coverage_mode: CoverageMode::RequiresResolvedSourceOrGraph,
        evidence: EvidencePredicate::CitedCarrier(citation_owns_log_record_creation),
    },
    FlowRequirement {
        id: "handler_processing",
        role: FlowRole::Dispatch,
        query_seeds: &[
            "handler registration",
            "handler processing",
            "handler interface",
        ],
        coverage_mode: CoverageMode::RequiresResolvedSourceOrGraph,
        evidence: EvidencePredicate::CitedCarrier(citation_owns_log_handler_processing),
    },
];

const SITE_BUILD_FLOW: &[FlowRequirement] = &[
    FlowRequirement {
        id: "site_lifecycle",
        role: FlowRole::Entrypoint,
        query_seeds: &["site build lifecycle", "site process phases"],
        coverage_mode: CoverageMode::RequiresResolvedSourceOrGraph,
        evidence: EvidencePredicate::CitedCarrier(citation_owns_site_lifecycle),
    },
    FlowRequirement {
        id: "site_terminal",
        role: FlowRole::TerminalBoundary,
        query_seeds: &["read generate render write", "renderer render"],
        coverage_mode: CoverageMode::AllowsSourceRange,
        evidence: EvidencePredicate::CitedCarrier(citation_owns_site_terminal),
    },
];

const MAPPER_PLAN_FLOW: &[FlowRequirement] = &[
    FlowRequirement {
        id: "mapper_config",
        role: FlowRole::Configuration,
        query_seeds: &[
            "mapper runtime api",
            "mapper configuration",
            "type map plan",
        ],
        coverage_mode: CoverageMode::RequiresResolvedSourceOrGraph,
        evidence: EvidencePredicate::CitedCarrier(citation_owns_mapper_configuration),
    },
    FlowRequirement {
        id: "mapper_execution",
        role: FlowRole::Dispatch,
        query_seeds: &["mapping execution plan", "source destination mapping"],
        coverage_mode: CoverageMode::RequiresResolvedSourceOrGraph,
        evidence: EvidencePredicate::CitedCarrier(citation_owns_mapper_execution),
    },
];

const RUNTIME_FORMATTING_FLOW: &[FlowRequirement] = &[
    FlowRequirement {
        id: "format_arguments",
        role: FlowRole::TransformOrValidate,
        query_seeds: &["format arguments", "format output"],
        coverage_mode: CoverageMode::RequiresResolvedSourceOrGraph,
        evidence: EvidencePredicate::CitedCarrier(citation_owns_format_arguments),
    },
    FlowRequirement {
        id: "format_errors",
        role: FlowRole::ErrorOrFallback,
        query_seeds: &["format error", "error formatting"],
        coverage_mode: CoverageMode::AllowsSourceRange,
        evidence: EvidencePredicate::CitedCarrier(citation_owns_formatter_fallback),
    },
];

const SEARCH_EXECUTION_FLOW: &[FlowRequirement] = &[
    FlowRequirement {
        id: "search_entrypoint",
        role: FlowRole::Entrypoint,
        query_seeds: &["search entrypoint", "argument planning"],
        coverage_mode: CoverageMode::RequiresResolvedSourceOrGraph,
        evidence: EvidencePredicate::CitedRoles {
            subsystem: flow_belongs_to_search,
            roles: &[
                PacketEvidenceRole::SearchDriver,
                PacketEvidenceRole::ArgumentPlanning,
                PacketEvidenceRole::CommandEntrypoint,
            ],
        },
    },
    FlowRequirement {
        id: "search_dispatch",
        role: FlowRole::Dispatch,
        query_seeds: &[
            "search execution",
            "parallel search",
            "search execution unit",
        ],
        coverage_mode: CoverageMode::RequiresResolvedSourceOrGraph,
        evidence: EvidencePredicate::CitedRoles {
            subsystem: flow_belongs_to_search,
            roles: &[
                PacketEvidenceRole::SearchExecutionUnit,
                PacketEvidenceRole::CandidateFileConstruction,
            ],
        },
    },
];

/// Every requirement table, grouped the way a single question raises them. Requirements that share
/// a group and a `FlowRole` are the ones that must stay separable by evidence, so tests need the
/// grouping and not just a flat list.
#[cfg(test)]
pub(crate) fn all_flow_requirement_groups() -> Vec<(&'static str, Vec<FlowRequirement>)> {
    let mut client_dispatch = CLIENT_REQUEST_DISPATCH_FLOW.to_vec();
    client_dispatch.push(REQUEST_INTERCEPTOR_REQUIREMENT);
    vec![
        ("indexing", INDEXING_FLOW.to_vec()),
        (
            "server_request_dispatch",
            SERVER_REQUEST_DISPATCH_FLOW.to_vec(),
        ),
        ("client_request_dispatch", client_dispatch),
        ("url_session", URL_SESSION_FLOW.to_vec()),
        (
            "client_send",
            vec![
                CLIENT_PUBLIC_FACADE_REQUIREMENT,
                CLIENT_INTERFACE_HELPERS_REQUIREMENT,
                CLIENT_REQUEST_FINALIZATION_REQUIREMENT,
                CLIENT_TRANSPORT_SEND_REQUIREMENT,
                CLIENT_RESPONSE_MATERIALIZATION_REQUIREMENT,
            ],
        ),
        (
            "hook_cache",
            vec![
                HOOK_PUBLIC_EXPORT_REQUIREMENT,
                HOOK_KEY_SERIALIZATION_REQUIREMENT,
                HOOK_CACHE_HELPER_REQUIREMENT,
                HOOK_MUTATION_FLOW_REQUIREMENT,
            ],
        ),
        (
            "command_loop",
            vec![
                COMMAND_SERVER_BOOTSTRAP_REQUIREMENT,
                COMMAND_EVENT_LOOP_REQUIREMENT,
                COMMAND_NETWORK_INPUT_REQUIREMENT,
                COMMAND_DISPATCH_REQUIREMENT,
            ],
        ),
        ("sql_schema", SQL_SCHEMA_FLOW.to_vec()),
        ("html_css", HTML_CSS_FLOW.to_vec()),
        ("css_animation", CSS_ANIMATION_FLOW.to_vec()),
        ("form_validation", FORM_VALIDATION_FLOW.to_vec()),
        ("shell_install", SHELL_INSTALL_FLOW.to_vec()),
        ("buffered_io", BUFFERED_IO_FLOW.to_vec()),
        ("log_handler", LOG_HANDLER_FLOW.to_vec()),
        ("site_build", SITE_BUILD_FLOW.to_vec()),
        ("mapper_plan", MAPPER_PLAN_FLOW.to_vec()),
        ("runtime_formatting", RUNTIME_FORMATTING_FLOW.to_vec()),
        ("search_execution", SEARCH_EXECUTION_FLOW.to_vec()),
    ]
}

#[cfg(test)]
pub(crate) fn all_flow_requirements() -> Vec<FlowRequirement> {
    all_flow_requirement_groups()
        .into_iter()
        .flat_map(|(_, requirements)| requirements)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::packet_terms::packet_probe_terms;
    use codestory_contracts::api::{NodeId, NodeKind, SearchHitOrigin};
    use std::collections::BTreeMap;

    fn client_requirement_ids(prompt: &str) -> Vec<&'static str> {
        packet_flow_requirements_for_terms(
            &packet_probe_terms(prompt),
            PacketTaskClassDto::DataFlow,
        )
        .into_iter()
        .filter_map(|requirement| {
            requirement
                .id
                .starts_with("client_")
                .then_some(requirement.id)
        })
        .collect()
    }

    #[test]
    fn broad_client_send_prompt_requires_full_lifecycle() {
        assert_eq!(
            client_requirement_ids(
                "Explain how an HTTP client exposes top-level helpers, provides client convenience methods, finalizes requests before transport send, and materializes responses."
            ),
            vec![
                "client_public_facade",
                "client_interface_helpers",
                "client_request_finalization",
                "client_transport_send",
                "client_response_materialization",
            ]
        );
    }

    #[test]
    fn focused_client_finalization_prompt_does_not_require_full_lifecycle() {
        assert_eq!(
            client_requirement_ids(
                "Explain how an HTTP client finalizes requests before transport."
            ),
            vec!["client_request_finalization"]
        );
    }

    #[test]
    fn focused_client_transport_prompt_does_not_require_full_lifecycle() {
        assert_eq!(
            client_requirement_ids("Explain how an HTTP client performs transport send."),
            vec!["client_transport_send"]
        );
    }

    #[test]
    fn client_request_flow_uses_behavior_owner_probes_without_server_registration() {
        let requirements = packet_flow_requirements_for_terms(
            &packet_probe_terms(
                "Explain how a default HTTP client instance is created, then a request passes through request and response interceptors before dispatch to the adapter and transport.",
            ),
            PacketTaskClassDto::ArchitectureExplanation,
        );
        let entrypoint = requirements
            .iter()
            .find(|requirement| requirement.id == "request_entrypoint")
            .expect("client request flow should require an entrypoint");
        let queries = requirements
            .iter()
            .flat_map(|requirement| requirement.query_seeds.iter().copied())
            .collect::<Vec<_>>();

        assert_eq!(entrypoint.role, FlowRole::Entrypoint);
        for expected in [
            "default instance",
            "request method",
            "interceptor handlers",
            "adapters",
        ] {
            assert!(
                queries.contains(&expected),
                "client request flow should probe {expected}"
            );
        }
        for server_only in ["route registration", "handler dispatch"] {
            assert!(
                !queries.contains(&server_only),
                "client request flow should not probe {server_only}"
            );
        }
    }

    #[test]
    fn server_request_flow_retains_route_registration_and_handler_dispatch() {
        let requirements = packet_flow_requirements_for_terms(
            &packet_probe_terms(
                "Trace how an HTTP server routes an incoming request through route registration, request handler dispatch, and response finalization.",
            ),
            PacketTaskClassDto::RouteTracing,
        );
        let entrypoint = requirements
            .iter()
            .find(|requirement| requirement.id == "request_entrypoint")
            .expect("server request flow should require an entrypoint");
        let queries = requirements
            .iter()
            .flat_map(|requirement| requirement.query_seeds.iter().copied())
            .collect::<Vec<_>>();

        assert_eq!(entrypoint.role, FlowRole::Registration);
        for expected in ["route registration", "handler dispatch"] {
            assert!(
                queries.contains(&expected),
                "server request flow should probe {expected}"
            );
        }
        for client_only in [
            "default instance",
            "request method",
            "interceptor handlers",
            "adapters",
        ] {
            assert!(
                !queries.contains(&client_only),
                "server request flow should not probe {client_only}"
            );
        }
    }

    /// The checked-in inventory of every requirement a question can raise, as
    /// `id | role | coverage mode`.
    ///
    /// This exists so that a requirement can never be quietly removed to make a gate pass. The
    /// previous invariant asked "can any evidence role carry this requirement's `FlowRole`?", which
    /// a failing lane could satisfy by deleting the requirement; this one fails on removal too, and
    /// the only way past it is to edit the list in the diff a reviewer reads.
    const FLOW_REQUIREMENT_INVENTORY: &[&str] = &[
        "buffered_read_write | dispatch | RequiresResolvedSourceOrGraph",
        "buffered_storage | state_or_storage | AllowsSourceRange",
        "client_interface_helpers | entrypoint | RequiresResolvedSourceOrGraph",
        "client_public_facade | entrypoint | RequiresResolvedSourceOrGraph",
        "client_request_finalization | transform_or_validate | RequiresResolvedSourceOrGraph",
        "client_response_materialization | terminal_boundary | RequiresResolvedSourceOrGraph",
        "client_transport_send | dispatch | RequiresResolvedSourceOrGraph",
        "command_dispatch | dispatch | RequiresResolvedSourceOrGraph",
        "command_event_loop | dispatch | RequiresResolvedSourceOrGraph",
        "command_network_input | dispatch | RequiresResolvedSourceOrGraph",
        "command_server_bootstrap | entrypoint | RequiresResolvedSourceOrGraph",
        "css_animation_entrypoint | entrypoint | AllowsLexicalSource",
        "css_animation_structure | configuration | AllowsLexicalSource",
        "css_structure | configuration | AllowsLexicalSource",
        "form_custom_validation | transform_or_validate | AllowsLexicalSource",
        "form_native_constraints | transform_or_validate | AllowsLexicalSource",
        "form_submit_guard | terminal_boundary | AllowsLexicalSource",
        "format_arguments | transform_or_validate | RequiresResolvedSourceOrGraph",
        "format_errors | error_or_fallback | AllowsSourceRange",
        "handler_processing | dispatch | RequiresResolvedSourceOrGraph",
        "hook_cache_helper | state_or_storage | AllowsSourceRange",
        "hook_key_serialization | transform_or_validate | AllowsSourceRange",
        "hook_mutation_flow | dispatch | AllowsSourceRange",
        "hook_public_export | entrypoint | AllowsSourceRange",
        "html_app_shell | entrypoint | AllowsLexicalSource",
        "indexing_entrypoint | entrypoint | RequiresResolvedSourceOrGraph",
        "indexing_storage | state_or_storage | AllowsSourceRange",
        "logger_event | entrypoint | RequiresResolvedSourceOrGraph",
        "mapper_config | configuration | RequiresResolvedSourceOrGraph",
        "mapper_execution | dispatch | RequiresResolvedSourceOrGraph",
        "request_dispatch | dispatch | RequiresResolvedSourceOrGraph",
        "request_entrypoint | entrypoint | RequiresResolvedSourceOrGraph",
        "request_entrypoint | registration | RequiresResolvedSourceOrGraph",
        "request_interceptor_management | dispatch | RequiresResolvedSourceOrGraph",
        "request_terminal | terminal_boundary | AllowsSourceRange",
        "search_dispatch | dispatch | RequiresResolvedSourceOrGraph",
        "search_entrypoint | entrypoint | RequiresResolvedSourceOrGraph",
        "session_callbacks | dispatch | AllowsSourceRange",
        "session_request | entrypoint | RequiresResolvedSourceOrGraph",
        "shell_completion | terminal_boundary | DiagnosticOnly",
        "shell_function_dispatch | dispatch | AllowsLexicalSource",
        "shell_installer_bootstrap | entrypoint | AllowsLexicalSource",
        "site_lifecycle | entrypoint | RequiresResolvedSourceOrGraph",
        "site_terminal | terminal_boundary | AllowsSourceRange",
        "sql_relationships | configuration | AllowsLexicalSource",
        "sql_tables | state_or_storage | AllowsLexicalSource",
    ];

    fn requirement_inventory_entry(requirement: &FlowRequirement) -> String {
        format!(
            "{} | {} | {:?}",
            requirement.id,
            requirement.role_id(),
            requirement.coverage_mode
        )
    }

    #[test]
    fn the_requirement_inventory_matches_the_requirement_tables() {
        let mut live = all_flow_requirements()
            .iter()
            .map(requirement_inventory_entry)
            .collect::<Vec<_>>();
        live.sort();
        live.dedup();

        let mut recorded = FLOW_REQUIREMENT_INVENTORY
            .iter()
            .map(|entry| (*entry).to_string())
            .collect::<Vec<_>>();
        recorded.sort();

        let removed = recorded
            .iter()
            .filter(|entry| !live.contains(entry))
            .collect::<Vec<_>>();
        assert!(
            removed.is_empty(),
            "a requirement disappeared from the tables; a requirement no evidence can reach is a \
             retrieval gap to close, not a requirement to drop: {removed:?}"
        );
        let added = live
            .iter()
            .filter(|entry| !recorded.contains(entry))
            .collect::<Vec<_>>();
        assert!(
            added.is_empty(),
            "a new requirement is not in the checked-in inventory; add it there so removals stay \
             visible in review: {added:?}"
        );
    }

    /// One cited anchor that proves each requirement. Two jobs: it shows every requirement is
    /// reachable at all (a requirement no evidence can close would report partial forever), and it
    /// gives the same-role distinctness test the witnesses it needs.
    fn requirement_witnesses() -> Vec<((&'static str, &'static str), AgentCitationDto)> {
        vec![
            (
                ("indexing_entrypoint", "entrypoint"),
                witness("buildIndex", "src/indexer/build.rs", NodeKind::FUNCTION),
            ),
            (
                ("indexing_storage", "state_or_storage"),
                witness(
                    "SymbolStore.persist",
                    "src/store/symbols.rs",
                    NodeKind::METHOD,
                ),
            ),
            (
                ("request_entrypoint", "registration"),
                witness("Router.add_route", "src/routing.py", NodeKind::FUNCTION),
            ),
            (
                ("request_entrypoint", "entrypoint"),
                witness("createInstance", "lib/axios.js", NodeKind::FUNCTION),
            ),
            (
                ("request_dispatch", "dispatch"),
                witness(
                    "dispatchRequest",
                    "lib/core/dispatchRequest.js",
                    NodeKind::FUNCTION,
                ),
            ),
            (
                ("request_terminal", "terminal_boundary"),
                witness(
                    "selectAdapter",
                    "lib/adapters/adapters.js",
                    NodeKind::FUNCTION,
                ),
            ),
            (
                ("request_interceptor_management", "dispatch"),
                witness(
                    "InterceptorManager",
                    "lib/core/InterceptorManager.js",
                    NodeKind::CLASS,
                ),
            ),
            (
                ("session_request", "entrypoint"),
                witness(
                    "createClientInstance",
                    "Source/Session.swift",
                    NodeKind::FUNCTION,
                ),
            ),
            (
                ("session_callbacks", "dispatch"),
                witness(
                    "SessionDelegate.dispatchEvent",
                    "Source/SessionDelegate.swift",
                    NodeKind::METHOD,
                ),
            ),
            (
                ("client_public_facade", "entrypoint"),
                witness("createClient", "lib/client.dart", NodeKind::FUNCTION),
            ),
            (
                ("client_interface_helpers", "entrypoint"),
                witness("Client.get", "lib/client.dart", NodeKind::METHOD),
            ),
            (
                ("client_request_finalization", "transform_or_validate"),
                witness(
                    "BaseRequest.finalize",
                    "lib/base_request.dart",
                    NodeKind::METHOD,
                ),
            ),
            (
                ("client_transport_send", "dispatch"),
                witness(
                    "IOClient.sendAdapter",
                    "lib/io_client.dart",
                    NodeKind::METHOD,
                ),
            ),
            (
                ("client_response_materialization", "terminal_boundary"),
                witness("Response.fromStream", "lib/response.dart", NodeKind::METHOD),
            ),
            (
                ("hook_public_export", "entrypoint"),
                witness("useData", "src/index/use-data.ts", NodeKind::FUNCTION),
            ),
            (
                ("hook_key_serialization", "transform_or_validate"),
                witness(
                    "serializeKey",
                    "src/_internal/utils/serialize.ts",
                    NodeKind::FUNCTION,
                ),
            ),
            (
                ("hook_cache_helper", "state_or_storage"),
                witness(
                    "makeCacheHelper",
                    "src/_internal/utils/helper.ts",
                    NodeKind::FUNCTION,
                ),
            ),
            (
                ("hook_mutation_flow", "dispatch"),
                witness(
                    "applyMutation",
                    "src/_internal/utils/mutate.ts",
                    NodeKind::FUNCTION,
                ),
            ),
            (
                ("command_server_bootstrap", "entrypoint"),
                witness("main", "src/server.c", NodeKind::FUNCTION),
            ),
            (
                ("command_event_loop", "dispatch"),
                witness("aeProcessEvents", "src/event/ae.c", NodeKind::FUNCTION),
            ),
            (
                ("command_network_input", "dispatch"),
                witness(
                    "readQueryFromClient",
                    "src/networking.c",
                    NodeKind::FUNCTION,
                ),
            ),
            (
                ("command_dispatch", "dispatch"),
                witness("processCommand", "src/server.c", NodeKind::FUNCTION),
            ),
            (
                ("sql_tables", "state_or_storage"),
                witness("CREATE TABLE Artist", "db/schema.sql", NodeKind::FUNCTION),
            ),
            (
                ("sql_relationships", "configuration"),
                witness("FOREIGN KEY", "db/schema.sql", NodeKind::FUNCTION),
            ),
            (
                ("html_app_shell", "entrypoint"),
                witness("div#app", "src/index.html", NodeKind::FUNCTION),
            ),
            (
                ("css_structure", "configuration"),
                witness(":root", "src/main.css", NodeKind::FUNCTION),
            ),
            (
                ("css_animation_entrypoint", "entrypoint"),
                witness(
                    "@import \"animations/base\"",
                    "src/animations/index.css",
                    NodeKind::FUNCTION,
                ),
            ),
            (
                ("css_animation_structure", "configuration"),
                witness(
                    "@keyframes fade-in",
                    "src/animations/fade.css",
                    NodeKind::FUNCTION,
                ),
            ),
            (
                ("form_native_constraints", "transform_or_validate"),
                witness("required", "examples/form.html", NodeKind::FUNCTION),
            ),
            (
                ("form_custom_validation", "transform_or_validate"),
                witness(
                    "setCustomValidity",
                    "examples/validate.js",
                    NodeKind::FUNCTION,
                ),
            ),
            (
                ("form_submit_guard", "terminal_boundary"),
                witness("onSubmitGuard", "examples/submit.js", NodeKind::FUNCTION),
            ),
            (
                ("shell_installer_bootstrap", "entrypoint"),
                witness("nvm_download", "install.sh", NodeKind::FUNCTION),
            ),
            (
                ("shell_function_dispatch", "dispatch"),
                witness("nvm_command", "nvm.sh", NodeKind::FUNCTION),
            ),
            (
                ("shell_completion", "terminal_boundary"),
                witness("nvm_completion", "bash_completion.sh", NodeKind::FUNCTION),
            ),
            (
                ("buffered_storage", "state_or_storage"),
                witness("Buffer", "okio/src/buffer.kt", NodeKind::CLASS),
            ),
            (
                ("buffered_read_write", "dispatch"),
                witness("Buffer.writeUtf8", "okio/src/buffer.kt", NodeKind::METHOD),
            ),
            (
                ("logger_event", "entrypoint"),
                witness(
                    "Logger.addRecord",
                    "src/logging/Logger.php",
                    NodeKind::METHOD,
                ),
            ),
            (
                ("handler_processing", "dispatch"),
                witness(
                    "AbstractProcessingHandler.write",
                    "src/logging/Handler.php",
                    NodeKind::METHOD,
                ),
            ),
            // The site object's own lifecycle and output methods. `Build.process` and
            // `Renderer.render` stood here before, and both took their subsystem entirely from the
            // `lib/site/` folder they were filed in — which is what made a directory look
            // load-bearing for this flow. A site generator's build phases hang off the site, and
            // that is what the two anchors say now; the folder is left in place so the corpus keeps
            // filing off-subject symbols beside them.
            (
                ("site_lifecycle", "entrypoint"),
                witness("Site.process", "lib/site/site.rb", NodeKind::METHOD),
            ),
            (
                ("site_terminal", "terminal_boundary"),
                witness("Site.write", "lib/site/renderer.rb", NodeKind::METHOD),
            ),
            (
                ("mapper_config", "configuration"),
                witness(
                    "MapperConfiguration",
                    "src/AutoMapper/MapperConfiguration.cs",
                    NodeKind::CLASS,
                ),
            ),
            (
                ("mapper_execution", "dispatch"),
                witness(
                    "TypeMapPlanBuilder",
                    "src/AutoMapper/Execution/Plan.cs",
                    NodeKind::CLASS,
                ),
            ),
            (
                ("format_arguments", "transform_or_validate"),
                witness("basic_format_args", "include/fmt/base.h", NodeKind::CLASS),
            ),
            (
                ("format_errors", "error_or_fallback"),
                witness(
                    "throw_format_error",
                    "include/fmt/format.h",
                    NodeKind::FUNCTION,
                ),
            ),
            (
                ("search_entrypoint", "entrypoint"),
                witness("main", "crates/core/main.rs", NodeKind::FUNCTION),
            ),
            (
                ("search_dispatch", "dispatch"),
                witness("SearchWorker", "crates/core/search.rs", NodeKind::STRUCT),
            ),
        ]
    }

    fn witness(display_name: &str, file_path: &str, kind: NodeKind) -> AgentCitationDto {
        AgentCitationDto {
            node_id: NodeId(display_name.to_string()),
            display_name: display_name.to_string(),
            kind,
            file_path: Some(file_path.to_string()),
            line: Some(1),
            score: 1.0,
            origin: SearchHitOrigin::IndexedSymbol,
            resolvable: true,
            subgraph_id: None,
            evidence_edge_ids: Vec::new(),
            retrieval_score_breakdown: None,
            evidence_tier: None,
            evidence_producer: None,
            resolution_status: None,
            loss_reason: None,
            coverage_role: None,
            eligible_for_sufficiency: Some(true),
        }
    }

    #[test]
    fn every_requirement_has_evidence_that_can_close_it() {
        let witnesses = requirement_witnesses();
        for requirement in all_flow_requirements() {
            let key = (requirement.id, requirement.role_id());
            let witness = witnesses
                .iter()
                .find(|(witness_key, _)| *witness_key == key)
                .map(|(_, citation)| citation)
                .unwrap_or_else(|| {
                    panic!(
                        "requirement {} has no witness; every requirement needs evidence that can \
                         close it, or it reports partial forever",
                        requirement.id
                    )
                });
            assert!(
                requirement.evidence.citation_proves(witness),
                "requirement {} is unclosable: its witness `{}` does not satisfy its evidence \
                 predicate",
                requirement.id,
                witness.display_name
            );
        }
    }

    #[test]
    fn requirements_sharing_a_flow_role_stay_separable_by_evidence() {
        let witnesses = requirement_witnesses();
        let witness_for = |requirement: &FlowRequirement| {
            let key = (requirement.id, requirement.role_id());
            witnesses
                .iter()
                .find(|(witness_key, _)| *witness_key == key)
                .map(|(_, citation)| citation.clone())
                .unwrap_or_else(|| panic!("missing witness for {key:?}"))
        };

        let mut checked_pairs = 0;
        for (group, requirements) in all_flow_requirement_groups() {
            for (index, left) in requirements.iter().enumerate() {
                for right in requirements.iter().skip(index + 1) {
                    if left.role != right.role || left.id == right.id {
                        continue;
                    }
                    checked_pairs += 1;
                    let left_witness = witness_for(left);
                    let right_witness = witness_for(right);
                    assert!(
                        !right.evidence.citation_proves(&left_witness),
                        "in flow {group}, evidence for {} also closes its {} sibling {}: two \
                         requirements sharing a role must not be closed by one anchor",
                        left.id,
                        left.role.label(),
                        right.id
                    );
                    assert!(
                        !left.evidence.citation_proves(&right_witness),
                        "in flow {group}, evidence for {} also closes its {} sibling {}: two \
                         requirements sharing a role must not be closed by one anchor",
                        right.id,
                        right.role.label(),
                        left.id
                    );
                }
            }
        }
        assert!(
            checked_pairs >= 5,
            "the tables still contain same-role sibling requirements; this invariant must actually \
             be exercising them (checked {checked_pairs})"
        );
    }

    /// Symbols of the kind retrieval turns up in any repository, none of which prove anything about
    /// any flow requirement in the tables.
    ///
    /// The positive witnesses above only show each predicate accepts *one* hand-picked anchor. They
    /// cannot see a predicate that also accepts everything else, and that is exactly what happened:
    /// `citation_owns_formatter_fallback` matched any symbol whose name contained "error" anywhere in
    /// the repository, `citation_owns_hook_public_export` matched any name starting with the three
    /// letters "use", and `citation_owns_form_native_constraint` matched the unanchored substring
    /// "min" — so `CliParseError`, `userProfile` and `adminPanel` each closed a requirement they
    /// have nothing to do with, and packets carrying them published as sufficient.
    ///
    /// Every entry must be rejected by every requirement. Adding a needle to a carrier without
    /// checking it here is how the next false-safe verdict gets in.
    fn unrelated_repository_symbols() -> Vec<AgentCitationDto> {
        vec![
            witness("CliParseError", "src/cli/parse.cc", NodeKind::FUNCTION),
            witness("assert_valid_utf8", "src/text/utf8.rs", NodeKind::FUNCTION),
            witness("panic_hook", "src/runtime/panic.rs", NodeKind::FUNCTION),
            witness("failToOpenSocket", "src/net/socket.go", NodeKind::FUNCTION),
            witness("userProfile", "src/session/user.ts", NodeKind::FUNCTION),
            witness("useragentString", "src/http/headers.ts", NodeKind::FUNCTION),
            witness("determineFieldOrder", "src/layout.js", NodeKind::FUNCTION),
            witness("adminPanel", "src/admin.js", NodeKind::FUNCTION),
            witness("terminalWidth", "src/tty.js", NodeKind::FUNCTION),
            witness("submitTelemetry", "src/telemetry.js", NodeKind::FUNCTION),
            witness("Cache.write", "lib/cache.rb", NodeKind::METHOD),
            witness("Uri.prepare", "lib/uri.dart", NodeKind::METHOD),
            witness("ProjectSettings", "src/settings.rs", NodeKind::STRUCT),
            witness("parseTimestamp", "src/time/parse.ts", NodeKind::FUNCTION),
            witness("RowIterator", "src/db/rows.rs", NodeKind::STRUCT),
            witness("MigrationRunner", "src/db/migrate.rb", NodeKind::CLASS),
            // Each of these closed a requirement at exactly this path. The first six are role
            // classified, where the *directory* assigned the role: `/views/` and `/app/` mean route
            // handling, `store` means persistence, `/flags/` means argument planning. The last four
            // sit inside the very flow they were accepted by, which is the case a corpus of
            // symbols from elsewhere in the repository can never reach.
            witness("Store.delete", "src/store/store.rs", NodeKind::METHOD),
            witness(
                "serializeSettings",
                "src/store/serialize.ts",
                NodeKind::FUNCTION,
            ),
            witness("readManifest", "src/store/manifest.rs", NodeKind::FUNCTION),
            witness("renderChart", "src/views/chart.js", NodeKind::FUNCTION),
            witness("Cache.write", "app/views/cache.rb", NodeKind::METHOD),
            witness(
                "FeatureFlags.options",
                "src/flags/feature.rs",
                NodeKind::METHOD,
            ),
            witness("handleClick", "src/logging/ui.php", NodeKind::FUNCTION),
            witness(
                "createUserRecord",
                "src/logging/audit.php",
                NodeKind::FUNCTION,
            ),
            witness("use_temp_dir", "src/index/tmp.ts", NodeKind::FUNCTION),
            witness("Store.get", "lib/client.dart", NodeKind::METHOD),
            // Each of these closed a requirement one level below the last round's fix. The first
            // four are role-classified and the *file name* assigned the role — `runtime.c`,
            // `signal_dispatch.rs`, `store.ts` — after the directories had already been stripped.
            // The rest are carrier-backed, and each is a compound noun whose head is the flow's own
            // subject word: a form's `min`, a logger's `handler`, a site's `layout`, a build's
            // `post`, a buffer.
            witness("tooltipHandler", "src/os/runtime.c", NodeKind::FUNCTION),
            witness(
                "panicHandler",
                "src/os/signal_dispatch.rs",
                NodeKind::FUNCTION,
            ),
            witness(
                "workspaceSettings",
                "src/config/store.ts",
                NodeKind::FUNCTION,
            ),
            witness(
                "MathSymbolTable",
                "src/math/table_dispatch.rs",
                NodeKind::STRUCT,
            ),
            witness("clampMin", "src/forms/layout.ts", NodeKind::FUNCTION),
            witness(
                "PaymentHandler.process",
                "src/logging/payments.php",
                NodeKind::METHOD,
            ),
            witness(
                "Layout.render",
                "src/components/layout.tsx",
                NodeKind::METHOD,
            ),
            witness("readFile", "src/assets/io.ts", NodeKind::FUNCTION),
            witness(
                "PostMortem.generate",
                "src/crash/report.rb",
                NodeKind::METHOD,
            ),
            witness("FrameBuffer", "src/gfx/frame.cpp", NodeKind::STRUCT),
            witness("SegmentTree.read", "src/algo/segtree.rs", NodeKind::METHOD),
            witness(
                "sourceMapOptions",
                "src/build/config.ts",
                NodeKind::FUNCTION,
            ),
            witness("RoadMapPlanner", "src/nav/planner.rs", NodeKind::STRUCT),
            witness("dispatchRider", "src/delivery/rider.ts", NodeKind::FUNCTION),
            witness(
                "validationMinScore",
                "src/auth/password.ts",
                NodeKind::FUNCTION,
            ),
            witness("ChartAdapter", "src/charts/adapter.ts", NodeKind::CLASS),
        ]
    }

    /// Shapes of symbol name a repository is full of, none of which is evidence for any step in any
    /// flow in the tables.
    ///
    /// These are families, not examples, and each one is a way a predicate here has been fooled or
    /// could be. A **verb-named accessor** meets a carrier that matched the HTTP method set on a
    /// symbol's terminal segment, so every `.get`, `.delete` and `.options` in the repository was a
    /// client's request method. A **`handle*` callback** meets a carrier that matched "handle" as a
    /// prefix of "handler", so every front end's click and scroll handlers were a logging
    /// framework's record processing. A **`*Record` builder** meets a carrier that matched the word
    /// "record", so every database row constructor was a logger's record creation. A **snake- or
    /// kebab-cased `use_*`** meets a carrier that treated `_` and `-` as the front-end hook naming
    /// convention. The last family is ordinary vocabulary from subsystems no flow here covers.
    ///
    /// The property that makes a name a negative, and the bar a new entry has to clear, is that no
    /// requirement's *two* factors are both satisfied by it. Sharing one is allowed and is the point:
    /// `Cache.write` names a step word the site build reads, `Matrix.post` names one of its subjects,
    /// `Store.get` names an HTTP verb — and each must still be rejected, because none of them names
    /// both. A name that names both is not a negative; it is evidence.
    fn off_subject_symbol_names() -> Vec<(&'static str, NodeKind)> {
        let mut names = Vec::new();
        for name in [
            "Store.get",
            "Store.delete",
            "Cache.put",
            "FeatureFlags.options",
            "Queue.head",
            "Matrix.post",
            "Palette.patch",
        ] {
            names.push((name, NodeKind::METHOD));
        }
        for name in [
            "handleClick",
            "handleKeypress",
            "handleDragStart",
            "handleScroll",
            "handleResize",
        ] {
            names.push((name, NodeKind::FUNCTION));
        }
        for name in [
            "createUserRecord",
            "createDnsRecord",
            "addBillingRecord",
            "makeInventoryRecord",
        ] {
            names.push((name, NodeKind::FUNCTION));
        }
        for name in ["use_temp_dir", "use-legacy-mode", "use_default_locale"] {
            names.push((name, NodeKind::FUNCTION));
        }
        for name in [
            "compareVersions",
            "parseTimestamp",
            "TooltipAnchor",
            "ColorPalette",
            "computeChecksum",
            "encodeBase64",
            "serializeSettings",
            "readManifest",
            "renderChart",
            "MigrationRunner",
            "RowIterator",
            "ProjectSettings",
            "adminPanel",
            "terminalWidth",
            "determineFieldOrder",
            "userProfile",
            "submitTelemetry",
            "Uri.prepare",
            "Cache.write",
        ] {
            names.push((name, NodeKind::FUNCTION));
        }
        names
    }

    /// Every directory the corpus places an off-subject symbol in.
    ///
    /// The first half is derived from the witness table, so every flow's *own* folder is covered
    /// and stays covered as requirements are added — a symbol sitting beside a flow's real evidence
    /// is the case a corpus of symbols from elsewhere in the repository cannot reach, and path
    /// tokens are what re-open a scoped predicate. The second half is every path fragment the
    /// shared evidence-role classifier will assign a role from on its own, read out of
    /// `packet_evidence_roles`: those directories hand out a role to whatever is filed in them.
    fn off_subject_directories() -> Vec<String> {
        let mut directories = Vec::new();
        let mut push = |directory: String| {
            if !directories.contains(&directory) {
                directories.push(directory);
            }
        };
        for ((_, _), witness) in requirement_witnesses() {
            let path = witness.file_path.clone().unwrap_or_default();
            push(match path.rfind('/') {
                Some(index) => path[..index + 1].to_string(),
                None => String::new(),
            });
        }
        for directory in [
            "src/routes/",
            "src/router/",
            "src/controllers/",
            "src/views/",
            "src/pages/",
            "app/",
            "app/views/",
            "src/event/",
            "src/events/",
            "src/flags/",
            "src/protocol/",
            "src/networking/",
            "src/runtime/",
            "src/store/",
            "src/indexer/",
            "src/workspace/",
            "src/interceptors/",
            "src/dispatch/",
            "src/collections/",
            "src/source_group/",
            // The same directories a Windows citation arrives with. Two code paths disagree about
            // separators — the role classifier normalizes them, the carriers lowercase and replace
            // them, and stripping a directory has to split on both — so the corpus carries both.
            "app\\views\\",
            "src\\store\\",
            "src\\runtime\\",
        ] {
            push(directory.to_string());
        }
        directories
    }

    /// The extensions the corpus crosses its directories with.
    ///
    /// Derived from the witness paths, minus the document surfaces. A stylesheet, a markup
    /// document, a schema file and a shell script are proved *by the file*: their anchors are
    /// selectors, attributes and statements, not identifiers, and "a code identifier inside a
    /// `.css` file" is not a citation retrieval can produce. Those requirements are still exercised
    /// by this corpus — they have to reject every code path in it.
    fn off_subject_code_extensions() -> Vec<String> {
        let document_surfaces = [
            ".css", ".scss", ".sass", ".less", ".html", ".htm", ".sql", ".sh",
        ];
        let mut extensions = Vec::new();
        for ((_, _), witness) in requirement_witnesses() {
            let path = witness.file_path.clone().unwrap_or_default();
            let Some(index) = path.rfind('.') else {
                continue;
            };
            let extension = path[index..].to_ascii_lowercase();
            if document_surfaces.contains(&extension.as_str()) || extensions.contains(&extension) {
                continue;
            }
            extensions.push(extension);
        }
        extensions
    }

    /// The generated corpus: every off-subject name, in every flow's directory and every
    /// role-granting directory, under every code extension, as every kind of behavior owner.
    fn generated_off_subject_symbols() -> Vec<AgentCitationDto> {
        let mut symbols = Vec::new();
        for (name, kind) in off_subject_symbol_names() {
            for directory in off_subject_directories() {
                for extension in off_subject_code_extensions() {
                    for owner_kind in [
                        kind,
                        NodeKind::CLASS,
                        NodeKind::STRUCT,
                        NodeKind::INTERFACE,
                        NodeKind::CONSTANT,
                    ] {
                        symbols.push(witness(
                            name,
                            &format!("{directory}elsewhere{extension}"),
                            owner_kind,
                        ));
                    }
                }
            }
        }
        symbols
    }

    /// A bare `map` is not an object mapper, and the family that rides in on it is large.
    ///
    /// `MapPlanner` used to be documented here as an accepted limitation: `mapper_execution` asks
    /// for an object mapper and an execution plan, and both words were literally in the name. But
    /// the word carrying the subsystem was `map`, which is the head of `sourceMap`, `roadMap`,
    /// `siteMap`, `heatMap` and `tileMap` — so the limitation was not one name, it was every
    /// compound noun in software ending in "map", and `sourceMapOptions` (in every JavaScript build
    /// configuration there is) plus `RoadMapPlanner` closed the whole two-step flow between them.
    ///
    /// A bare `map` now has to say what it maps. `TypeMapPlanBuilder`, the real anchor, does.
    #[test]
    fn a_map_that_is_not_an_object_mapper_closes_nothing() {
        let requirement_named = |id: &str| {
            all_flow_requirements()
                .into_iter()
                .find(|requirement| requirement.id == id)
                .unwrap_or_else(|| panic!("{id} should be in the tables"))
        };

        for (display_name, kind) in [
            ("MapPlanner", NodeKind::STRUCT),
            ("RoadMapPlanner", NodeKind::STRUCT),
            ("SiteMapPlan", NodeKind::STRUCT),
            ("TileMapExecutor", NodeKind::STRUCT),
            ("sourceMapOptions", NodeKind::FUNCTION),
            ("HeatMapConfig", NodeKind::STRUCT),
            ("bitmapPipeline", NodeKind::FUNCTION),
        ] {
            for path in ["src/store/planner.rs", "src/mapping/plan.rs", "src/nav.ts"] {
                let anchor = witness(display_name, path, kind);
                for id in ["indexing_storage", "mapper_execution", "mapper_config"] {
                    assert!(
                        !requirement_named(id).evidence.citation_proves(&anchor),
                        "`{display_name}` at `{path}` is not {id}: the word carrying the subsystem \
                         is the head of a compound noun from another domain"
                    );
                }
            }
        }

        let real = witness(
            "TypeMapPlanBuilder",
            "src/AutoMapper/Execution/Plan.cs",
            NodeKind::CLASS,
        );
        assert!(
            requirement_named("mapper_execution")
                .evidence
                .citation_proves(&real),
            "a type map's plan builder is still the mapper's execution step"
        );
    }

    /// The complete set of *bare, one-word* symbol names that close a requirement, as
    /// `requirement | word`.
    ///
    /// A one-word name carries no second factor: there is no room in it for both "which subsystem
    /// is this" and "which step of it". So every entry here is a word that, on its own, anywhere in
    /// any repository, under any directory and any language, proves a step.
    ///
    /// This list is **not** the whole surface, and it used to claim to be. Every predicate in this
    /// crate matches whole tokens *inside* a name, so a word that closes a requirement bare closes
    /// it inside compounds too — `buffer` here meant `FrameBuffer` and `ZBuffer` as well, and the
    /// list said nothing about it. `COMPOUND_EVIDENCE_SURFACE` above is the family version and is
    /// the one to read for what an unrelated symbol can still be mistaken for; this one is the
    /// stricter subset, kept because a *bare* word closing a requirement is a sharper signal.
    ///
    /// Each of these words *is* the requirement's subject: a class named `Buffer` is the buffer, a
    /// function named `main` is the entrypoint, a method named `request` is the client's request
    /// method. That is the intended reading of a name-driven predicate. What must not happen is the
    /// list growing quietly: an entry appearing here means some carrier's two factors collapsed
    /// into one word, which is how `renderChart` proved a site renderer and every `.get` in the
    /// repository proved a client's convenience method.
    ///
    /// The stylesheet, markup and shell entries arrived when the sweep started crossing every
    /// surface class a carrier branches on rather than `.rs` and `.ts`. They are not a collapse:
    /// on those surfaces the anchor is a selector, an attribute or a shell function, which is the
    /// declared exception, and no code identifier can reach them because the extension gate will
    /// not have it. What the widening was for is the case that is *not* an exception — a `.vue`
    /// anchor was taking the markup branch while the `.ts` beside it took the name branch, and a
    /// sweep that only ever asked about `.ts` could not see the difference.
    const ONE_WORD_EVIDENCE_SURFACE: &[&str] = &[
        "buffered_storage | buffer",
        "client_interface_helpers | request",
        "command_server_bootstrap | main",
        "css_animation_entrypoint | forward",
        "css_animation_entrypoint | import",
        "css_animation_entrypoint | use",
        "css_animation_structure | animated",
        "css_animation_structure | animation",
        "css_animation_structure | delay",
        "css_animation_structure | duration",
        "css_animation_structure | fillmode",
        "css_animation_structure | iteration",
        "css_animation_structure | keyframes",
        "css_animation_structure | transition",
        "form_custom_validation | validity",
        "hook_mutation_flow | mutat",
        "hook_mutation_flow | mutate",
        "hook_mutation_flow | mutation",
        "html_app_shell | app",
        "html_app_shell | body",
        "html_app_shell | main",
        "html_app_shell | module",
        "html_app_shell | mount",
        "html_app_shell | root",
        "html_app_shell | script",
        "html_app_shell | shell",
        "indexing_storage | indexer",
        "indexing_storage | indexers",
        "indexing_storage | snapshot",
        "indexing_storage | snapshots",
        "indexing_storage | symbol",
        "indexing_storage | symbols",
        "request_entrypoint | asgi",
        "request_entrypoint | route",
        "request_entrypoint | router",
        "request_entrypoint | routers",
        "request_entrypoint | routes",
        "request_entrypoint | servlet",
        "request_entrypoint | wsgi",
        "search_entrypoint | main",
        "shell_completion | alias",
        "shell_completion | compgen",
        "shell_completion | complete",
        "shell_completion | completion",
        "shell_function_dispatch | case",
        "shell_function_dispatch | command",
        "shell_function_dispatch | commands",
        "shell_function_dispatch | dispatch",
        "shell_function_dispatch | dispatcher",
        "shell_function_dispatch | exec",
        "shell_function_dispatch | execut",
        "shell_function_dispatch | execute",
        "shell_function_dispatch | execution",
        "shell_function_dispatch | run",
        "shell_function_dispatch | use",
        "shell_installer_bootstrap | bootstrap",
        "shell_installer_bootstrap | download",
        "shell_installer_bootstrap | install",
        "shell_installer_bootstrap | setup",
        "shell_installer_bootstrap | source",
        "shell_installer_bootstrap | sources",
    ];

    /// Every word any predicate in this crate reads, so the sweep below covers the whole vocabulary
    /// the tables are written in rather than a sample of it. Held to the carriers' own source by
    /// `the_one_word_sweep_covers_every_word_the_carriers_match_on`, so it cannot fall behind them.
    fn evidence_vocabulary() -> Vec<&'static str> {
        vec![
            "request",
            "requests",
            "route",
            "routes",
            "router",
            "routing",
            "controller",
            "handler",
            "handlers",
            "endpoint",
            "server",
            "middleware",
            "http",
            "protocol",
            "dispatch",
            "dispatcher",
            "wsgi",
            "asgi",
            "rack",
            "servlet",
            "gateway",
            "client",
            "clients",
            "instance",
            "factory",
            "session",
            "transport",
            "adapter",
            "adapters",
            "send",
            "fetch",
            "url",
            "connection",
            "response",
            "socket",
            "stream",
            "writer",
            "sink",
            "buffer",
            "sender",
            "task",
            "delegate",
            "index",
            "indexer",
            "indexing",
            "symbol",
            "symbols",
            "snapshot",
            "workspace",
            "candidate",
            "catalog",
            "ingest",
            "crawl",
            "serve",
            "daemon",
            "bootstrap",
            "startup",
            "init",
            "main",
            "listen",
            "listener",
            "event",
            "events",
            "loop",
            "poll",
            "select",
            "epoll",
            "reactor",
            "tick",
            "network",
            "networking",
            "query",
            "wire",
            "command",
            "commands",
            "table",
            "exec",
            "execute",
            "search",
            "searcher",
            "grep",
            "match",
            "matcher",
            "args",
            "argv",
            "arg",
            "worker",
            "printer",
            "log",
            "logger",
            "logging",
            "record",
            "records",
            "site",
            "page",
            "post",
            "layout",
            "template",
            "document",
            "collection",
            "static",
            "theme",
            "asset",
            "renderer",
            "generator",
            "build",
            "builder",
            "pipeline",
            "process",
            "run",
            "start",
            "generate",
            "render",
            "write",
            "read",
            "output",
            "emit",
            "map",
            "mapper",
            "mapping",
            "typemap",
            "plan",
            "execution",
            "config",
            "profile",
            "option",
            "options",
            "format",
            "formatter",
            "fmt",
            "vformat",
            "error",
            "throw",
            "fail",
            "assert",
            "fallback",
            "panic",
            "cache",
            "caches",
            "helper",
            "key",
            "keys",
            "serialize",
            "mutate",
            "mutation",
            "form",
            "validate",
            "validity",
            "guard",
            "submit",
            "required",
            "pattern",
            "min",
            "max",
            "install",
            "setup",
            "download",
            "completion",
            "prepare",
            "finalize",
            "materialize",
            "interceptor",
            "storage",
            "persist",
            "manifest",
            "get",
            "put",
            "patch",
            "delete",
            "head",
            "https",
            "transports",
            "sends",
            "finaliz",
            "finalis",
            "prepar",
            "to",
            "body",
            "responses",
            "bytes",
            "settle",
            "settled",
            "transform",
            "materiali",
            "use",
            "serializ",
            "serialis",
            "hash",
            "stable",
            "stringify",
            "helpers",
            "provider",
            "context",
            "state",
            "store",
            "make",
            "creat",
            "mutat",
            "app",
            "root",
            "shell",
            "module",
            "script",
            "mount",
            "import",
            "forward",
            "keyframes",
            "animation",
            "animated",
            "transition",
            "duration",
            "delay",
            "iteration",
            "fillmode",
            "forms",
            "fieldset",
            "validation",
            "validations",
            "validates",
            "invalid",
            "constraint",
            "constraints",
            "guards",
            "preventdefault",
            "minlength",
            "maxlength",
            "inputtype",
            "inputmode",
            "validator",
            "customvalid",
            "checkvalid",
            "reportvalid",
            "submits",
            "submitt",
            "source",
            "case",
            "compgen",
            "complete",
            "alias",
            "segment",
            "reads",
            "writes",
            "emits",
            "flush",
            "skip",
            "copy",
            "copyto",
            "readfrom",
            "writeto",
            "logs",
            "loggers",
            "handle",
            "add",
            "create",
            "push",
            "pop",
            "remove",
            "set",
            "register",
            "batch",
            "interface",
            "sites",
            "pages",
            "posts",
            "layouts",
            "templates",
            "documents",
            "collections",
            "themes",
            "assets",
            "file",
            "files",
            "html",
            "phases",
            "writ",
            "outputs",
            "renders",
            "maps",
            "mappers",
            "mappings",
            "execut",
            "formats",
            "formatters",
            "formatting",
            "printf",
            "sprintf",
            "fprintf",
            "arguments",
            "value",
            "values",
            "err",
            "indexes",
            "indexed",
            "indexers",
            "snapshots",
            "workspaces",
            "candidates",
            "catalogs",
            "routers",
            "controllers",
            "endpoints",
            "servers",
            "cgi",
            "fastcgi",
            "instances",
            "factories",
            "sessions",
            "urls",
            "connections",
            "sockets",
            "streams",
            "tasks",
            "delegates",
            "loops",
            "polling",
            "kqueue",
            "queries",
            "searches",
            "matchers",
            // The IO peers a byte buffer sits between, the record-pipeline words a logging
            // framework qualifies its handler classes with, and the model words an object mapper
            // maps. Each became a way to satisfy a subsystem factor this round, so each has to be
            // swept as a name in its own right.
            "sources",
            "sinks",
            "byte",
            "io",
            "reader",
            "input",
            "pipe",
            "channel",
            "abstract",
            "base",
            "default",
            "generic",
            "null",
            "noop",
            "interfaces",
            "impl",
            "implementation",
            "processing",
            "processor",
            "processors",
            "entry",
            "entries",
            "formatted",
            "group",
            "chain",
            "stack",
            "type",
            "types",
            "object",
            "objects",
            "model",
            "models",
            "entity",
            "entities",
            "dto",
            "dtos",
            "member",
            "members",
            "property",
            "properties",
            "destination",
            "class",
            "classes",
            // A `site` beside a `map` is a sitemap, which the static-site carriers now reject. The
            // rejecting word has to be swept too: a word that *narrows* a carrier is a word whose
            // removal widens it, and the sweep is what would notice.
            "sitemap",
            "sitemaps",
        ]
    }

    /// The sweep is only as wide as the vocabulary it sweeps, so the vocabulary is checked against
    /// the carriers' own source instead of being maintained beside them by hand.
    ///
    /// Every bare lowercase word a carrier matches on is a word that can move a predicate on its
    /// own. A word present there and absent here is a blind spot in the sweep — and it would sit
    /// exactly where the next widening lands, because a widening *is* a word being added to a
    /// carrier.
    #[test]
    fn the_one_word_sweep_covers_every_word_the_carriers_match_on() {
        let vocabulary = evidence_vocabulary();
        let mut missing: Vec<String> = Vec::new();
        for line in include_str!("packet_evidence_carriers.rs").lines() {
            let code = line.trim_start();
            if code.starts_with("#[cfg(test)]") {
                // Below here are the carriers' own fixtures, whose literals are anchors rather
                // than needles.
                break;
            }
            if code.starts_with("//") {
                continue;
            }
            for (index, literal) in code.split('"').enumerate() {
                if index % 2 == 0
                    || literal.len() < 2
                    || !literal
                        .chars()
                        .all(|character| character.is_ascii_lowercase())
                    || vocabulary.contains(&literal)
                    || missing.iter().any(|word| word == literal)
                {
                    continue;
                }
                missing.push(literal.to_string());
            }
        }
        assert!(
            missing.is_empty(),
            "these words move a carrier but are never swept as a one-word symbol name, so the \
             recorded surface below cannot see what they admit: {missing:?}"
        );
    }

    /// Nouns from domains no flow in the tables covers.
    ///
    /// Crossing them with the evidence vocabulary builds the compound names a repository is
    /// actually full of — `FrameBuffer`, `sourceMapOptions`, `PaymentHandler`, `symbolFont` — which
    /// is the shape the bare-word sweep below cannot see.
    fn off_subject_qualifiers() -> Vec<&'static str> {
        vec![
            "Frame", "Road", "Payment", "Math", "Picker", "Chart", "Pixel", "Crash", "Coupon",
            "Rider",
        ]
    }

    /// One file per *surface class* a carrier branches on, in each directory that used to hand out
    /// a subsystem.
    ///
    /// This replaces a directory list crossed with `[".rs", ".ts"]` and a comment asserting that
    /// "`.ts` stands for every script surface". That was not true and the sweep could not see it:
    /// `is_form_validation_surface`, `is_markup_document` and `is_stylesheet` each branch on the
    /// extension into a *path-reading* code path, and `.vue` took the markup branch, so every
    /// symbol in a component library's `forms/` directory inherited the form factor from its folder
    /// while the sweep only ever asked about `.rs` and `.ts`. Nothing here is asserted any more:
    /// `the_sweeps_stand_for_every_surface_a_carrier_branches_on` checks each of these against
    /// every extension a carrier actually reads.
    ///
    /// Fewer directories than the bare-word sweep uses, and deliberately so: after
    /// `role_survives_without_its_path` no directory can grant a role at all, and the one invariant
    /// that still has to see every directory — `no_requirement_is_closed_by_an_unrelated_repository_symbol`
    /// — already crosses the full list. What is left that reads a path is the declared document
    /// exception, so this is the repository root, a plain source directory, and the folders that a
    /// carrier used to take a subsystem from: the static-site subject word, the `static/` spelling
    /// of it, a logging folder and a form example folder.
    fn sweep_surfaces() -> Vec<&'static str> {
        vec![
            "one.rs",
            "src/one.ts",
            "lib/site/one.rs",
            "lib/site/one.ts",
            "public/static/one.ts",
            "src/logging/one.rs",
            "examples/form/one.ts",
            "examples/form/one.vue",
            "examples/form/one.html",
            "src/styles/one.css",
            "scripts/install",
            "db/one.sql",
        ]
    }

    /// The file names the bare-word sweep crosses its (much longer) directory list with: one per
    /// surface class, for the same reason as above.
    fn sweep_surface_files() -> Vec<&'static str> {
        vec![
            "one.rs", "one.ts", "one.vue", "one.html", "one.css", "one.sh", "one.sql", "install",
        ]
    }

    /// Every extension a carrier reads, paired with the sweep file that stands for it.
    ///
    /// The left column is checked against the carriers' own source, so an extension added to a
    /// carrier without a representative here fails the gate rather than opening a hole in it. The
    /// right column is checked *behaviourally*: a representative that stopped behaving like the
    /// extension it stands for is exactly the `.vue`-as-markup defect, and it now fails.
    fn carrier_surface_classes() -> Vec<(&'static str, &'static str)> {
        vec![
            // Script surfaces. `.vue` and `.svelte` are here rather than with the markup documents
            // because the indexer blanks a single-file component's template and parses only its
            // `<script>` block, so their citations are identifiers.
            (".js", "one.ts"),
            (".mjs", "one.ts"),
            (".cjs", "one.ts"),
            (".ts", "one.ts"),
            (".mts", "one.ts"),
            (".cts", "one.ts"),
            (".jsx", "one.ts"),
            (".tsx", "one.ts"),
            (".vue", "one.ts"),
            (".svelte", "one.ts"),
            // Markup documents: ids, classes and attributes, with no identifier to scope by.
            (".html", "one.html"),
            (".htm", "one.html"),
            (".xhtml", "one.html"),
            // Stylesheets: selectors and at-rules.
            (".css", "one.css"),
            (".scss", "one.css"),
            (".sass", "one.css"),
            (".less", "one.css"),
            // Shell scripts, including the extensionless installer `is_shell_script` accepts.
            (".sh", "one.sh"),
            (".bash", "one.sh"),
            (".zsh", "one.sh"),
            ("install", "install"),
            // Schemas.
            (".sql", "one.sql"),
        ]
    }

    /// The compound names the sweep crosses each vocabulary word into: the word as the head of an
    /// off-subject compound, as its qualifier, and as a method on an off-subject receiver.
    ///
    /// Every name here carries exactly one vocabulary word. That is the limit of what this shape
    /// can see, and it is why `no_carrier_flow_closes_on_evidence_that_never_names_it` exists: a
    /// carrier needs an object word *and* a step verb, so a name with one vocabulary word in it can
    /// never reach the second factor. `ChartPipeline` closed nothing while `ChartPipeline.run`
    /// closed a static-site build, and this generator produced the first and never the second.
    fn compound_shapes_for(word: &str) -> Vec<String> {
        let mut capitalized = word.chars();
        let capitalized = match capitalized.next() {
            Some(first) => first.to_ascii_uppercase().to_string() + capitalized.as_str(),
            None => String::new(),
        };
        let mut names = Vec::new();
        for qualifier in off_subject_qualifiers() {
            names.push(format!("{qualifier}{capitalized}"));
            let mut lowered = qualifier.chars();
            let lowered = match lowered.next() {
                Some(first) => first.to_ascii_lowercase().to_string() + lowered.as_str(),
                None => String::new(),
            };
            names.push(format!("{word}{qualifier}"));
            names.push(format!("{lowered}{capitalized}"));
            names.push(format!("{qualifier}Kind.{word}"));
        }
        names
    }

    /// The surface each evidence word admits *as a token inside a name*, as `requirement | word`.
    ///
    /// `ONE_WORD_EVIDENCE_SURFACE` below records bare names, and for a long time its doc claimed to
    /// be "the exact surface on which an unrelated symbol can still be mistaken for evidence". It
    /// was not. Every predicate in this crate matches whole *tokens* inside a name, so a word that
    /// closes a requirement on its own closes it inside every compound that contains it: the entry
    /// `buffered_storage | buffer` read as "a class named `Buffer`" and meant `FrameBuffer`,
    /// `ZBuffer` and `RingBufferStats` as well. This list is the honest version — a word appears
    /// here when an off-subject compound built around it still closes the requirement.
    ///
    /// Each remaining entry is a word that *is* its requirement's subject in any compound: a
    /// `*Symbol*` is a symbol, a `use*` in camelCase is a React hook, a name with `dispatch` in it
    /// dispatches. Growth here is the signal to look at: a new entry means a predicate's two
    /// factors collapsed into one word that a compound noun can carry.
    ///
    /// Four of these are irreducible against a positive anchor that has the identical shape, and
    /// saying so is the point of recording them:
    ///
    /// - `client_interface_helpers | request` — the real anchor is `Axios.prototype.request`, whose
    ///   only client word *is* the verb. `FrameKind.request` cannot be told apart from it by name.
    /// - `buffered_storage | buffer` — a segment of a name that is nothing but "buffer" is the
    ///   buffer; okio's own wrapper is a function called `buffer`.
    /// - `hook_public_export | use` — `use` followed by a capital is the React hook convention, so
    ///   every custom hook in a front end reads as a public hook export.
    /// - `form_custom_validation | validity` — `validity` is both what makes an anchor a form
    ///   control's and what makes it the validation step, and the real anchors `setCustomValidity`
    ///   and `renderValidityMessage` carry no other form word. Its siblings
    ///   `form_native_constraints` and `form_submit_guard` still need a second word, so the flow as
    ///   a whole does not close on this.
    /// - `indexing_storage | symbol,snapshot,indexer` — the widest one left. `symbolFont`,
    ///   `SymbolPicker` and `SnapshotDiffViewer` close an indexer's persistence step. Requiring a
    ///   storage verb beside the subsystem word would close it, and would also make
    ///   `indexing_storage` unreachable for Sourcetrail, whose storage anchors are `IndexerJava`,
    ///   `StorageAccess` and `PersistentStorage`. A false negative on a live task is not a good
    ///   trade for this, so it stays open and named.
    ///
    /// The list tripled when the sweep started crossing the surface classes the carriers actually
    /// branch on instead of `.rs` and `.ts`. Nothing widened to cause that: every added entry is a
    /// stylesheet, markup, shell or schema family that has always been admitted and that a sweep
    /// asking only about two code extensions could not reach. They are the module header's declared
    /// exception seen from the outside — on those surfaces the anchor is a selector, an attribute or
    /// a statement, so a `css_animation_structure | keyframes` entry says "an at-rule in a
    /// stylesheet closes the animation structure step", which is what it is supposed to say and
    /// what no code identifier can imitate, because `is_stylesheet` will not have it.
    ///
    /// The exception is bounded by how much of a flow it can close, and that is what changed:
    /// `form_custom_validation` and `form_submit_guard` used to be admitted from a markup path too,
    /// so one HTML document under a `forms/` directory closed all three steps of form validation out
    /// of any three lexical hits in it. They read the form from the name now, on every surface, and
    /// only `form_native_constraints` — whose anchors genuinely are the document's own attributes —
    /// still takes the path.
    const COMPOUND_EVIDENCE_SURFACE: &[&str] = &[
        "buffered_storage | buffer",
        "client_interface_helpers | request",
        "css_animation_entrypoint | forward",
        "css_animation_entrypoint | import",
        "css_animation_entrypoint | use",
        "css_animation_structure | animated",
        "css_animation_structure | animation",
        "css_animation_structure | delay",
        "css_animation_structure | duration",
        "css_animation_structure | fillmode",
        "css_animation_structure | iteration",
        "css_animation_structure | keyframes",
        "css_animation_structure | transition",
        "form_custom_validation | validity",
        "form_native_constraints | inputmode",
        "form_native_constraints | inputtype",
        "form_native_constraints | max",
        "form_native_constraints | maxlength",
        "form_native_constraints | min",
        "form_native_constraints | minlength",
        "form_native_constraints | pattern",
        "form_native_constraints | required",
        "hook_mutation_flow | mutat",
        "hook_mutation_flow | mutate",
        "hook_mutation_flow | mutation",
        "hook_public_export | use",
        "html_app_shell | app",
        "html_app_shell | body",
        "html_app_shell | main",
        "html_app_shell | module",
        "html_app_shell | mount",
        "html_app_shell | root",
        "html_app_shell | script",
        "html_app_shell | shell",
        "indexing_storage | indexer",
        "indexing_storage | indexers",
        "indexing_storage | snapshot",
        "indexing_storage | snapshots",
        "indexing_storage | symbol",
        "indexing_storage | symbols",
        "request_entrypoint | asgi",
        "request_entrypoint | route",
        "request_entrypoint | router",
        "request_entrypoint | routers",
        "request_entrypoint | routes",
        "request_entrypoint | servlet",
        "request_entrypoint | wsgi",
        "shell_completion | alias",
        "shell_completion | compgen",
        "shell_completion | complete",
        "shell_completion | completion",
        "shell_function_dispatch | case",
        "shell_function_dispatch | command",
        "shell_function_dispatch | commands",
        "shell_function_dispatch | dispatch",
        "shell_function_dispatch | dispatcher",
        "shell_function_dispatch | exec",
        "shell_function_dispatch | execut",
        "shell_function_dispatch | execute",
        "shell_function_dispatch | execution",
        "shell_function_dispatch | run",
        "shell_function_dispatch | use",
        "shell_installer_bootstrap | bootstrap",
        "shell_installer_bootstrap | download",
        "shell_installer_bootstrap | install",
        "shell_installer_bootstrap | setup",
        "shell_installer_bootstrap | source",
        "shell_installer_bootstrap | sources",
    ];

    /// The requirements no anchor *name* can be wrong about, because on their own surface the file
    /// is the evidence.
    ///
    /// A `.css` citation is a selector or an at-rule; `css_structure` asks for a stylesheet and
    /// nothing more, so every name in the sweep closes it there. That is the module header's
    /// declared exception rather than a word deciding a step, and recording it as one line instead
    /// of 362 is what keeps the surface below readable. An entry appearing here means a requirement
    /// stopped reading its anchor at all.
    const FILE_IS_THE_EVIDENCE_SURFACE: &[&str] = &["css_structure | src/styles/one.css"];

    /// The sweeps cross names with one file per surface class rather than with every extension the
    /// carriers read. That reduction is only sound while each representative behaves exactly like
    /// the extensions it stands for, so it is checked rather than asserted in a comment.
    ///
    /// This is the test that would have caught the single-file-component defect. `.vue` was in
    /// `is_markup_document`, so a `.vue` anchor took the path-reading branch of
    /// `is_form_validation_surface` while `.ts` took the name-reading one — the two extensions
    /// disagreed, and the sweeps, which only ever asked about `.ts`, could not see it.
    #[test]
    fn the_sweeps_stand_for_every_surface_a_carrier_branches_on() {
        let classes = carrier_surface_classes();

        // Every extension literal in the carriers' own source has to be accounted for, so a new
        // surface added to a carrier fails here rather than opening a hole in the sweeps.
        let mut unaccounted: Vec<String> = Vec::new();
        for line in include_str!("packet_evidence_carriers.rs").lines() {
            let code = line.trim_start();
            if code.starts_with("#[cfg(test)]") {
                break;
            }
            if code.starts_with("//") {
                continue;
            }
            for (index, literal) in code.split('"').enumerate() {
                if index % 2 == 0 || !literal.starts_with('.') || literal.len() < 2 {
                    continue;
                }
                if !literal[1..].chars().all(|c| c.is_ascii_lowercase())
                    || classes.iter().any(|(extension, _)| *extension == literal)
                    || unaccounted.iter().any(|seen| seen == literal)
                {
                    continue;
                }
                unaccounted.push(literal.to_string());
            }
        }
        assert!(
            unaccounted.is_empty(),
            "a carrier branches on these file extensions but no sweep surface stands for them, so \
             the recorded surfaces cannot see what they admit: {unaccounted:?}"
        );

        let requirements = all_flow_requirements();
        let verdicts = |file: &str, name: &str, directory: &str, kind| {
            let citation = witness(name, &format!("{directory}{file}"), kind);
            requirements
                .iter()
                .filter(|requirement| requirement.evidence.citation_proves(&citation))
                .map(|requirement| requirement.id)
                .collect::<Vec<_>>()
        };
        for (extension, representative) in &classes {
            let file = if extension.starts_with('.') {
                format!("one{extension}")
            } else {
                (*extension).to_string()
            };
            for word in evidence_vocabulary() {
                for name in [word.to_string(), format!("Chart{word}.run")] {
                    // The root and the one directory a carrier is still allowed to read.
                    for directory in ["", "examples/form/"] {
                        for kind in [NodeKind::FUNCTION, NodeKind::CLASS] {
                            assert_eq!(
                                verdicts(&file, &name, directory, kind),
                                verdicts(representative, &name, directory, kind),
                                "`{directory}{file}` and the `{representative}` that stands for it \
                                 in the sweeps disagree about `{name}`: the sweeps are asking about \
                                 a surface class that no longer has one behaviour, which is how a \
                                 `.vue` anchor took a path-reading branch that `.ts` did not"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn compound_names_close_only_the_requirements_the_word_is_the_subject_of() {
        let requirements = all_flow_requirements();
        let mut live: Vec<String> = Vec::new();
        let mut checked = 0_u64;
        let mut names_swept = 0_usize;
        let mut closes_every_name: BTreeMap<String, usize> = BTreeMap::new();
        for word in evidence_vocabulary() {
            for name in compound_shapes_for(word) {
                for surface in sweep_surfaces() {
                    names_swept += 1;
                    let mut closed_here: Vec<&str> = Vec::new();
                    for kind in [NodeKind::FUNCTION, NodeKind::METHOD, NodeKind::CLASS] {
                        let citation = witness(&name, surface, kind);
                        for requirement in &requirements {
                            checked += 1;
                            if !requirement.evidence.citation_proves(&citation) {
                                continue;
                            }
                            if !closed_here.contains(&requirement.id) {
                                closed_here.push(requirement.id);
                            }
                        }
                    }
                    for id in closed_here {
                        *closes_every_name
                            .entry(format!("{id} | {surface}"))
                            .or_default() += 1;
                        let entry = format!("{id} | {word}");
                        if !live.contains(&entry) {
                            live.push(entry);
                        }
                    }
                }
            }
        }

        // A requirement that closes for *every* name on one surface is not admitting a word — it is
        // the declared exception where the file is the evidence. Recording 362 words for it would
        // bury the list this test exists to keep readable, so it is recorded once, as the surface.
        let names_per_surface = names_swept / sweep_surfaces().len();
        let mut by_file: Vec<String> = closes_every_name
            .into_iter()
            .filter(|(_, count)| *count == names_per_surface)
            .map(|(entry, _)| entry)
            .collect();
        by_file.sort();
        assert_eq!(
            by_file,
            FILE_IS_THE_EVIDENCE_SURFACE
                .iter()
                .map(|entry| (*entry).to_string())
                .collect::<Vec<_>>(),
            "the set of requirements proved by their file alone changed; each one is a place where \
             no anchor name can make a packet wrong, so the list belongs in the diff a reviewer \
             reads"
        );
        live.retain(|entry| {
            !by_file
                .iter()
                .any(|proved| proved.split(" | ").next() == entry.split(" | ").next())
        });
        assert!(
            checked >= 2_000_000,
            "the compound sweep must actually cross the vocabulary with off-subject qualifiers \
             (checked {checked})"
        );
        live.sort();

        let mut recorded = COMPOUND_EVIDENCE_SURFACE
            .iter()
            .map(|entry| (*entry).to_string())
            .collect::<Vec<_>>();
        recorded.sort();

        let added = live
            .iter()
            .filter(|entry| !recorded.contains(entry))
            .collect::<Vec<_>>();
        assert!(
            added.is_empty(),
            "an off-subject compound name now closes a requirement it did not before: one word \
             inside a name is deciding a step, which is the collapse this module exists to \
             prevent: {added:?}"
        );
        let removed = recorded
            .iter()
            .filter(|entry| !live.contains(entry))
            .collect::<Vec<_>>();
        assert!(
            removed.is_empty(),
            "these compound families no longer close their requirement; if that is intended, take \
             them out of the recorded surface in the diff a reviewer reads: {removed:?}"
        );
    }

    #[test]
    fn one_word_names_close_only_the_requirements_they_are_the_subject_of() {
        let requirements = all_flow_requirements();
        let directories = off_subject_directories();
        let mut live: Vec<String> = Vec::new();
        for word in evidence_vocabulary() {
            for directory in &directories {
                // Every directory, because a directory handing out a role is what this sweep looks
                // for, crossed with one file per surface class. It used to be `.rs` and `.ts` under
                // a comment claiming `.ts` stood for every script surface; that claim is now a
                // test rather than a comment, and it was false while `.vue` took the markup branch.
                // `STRUCT` is treated identically to `CLASS` by every predicate in the crate, and
                // the non-behavior kinds are crossed against the corpus above.
                for file in sweep_surface_files() {
                    for kind in [NodeKind::FUNCTION, NodeKind::METHOD, NodeKind::CLASS] {
                        let citation = witness(word, &format!("{directory}{file}"), kind);
                        for requirement in &requirements {
                            if !requirement.evidence.citation_proves(&citation) {
                                continue;
                            }
                            let entry = format!("{} | {word}", requirement.id);
                            if !live.contains(&entry) {
                                live.push(entry);
                            }
                        }
                    }
                }
            }
        }
        live.retain(|entry| {
            !FILE_IS_THE_EVIDENCE_SURFACE
                .iter()
                .any(|proved| proved.split(" | ").next() == entry.split(" | ").next())
        });
        live.sort();

        let mut recorded = ONE_WORD_EVIDENCE_SURFACE
            .iter()
            .map(|entry| (*entry).to_string())
            .collect::<Vec<_>>();
        recorded.sort();

        let added = live
            .iter()
            .filter(|entry| !recorded.contains(entry))
            .collect::<Vec<_>>();
        assert!(
            added.is_empty(),
            "a bare one-word symbol name now closes a requirement it did not before; a name with \
             no second word cannot say both which subsystem it is in and which step it is, so this \
             is a predicate whose two factors collapsed into one: {added:?}"
        );
        let removed = recorded
            .iter()
            .filter(|entry| !live.contains(entry))
            .collect::<Vec<_>>();
        assert!(
            removed.is_empty(),
            "these one-word names no longer close their requirement; if that is intended, take \
             them out of the recorded surface in the diff a reviewer reads: {removed:?}"
        );
    }

    /// The acceptance bar for this round, one case per carrier-backed flow.
    ///
    /// Each case is a real question that raises a whole flow, answered with citations drawn from
    /// somewhere else in the repository — the exact shape that reached a fully-closed *Sufficient*
    /// verdict in five of these six flows. `clampMin`/`validateCoupon`/`submitOrder` closed form
    /// validation; `AssetPipeline.run` + `Layout.render` closed a static-site build;
    /// `Logger.addRecord` + `PaymentHandler.process` closed a logger and its handler;
    /// `sourceMapOptions` + `RoadMapPlanner` closed an object mapper; `FrameBuffer` +
    /// `SegmentTree.read` closed buffered IO.
    ///
    /// The property is not "these names are rejected" — a fix that only rejects the reported names
    /// leaves the shape open, which is how this lane got here. It is that **each flow still names
    /// the step it has no evidence for**: the verdict has to be partial *and* the gap has to be the
    /// requirement the evidence genuinely fails to prove, not some other one.
    #[test]
    fn every_carrier_flow_reports_the_step_its_evidence_does_not_prove() {
        struct Case {
            flow: &'static str,
            prompt: &'static str,
            citations: Vec<AgentCitationDto>,
            expected_missing: &'static [&'static str],
        }

        let cases = vec![
            Case {
                flow: "form validation",
                prompt: "Explain how the form validation examples combine native HTML constraints \
                         with custom JavaScript validation and a submit guard.",
                citations: vec![
                    witness("clampMin", "src/forms/layout.ts", NodeKind::FUNCTION),
                    witness("validateCoupon", "src/forms/coupon.ts", NodeKind::FUNCTION),
                    witness("submitOrder", "src/forms/order.ts", NodeKind::FUNCTION),
                    witness(
                        "validationMinScore",
                        "src/auth/password.ts",
                        NodeKind::FUNCTION,
                    ),
                ],
                expected_missing: &[
                    "form_native_constraints",
                    "form_custom_validation",
                    "form_submit_guard",
                ],
            },
            Case {
                flow: "static-site build",
                prompt: "Trace how the static site build command creates a site and runs the read, \
                         generate, render, and write phases.",
                citations: vec![
                    witness("AssetPipeline.run", "src/build/assets.rb", NodeKind::METHOD),
                    witness(
                        "Layout.render",
                        "src/components/layout.tsx",
                        NodeKind::METHOD,
                    ),
                    witness("readFile", "src/assets/io.ts", NodeKind::FUNCTION),
                    witness(
                        "PostMortem.generate",
                        "src/crash/report.rb",
                        NodeKind::METHOD,
                    ),
                ],
                expected_missing: &["site_lifecycle", "site_terminal"],
            },
            Case {
                flow: "logger record + handler",
                prompt: "Explain how a logger turns a log call into a record object and passes it \
                         through handlers.",
                citations: vec![
                    witness(
                        "Logger.addRecord",
                        "src/logging/payments.php",
                        NodeKind::METHOD,
                    ),
                    witness(
                        "PaymentHandler.process",
                        "src/logging/payments.php",
                        NodeKind::METHOD,
                    ),
                    witness("handleClick", "src/logging/ui.php", NodeKind::FUNCTION),
                ],
                expected_missing: &["handler_processing"],
            },
            Case {
                flow: "object mapper configuration + execution",
                prompt: "Explain how mapper configuration and runtime mapper APIs cooperate to map \
                         source objects to destination objects through type map plans.",
                citations: vec![
                    witness(
                        "sourceMapOptions",
                        "src/build/config.ts",
                        NodeKind::FUNCTION,
                    ),
                    witness("RoadMapPlanner", "src/nav/planner.rs", NodeKind::STRUCT),
                    witness("HeatMapConfig", "src/charts/heat.ts", NodeKind::STRUCT),
                    witness("TileMapExecutor", "src/gfx/tiles.rs", NodeKind::STRUCT),
                ],
                expected_missing: &["mapper_config", "mapper_execution"],
            },
            Case {
                flow: "buffered io",
                prompt: "Explain how Buffer, Source, Sink, and buffered wrappers cooperate to move \
                         bytes through reads and writes.",
                citations: vec![
                    witness("FrameBuffer", "src/gfx/frame.cpp", NodeKind::STRUCT),
                    witness("SegmentTree.read", "src/algo/segtree.rs", NodeKind::METHOD),
                    witness("ZBuffer", "src/gfx/depth.cpp", NodeKind::STRUCT),
                    witness("RingBufferStats", "src/metrics/ring.rs", NodeKind::STRUCT),
                ],
                expected_missing: &["buffered_storage", "buffered_read_write"],
            },
            Case {
                flow: "runtime formatting",
                prompt: "Explain how formatting arguments become type-erased format args and reach \
                         the vformat error fallback path.",
                citations: vec![
                    witness("NumberFormatError", "src/num/parse.cc", NodeKind::STRUCT),
                    witness(
                        "formatCurrencyError",
                        "src/money/fmt.cc",
                        NodeKind::FUNCTION,
                    ),
                    witness("CliParseError", "src/cli/parse.cc", NodeKind::FUNCTION),
                ],
                expected_missing: &["format_arguments"],
            },
        ];

        assert_eq!(cases.len(), 6, "one case per carrier-backed flow");

        for case in cases {
            let requirements = packet_flow_requirements_for_terms(
                &packet_probe_terms(case.prompt),
                PacketTaskClassDto::DataFlow,
            );
            assert!(
                !requirements.is_empty(),
                "the {} prompt must raise its flow, or this case proves nothing",
                case.flow
            );
            for expected in case.expected_missing {
                assert!(
                    requirements.iter().any(|r| r.id == *expected),
                    "the {} prompt must raise {expected}, or the gap below is vacuous",
                    case.flow
                );
            }

            let missing = requirements
                .iter()
                .filter(|requirement| {
                    !case
                        .citations
                        .iter()
                        .any(|citation| requirement.evidence.citation_proves(citation))
                })
                .map(|requirement| requirement.id)
                .collect::<Vec<_>>();

            assert!(
                !missing.is_empty(),
                "the {} flow reports every step proved by citations that prove none of it: a \
                 false-safe sufficient verdict is the disqualifying class for this lane",
                case.flow
            );
            for expected in case.expected_missing {
                assert!(
                    missing.contains(expected),
                    "the {} flow is partial but does not name {expected} as the gap; it named \
                     {missing:?}. A verdict that is partial for the wrong reason still tells the \
                     caller the wrong thing about what was proved",
                    case.flow
                );
            }
        }
    }

    /// The packets that reached a fully-closed *Sufficient* verdict on evidence that proved none of
    /// the flow they answered, named one by one so they cannot come back quietly.
    ///
    /// Rejecting these names is not what makes the fix a fix — `no_carrier_flow_closes_on_evidence_that_never_names_it`
    /// is, because it generates the whole shape rather than the examples. This test is the other
    /// half of the bar: the verdict has to be partial *and* it has to name the step the evidence
    /// genuinely fails to prove. A packet that is partial for the wrong reason still tells the
    /// caller the wrong thing.
    #[test]
    fn the_false_safe_packets_are_partial_and_name_the_step_they_do_not_prove() {
        let site = "Trace how the static site build command creates a site and runs the read, \
                    generate, render, and write phases.";
        let form = "Explain how the form validation examples combine native HTML constraints with \
                    custom JavaScript validation and a submit guard.";

        let cases: Vec<(&str, &str, Vec<AgentCitationDto>, &[&str])> = vec![
            // The subject came from the `lib/site/` directory, so neither name had to say "site".
            (
                "the acceptance bar's own citations, one directory over",
                site,
                vec![
                    witness("AssetPipeline.run", "lib/site/assets.rb", NodeKind::METHOD),
                    witness("Layout.render", "lib/site/layout.tsx", NodeKind::METHOD),
                ],
                &["site_lifecycle", "site_terminal"],
            ),
            (
                "a build pipeline and a render pipeline in the site folder",
                site,
                vec![
                    witness("DataPipeline.run", "lib/site/gen.rb", NodeKind::METHOD),
                    witness("RenderPipeline.run", "lib/site/gen.rb", NodeKind::METHOD),
                    witness("Pipeline.run", "lib/site/gen.rb", NodeKind::METHOD),
                ],
                &["site_lifecycle", "site_terminal"],
            ),
            // `public/static/` was the second spelling of the site root, and bundled vendor
            // JavaScript is what actually lives there.
            (
                "bundled vendor script under the static asset root",
                site,
                vec![
                    witness(
                        "AssetPipeline.run",
                        "public/static/vendor.ts",
                        NodeKind::METHOD,
                    ),
                    witness("Layout.render", "public/static/vendor.ts", NodeKind::METHOD),
                ],
                &["site_lifecycle", "site_terminal"],
            ),
            (
                "a generator directory in a server-rendered application",
                site,
                vec![
                    witness("Pages.generate", "src/generator/pages.rb", NodeKind::METHOD),
                    witness("Pages.render", "src/generator/pages.rb", NodeKind::METHOD),
                ],
                &["site_lifecycle", "site_terminal"],
            ),
            // Two different generic web nouns, which is what survived taking the directory away.
            (
                "two generic web nouns and a step verb, filed nowhere near a site",
                site,
                vec![
                    witness(
                        "AssetCollection.process",
                        "src/ui/assets.ts",
                        NodeKind::METHOD,
                    ),
                    witness("PageTemplate.render", "src/ui/page.tsx", NodeKind::METHOD),
                    witness("ThemeGenerator.run", "src/ui/theme.ts", NodeKind::METHOD),
                    witness(
                        "LayoutRenderer.output",
                        "src/ui/layout.ts",
                        NodeKind::METHOD,
                    ),
                ],
                &["site_lifecycle", "site_terminal"],
            ),
            // A single-file component was a markup document, so its `forms/` folder answered the
            // form question for every symbol in the file. `validityWindow` still closes one step,
            // and is meant to: "validity" is the recorded `form_custom_validation | validity`
            // surface and it closes exactly the same step in the `.ts` next to it.
            (
                "one component's exports, when a component was markup",
                form,
                vec![
                    witness("clampMin", "src/forms/Widget.vue", NodeKind::FUNCTION),
                    witness("submitJob", "src/forms/Widget.vue", NodeKind::FUNCTION),
                    witness("validityWindow", "src/forms/Widget.vue", NodeKind::FUNCTION),
                ],
                &["form_native_constraints", "form_submit_guard"],
            ),
            (
                "components and a document across three form directories",
                form,
                vec![
                    witness("maxRetries", "src/forms/Card.vue", NodeKind::FUNCTION),
                    witness(
                        "submitTransaction",
                        "app/forms/Checkout.svelte",
                        NodeKind::FUNCTION,
                    ),
                    witness(
                        "setValidityPeriod",
                        "src/forms/page.html",
                        NodeKind::FUNCTION,
                    ),
                ],
                &["form_native_constraints", "form_submit_guard"],
            ),
        ];

        for (label, prompt, citations, expected_missing) in cases {
            let requirements = packet_flow_requirements_for_terms(
                &packet_probe_terms(prompt),
                PacketTaskClassDto::DataFlow,
            );
            assert!(
                !requirements.is_empty(),
                "{label}: the prompt must raise its flow, or this case proves nothing"
            );
            let missing = requirements
                .iter()
                .filter(|requirement| {
                    !citations
                        .iter()
                        .any(|citation| requirement.evidence.citation_proves(citation))
                })
                .map(|requirement| requirement.id)
                .collect::<Vec<_>>();
            assert_eq!(
                missing, expected_missing,
                "{label}: this packet reported a different set of open steps than the one its \
                 evidence actually leaves open"
            );
        }
    }

    /// One carrier-backed flow: the question that raises it, the words that put an anchor in it,
    /// the surfaces its evidence turns up on, and the steps that an anchor naming none of those
    /// words can still close.
    struct CarrierFlow {
        flow: &'static str,
        prompt: &'static str,
        /// Mirrors the subsystem list the flow's carriers read. Held to them by
        /// `no_carrier_flow_closes_on_evidence_that_never_names_it`: a word dropped from a carrier's
        /// subsystem list widens what closes without it, and the sweep is what notices.
        subject_words: &'static [&'static str],
        surfaces: &'static [&'static str],
        /// The requirements of this flow that a name carrying no subject word still closes, as
        /// `requirement | surface`. Every entry is a residual, stated rather than left to be found.
        closable_without_the_subject: &'static [&'static str],
    }

    fn carrier_flows() -> Vec<CarrierFlow> {
        vec![
            CarrierFlow {
                flow: "form validation",
                prompt: "Explain how the form validation examples combine native HTML constraints \
                         with custom JavaScript validation and a submit guard.",
                subject_words: &[
                    "form",
                    "forms",
                    "fieldset",
                    "validity",
                    "constraint",
                    "constraints",
                    "guard",
                    "guards",
                ],
                surfaces: &[
                    "src/one.ts",
                    "src/forms/one.ts",
                    "src/forms/one.vue",
                    "src/forms/one.html",
                ],
                // The module header's declared exception, and the whole of what is left of it: a
                // constraint attribute is the entire anchor a markup document yields, so under a
                // `forms/` path the file answers the form question for that one step. Its two
                // siblings are script behaviour and read the form from the name, so an HTML
                // document cannot close the flow by itself — it did while all three took the
                // exception, out of any three lexical hits in one file.
                //
                // `.vue` used to take the exception too, which is what let `clampMin`, `submitJob`
                // and `validityWindow` in a single component close the flow; a single-file
                // component's citations are `<script>` exports, so it behaves like the `.ts` beside
                // it and closes nothing here.
                closable_without_the_subject: &["form_native_constraints | src/forms/one.html"],
            },
            CarrierFlow {
                flow: "static-site build",
                prompt: "Trace how the static site build command creates a site and runs the read, \
                         generate, render, and write phases.",
                subject_words: &["site", "sites"],
                surfaces: &["lib/site/one.rs", "public/static/one.ts", "src/ui/one.tsx"],
                closable_without_the_subject: &[],
            },
            CarrierFlow {
                flow: "logger record + handler",
                prompt: "Explain how a logger turns a log call into a record object and passes it \
                         through handlers.",
                subject_words: &["log", "logs", "logger", "loggers", "logging"],
                surfaces: &["src/logging/one.rs", "src/one.ts"],
                // A record pipeline qualifies its handler classes structurally — `AbstractHandler`,
                // `HandlerInterface`, `ProcessorChain` — and that vocabulary is what separates them
                // from a `PaymentHandler`. It does not say "log", so a structurally-qualified
                // handler closes the dispatch step without one. Its sibling `logger_event` still
                // needs the log word, so the flow does not close on this.
                closable_without_the_subject: &[
                    "handler_processing | src/logging/one.rs",
                    "handler_processing | src/one.ts",
                ],
            },
            CarrierFlow {
                flow: "object mapper configuration + execution",
                prompt: "Explain how mapper configuration and runtime mapper APIs cooperate to map \
                         source objects to destination objects through type map plans.",
                subject_words: &["map", "maps", "mapper", "mappers", "mapping", "mappings"],
                surfaces: &["src/mapping/one.rs", "src/one.ts"],
                closable_without_the_subject: &[],
            },
            CarrierFlow {
                flow: "buffered io",
                prompt: "Explain how Buffer, Source, Sink, and buffered wrappers cooperate to move \
                         bytes through reads and writes.",
                subject_words: &["buffer"],
                surfaces: &["src/io/one.rs", "src/one.ts"],
                // The read/write step accepts a source, a sink or a stream beside its verb, because
                // that is what the operations of a byte pipeline are named for and requiring the
                // container in the name would make the step unreachable. The container step
                // `buffered_storage` still needs the buffer, so the flow does not close on this.
                closable_without_the_subject: &[
                    "buffered_read_write | src/io/one.rs",
                    "buffered_read_write | src/one.ts",
                ],
            },
            CarrierFlow {
                flow: "runtime formatting",
                prompt: "Explain how formatting arguments become type-erased format args and reach \
                         the vformat error fallback path.",
                subject_words: &[
                    "format",
                    "formats",
                    "formatter",
                    "formatters",
                    "formatting",
                    // `belongs_to_runtime_formatting` reads "format" as a *prefix*, so every
                    // vocabulary word that starts with it is a subject word. Leaving this one out
                    // showed up below as both steps of the flow closing without a subject, which is
                    // what this list being wrong is supposed to look like.
                    "formatted",
                    "fmt",
                    "vformat",
                    "printf",
                    "sprintf",
                    "fprintf",
                ],
                surfaces: &["src/fmt/one.rs", "src/one.ts"],
                closable_without_the_subject: &[],
            },
        ]
    }

    /// The names the two-factor sweep builds from an object word and a step verb.
    ///
    /// `compound_shapes_for` puts exactly one vocabulary word in a name, which is why the recorded
    /// surfaces could not see the shape that was actually evading them: a carrier asks for an object
    /// *and* a step, so one vocabulary word can never reach the second factor. `ChartPipeline`
    /// closed nothing and `ChartPipeline.run` closed a static-site build; only the second is a
    /// counter-example, and only this generator produces it.
    ///
    /// Three spellings, because they are not interchangeable. The dotted form puts the step in the
    /// terminal segment, which is what the HTTP-verb and single-token-segment tests read; the
    /// qualified form adds an off-subject noun so the name is one a real repository contains; the
    /// lower-camel form is the only one that can carry the `use` + capital hook convention.
    fn two_factor_shapes_for(object: &str, step: &str) -> [String; 3] {
        let capitalize = |word: &str| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
                None => String::new(),
            }
        };
        [
            format!("{}.{step}", capitalize(object)),
            format!("Chart{}.{step}", capitalize(object)),
            format!("{object}{}", capitalize(step)),
        ]
    }

    /// **No carrier-backed flow may be closed by citations that never name it.**
    ///
    /// This is the invariant the lane exists for, generated rather than exampled. For each flow it
    /// builds every object-word + step-verb name the evidence vocabulary can spell out of words
    /// that are *not* that flow's subject, files each one on the flow's own surfaces, and records
    /// which of the flow's steps such a name can close. At least one step must survive — a flow
    /// with none left reports `Sufficient` on a packet that proved nothing, which is the
    /// disqualifying class.
    ///
    /// Two shapes of defect this catches that nothing before it did. `AssetPipeline.run` and
    /// `Layout.render` under `lib/site/` closed the static-site flow completely: the subject came
    /// from the directory, so neither name needed to say "site". `AssetCollection.process` and
    /// `PageTemplate.render` closed it again after the directory was taken away, because two
    /// generic web nouns stood in for one specific one. Both are two-word-plus-verb shapes, and the
    /// one-vocabulary-word sweeps are structurally blind to them.
    ///
    /// `NodeKind::METHOD` alone, and provably so: every requirement in these six flows is a
    /// `CitedCarrier`, and every carrier's kind test is either `owns_behavior` — which accepts
    /// `FUNCTION`, `METHOD`, `CLASS` and `STRUCT` alike — or `FUNCTION | METHOD`. No carrier can
    /// accept a kind it rejects for a method, so widening the axis could only cost time.
    #[test]
    fn no_carrier_flow_closes_on_evidence_that_never_names_it() {
        let vocabulary = evidence_vocabulary();
        let flows = carrier_flows();
        assert_eq!(flows.len(), 6, "one entry per carrier-backed flow");

        let mut checked = 0_u64;
        for flow in &flows {
            let requirements = packet_flow_requirements_for_terms(
                &packet_probe_terms(flow.prompt),
                PacketTaskClassDto::DataFlow,
            );
            assert!(
                requirements.len() >= 2,
                "the {} prompt must raise its whole flow, or this proves nothing",
                flow.flow
            );
            for requirement in &requirements {
                assert!(
                    matches!(requirement.evidence, EvidencePredicate::CitedCarrier(_)),
                    "{} is not carrier-backed, so the METHOD-only kind axis below is no longer \
                     sound for the {} flow",
                    requirement.id,
                    flow.flow
                );
            }

            let off_subject = vocabulary
                .iter()
                .filter(|word| !flow.subject_words.contains(*word))
                .collect::<Vec<_>>();
            let mut closable: Vec<String> = Vec::new();
            for object in &off_subject {
                for step in &off_subject {
                    for name in two_factor_shapes_for(object, step) {
                        for surface in flow.surfaces {
                            let citation = witness(&name, surface, NodeKind::METHOD);
                            for requirement in &requirements {
                                checked += 1;
                                if !requirement.evidence.citation_proves(&citation) {
                                    continue;
                                }
                                let entry = format!("{} | {surface}", requirement.id);
                                if !closable.contains(&entry) {
                                    closable.push(entry);
                                }
                            }
                        }
                    }
                }
            }
            closable.sort();

            let mut recorded = flow
                .closable_without_the_subject
                .iter()
                .map(|entry| (*entry).to_string())
                .collect::<Vec<_>>();
            recorded.sort();
            assert_eq!(
                closable, recorded,
                "the {} flow's set of steps closable by a name that never mentions its subject \
                 changed. Every entry is evidence CodeStory would report as proving a step it does \
                 not prove, so the list belongs in the diff a reviewer reads",
                flow.flow
            );

            // The property that matters: whatever the residual above, some step of the flow still
            // has no off-subject name that closes it, so the flow can never report sufficient on
            // evidence that never names it.
            let unreachable = requirements
                .iter()
                .filter(|requirement| {
                    !closable
                        .iter()
                        .any(|entry| entry.starts_with(&format!("{} |", requirement.id)))
                })
                .map(|requirement| requirement.id)
                .collect::<Vec<_>>();
            assert!(
                !unreachable.is_empty(),
                "every step of the {} flow can be closed by a name that never mentions the flow's \
                 subject, so an off-subject packet reports Sufficient: the disqualifying class for \
                 this lane",
                flow.flow
            );
        }

        assert!(
            checked >= 5_000_000,
            "the two-factor sweep must actually cross the vocabulary with itself (checked \
             {checked})"
        );
    }

    /// No requirement — role-classified or carrier-backed — may be closed by a symbol that has
    /// nothing to do with it.
    ///
    /// Both halves of the tables are held to this. The earlier version of this invariant skipped
    /// every `CitedRoles` requirement on the grounds that the role classifier is coarse by design,
    /// which left exactly half the tables untested; running this corpus against them turned up
    /// acceptances in nine of them, all from the same cause. A role is not scoped to a flow, and
    /// much of the classifier reads the path, so `renderChart` under `src/views/` was a server's
    /// request entrypoint, `Store.delete` was an indexer's persistence step, and every symbol under
    /// `runtime/` was a runtime orchestration entrypoint for three different flows at once.
    #[test]
    fn no_requirement_is_closed_by_an_unrelated_repository_symbol() {
        let mut checked = 0;
        let corpus = unrelated_repository_symbols()
            .into_iter()
            .chain(generated_off_subject_symbols())
            .collect::<Vec<_>>();
        for requirement in all_flow_requirements() {
            for symbol in &corpus {
                checked += 1;
                assert!(
                    !requirement.evidence.citation_proves(symbol),
                    "requirement {} is closed by `{}` at `{}`, which has nothing to do with it: a \
                     predicate that accepts arbitrary repository symbols reports sufficient on \
                     packets that proved nothing",
                    requirement.id,
                    symbol.display_name,
                    symbol.file_path.as_deref().unwrap_or_default()
                );
            }
        }
        assert!(
            checked >= 4_000_000,
            "the negative corpus must actually be exercised against the tables (checked {checked})"
        );
    }

    /// The generated corpus has to keep covering the whole space as the tables change: a flow added
    /// without its directory reaching the corpus is a flow this invariant cannot see into.
    #[test]
    fn the_generated_corpus_covers_every_flows_own_directory() {
        let directories = off_subject_directories();
        for ((requirement_id, _), witness) in requirement_witnesses() {
            let path = witness.file_path.clone().unwrap_or_default();
            let directory = match path.rfind('/') {
                Some(index) => path[..index + 1].to_string(),
                None => String::new(),
            };
            assert!(
                directories.contains(&directory),
                "the corpus never places an off-subject symbol beside {requirement_id}'s own \
                 evidence in `{directory}`"
            );
        }
        assert!(
            off_subject_code_extensions().len() >= 8,
            "the corpus must cross its directories with the languages the witnesses use"
        );
        assert!(
            generated_off_subject_symbols().len() >= 2_000,
            "the generated corpus collapsed; it is meant to be a cross product, not a list"
        );
    }

    /// Stronger than the same-role test above: inside one flow, *no* requirement may be closed by
    /// another requirement's evidence, whatever roles the two wear. Roles were never the thing that
    /// separated requirements; their evidence is.
    #[test]
    fn no_requirement_in_a_flow_is_closed_by_another_requirements_witness() {
        let witnesses = requirement_witnesses();
        let witness_for = |requirement: &FlowRequirement| {
            let key = (requirement.id, requirement.role_id());
            witnesses
                .iter()
                .find(|(witness_key, _)| *witness_key == key)
                .map(|(_, citation)| citation.clone())
                .unwrap_or_else(|| panic!("missing witness for {key:?}"))
        };

        let mut checked_pairs = 0;
        for (group, requirements) in all_flow_requirement_groups() {
            for (index, left) in requirements.iter().enumerate() {
                for right in requirements.iter().skip(index + 1) {
                    if left.id == right.id {
                        continue;
                    }
                    checked_pairs += 1;
                    let left_witness = witness_for(left);
                    let right_witness = witness_for(right);
                    assert!(
                        !right.evidence.citation_proves(&left_witness),
                        "in flow {group}, the anchor proving {} also closes {}: one anchor must \
                         not close two requirements",
                        left.id,
                        right.id
                    );
                    assert!(
                        !left.evidence.citation_proves(&right_witness),
                        "in flow {group}, the anchor proving {} also closes {}: one anchor must \
                         not close two requirements",
                        right.id,
                        left.id
                    );
                }
            }
        }
        assert!(
            checked_pairs >= 40,
            "this invariant must actually be exercising carrier-backed requirement pairs (checked \
             {checked_pairs})"
        );
    }
}
