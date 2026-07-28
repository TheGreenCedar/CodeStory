//! Generic packet flow requirements shared by planning, probes, and sufficiency.

use crate::agent::packet_evidence_carriers::{
    citation_owns_buffer_read_write, citation_owns_buffer_storage,
    citation_owns_client_request_finalization, citation_owns_client_request_method,
    citation_owns_client_response_materialization, citation_owns_css_animation_entrypoint,
    citation_owns_css_animation_structure, citation_owns_css_structure,
    citation_owns_form_custom_validation, citation_owns_form_native_constraint,
    citation_owns_form_submit_guard, citation_owns_format_arguments, citation_owns_format_errors,
    citation_owns_hook_cache_helper, citation_owns_hook_key_serialization,
    citation_owns_hook_mutation_flow, citation_owns_hook_public_export,
    citation_owns_html_app_shell, citation_owns_log_handler_processing,
    citation_owns_log_record_creation, citation_owns_mapper_configuration,
    citation_owns_mapper_execution, citation_owns_shell_completion,
    citation_owns_shell_function_dispatch, citation_owns_shell_installer_bootstrap,
    citation_owns_site_lifecycle, citation_owns_site_terminal,
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
    /// Covered by a citation the evidence-role classifier already places in this part of the flow.
    CitedRoles(&'static [PacketEvidenceRole]),
    /// Covered by a citation that passes a structural ownership check, used where the evidence
    /// role is too coarse to separate a requirement from its siblings.
    CitedCarrier(fn(&AgentCitationDto) -> bool),
}

impl EvidencePredicate {
    pub(crate) fn citation_proves(self, citation: &AgentCitationDto) -> bool {
        match self {
            Self::CitedRoles(roles) => {
                packet_evidence_role(citation).is_some_and(|role| roles.contains(&role))
            }
            Self::CitedCarrier(carrier) => carrier(citation),
        }
    }
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
        evidence: EvidencePredicate::CitedRoles(&[
            PacketEvidenceRole::IndexingWorkQueue,
            PacketEvidenceRole::CommandEntrypoint,
            PacketEvidenceRole::RuntimeOrchestration,
        ]),
    },
    FlowRequirement {
        id: "indexing_storage",
        role: FlowRole::StateOrStorage,
        query_seeds: &["file discovery", "symbol extraction", "storage persistence"],
        coverage_mode: CoverageMode::AllowsSourceRange,
        evidence: EvidencePredicate::CitedRoles(&[
            PacketEvidenceRole::PersistenceAndSearchProjection,
            PacketEvidenceRole::SymbolExtraction,
            PacketEvidenceRole::SnapshotRefresh,
            PacketEvidenceRole::WorkspaceDiscoveryAndPlanning,
            PacketEvidenceRole::CandidateFileConstruction,
        ]),
    },
];

const SERVER_REQUEST_DISPATCH_FLOW: &[FlowRequirement] = &[
    FlowRequirement {
        id: "request_entrypoint",
        role: FlowRole::Registration,
        query_seeds: &["request entrypoint", "route registration"],
        coverage_mode: CoverageMode::RequiresResolvedSourceOrGraph,
        evidence: EvidencePredicate::CitedRoles(&[
            PacketEvidenceRole::RouteHandling,
            PacketEvidenceRole::AppServerRequestProtocol,
        ]),
    },
    FlowRequirement {
        id: "request_dispatch",
        role: FlowRole::Dispatch,
        query_seeds: &["request dispatch", "handler dispatch", "transport adapter"],
        coverage_mode: CoverageMode::RequiresResolvedSourceOrGraph,
        evidence: EvidencePredicate::CitedRoles(&[
            PacketEvidenceRole::RequestDispatch,
            PacketEvidenceRole::CommandDispatch,
            PacketEvidenceRole::RuntimeOrchestration,
        ]),
    },
    FlowRequirement {
        id: "request_terminal",
        role: FlowRole::TerminalBoundary,
        query_seeds: &["response finalization", "transport send"],
        coverage_mode: CoverageMode::AllowsSourceRange,
        evidence: EvidencePredicate::CitedRoles(&[
            PacketEvidenceRole::TransportAdapter,
            PacketEvidenceRole::EventOutputProcessing,
            PacketEvidenceRole::BufferedIo,
        ]),
    },
];

const CLIENT_REQUEST_DISPATCH_FLOW: &[FlowRequirement] = &[
    FlowRequirement {
        id: "request_entrypoint",
        role: FlowRole::Entrypoint,
        query_seeds: &["default instance", "request method", "request entrypoint"],
        coverage_mode: CoverageMode::RequiresResolvedSourceOrGraph,
        evidence: EvidencePredicate::CitedRoles(&[
            PacketEvidenceRole::ClientFactory,
            PacketEvidenceRole::CommandEntrypoint,
        ]),
    },
    FlowRequirement {
        id: "request_dispatch",
        role: FlowRole::Dispatch,
        query_seeds: &["request dispatch", "adapters", "transport adapter"],
        coverage_mode: CoverageMode::RequiresResolvedSourceOrGraph,
        evidence: EvidencePredicate::CitedRoles(&[PacketEvidenceRole::RequestDispatch]),
    },
    FlowRequirement {
        id: "request_terminal",
        role: FlowRole::TerminalBoundary,
        query_seeds: &["response finalization", "transport send"],
        coverage_mode: CoverageMode::AllowsSourceRange,
        evidence: EvidencePredicate::CitedRoles(&[PacketEvidenceRole::TransportAdapter]),
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
        evidence: EvidencePredicate::CitedRoles(&[
            PacketEvidenceRole::ClientFactory,
            PacketEvidenceRole::AppServerRequestProtocol,
            PacketEvidenceRole::CommandEntrypoint,
        ]),
    },
    FlowRequirement {
        id: "session_callbacks",
        role: FlowRole::Dispatch,
        query_seeds: &["session delegate callbacks", "data request validation"],
        coverage_mode: CoverageMode::AllowsSourceRange,
        evidence: EvidencePredicate::CitedRoles(&[
            PacketEvidenceRole::RequestDispatch,
            PacketEvidenceRole::EventLoop,
            PacketEvidenceRole::RouteHandling,
            PacketEvidenceRole::TransportAdapter,
        ]),
    },
];

const CLIENT_PUBLIC_FACADE_REQUIREMENT: FlowRequirement = FlowRequirement {
    id: "client_public_facade",
    role: FlowRole::Entrypoint,
    query_seeds: &["http top level helper", "public client facade"],
    coverage_mode: CoverageMode::RequiresResolvedSourceOrGraph,
    evidence: EvidencePredicate::CitedRoles(&[PacketEvidenceRole::ClientFactory]),
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
    evidence: EvidencePredicate::CitedRoles(&[
        PacketEvidenceRole::TransportAdapter,
        PacketEvidenceRole::RequestDispatch,
    ]),
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
    evidence: EvidencePredicate::CitedRoles(&[
        PacketEvidenceRole::CommandEntrypoint,
        PacketEvidenceRole::RuntimeOrchestration,
    ]),
};

const COMMAND_EVENT_LOOP_REQUIREMENT: FlowRequirement = FlowRequirement {
    id: "command_event_loop",
    role: FlowRole::Dispatch,
    query_seeds: &["event loop", "event loop source"],
    coverage_mode: CoverageMode::RequiresResolvedSourceOrGraph,
    evidence: EvidencePredicate::CitedRoles(&[PacketEvidenceRole::EventLoop]),
};

const COMMAND_NETWORK_INPUT_REQUIREMENT: FlowRequirement = FlowRequirement {
    id: "command_network_input",
    role: FlowRole::Dispatch,
    query_seeds: &["network input", "network command input"],
    coverage_mode: CoverageMode::RequiresResolvedSourceOrGraph,
    evidence: EvidencePredicate::CitedRoles(&[PacketEvidenceRole::NetworkCommandInput]),
};

const COMMAND_DISPATCH_REQUIREMENT: FlowRequirement = FlowRequirement {
    id: "command_dispatch",
    role: FlowRole::Dispatch,
    query_seeds: &["command dispatch", "command table dispatch"],
    coverage_mode: CoverageMode::RequiresResolvedSourceOrGraph,
    evidence: EvidencePredicate::CitedRoles(&[
        PacketEvidenceRole::CommandDispatch,
        PacketEvidenceRole::RequestDispatch,
    ]),
};

const SQL_SCHEMA_FLOW: &[FlowRequirement] = &[
    FlowRequirement {
        id: "sql_tables",
        role: FlowRole::StateOrStorage,
        query_seeds: &["sql table definitions", "CREATE TABLE"],
        coverage_mode: CoverageMode::AllowsLexicalSource,
        evidence: EvidencePredicate::CitedRoles(&[PacketEvidenceRole::SqlTableDefinition]),
    },
    FlowRequirement {
        id: "sql_relationships",
        role: FlowRole::Configuration,
        query_seeds: &["foreign key relationships", "schema constraints"],
        coverage_mode: CoverageMode::AllowsLexicalSource,
        evidence: EvidencePredicate::CitedRoles(&[PacketEvidenceRole::SqlRelationshipConstraint]),
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
        evidence: EvidencePredicate::CitedCarrier(citation_owns_format_errors),
    },
];

const SEARCH_EXECUTION_FLOW: &[FlowRequirement] = &[
    FlowRequirement {
        id: "search_entrypoint",
        role: FlowRole::Entrypoint,
        query_seeds: &["search entrypoint", "argument planning"],
        coverage_mode: CoverageMode::RequiresResolvedSourceOrGraph,
        evidence: EvidencePredicate::CitedRoles(&[
            PacketEvidenceRole::SearchDriver,
            PacketEvidenceRole::ArgumentPlanning,
            PacketEvidenceRole::CommandEntrypoint,
        ]),
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
        evidence: EvidencePredicate::CitedRoles(&[
            PacketEvidenceRole::SearchExecutionUnit,
            PacketEvidenceRole::CandidateFileConstruction,
        ]),
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
            (
                ("site_lifecycle", "entrypoint"),
                witness("Build.process", "lib/site/build.rb", NodeKind::METHOD),
            ),
            (
                ("site_terminal", "terminal_boundary"),
                witness("Renderer.render", "lib/site/renderer.rb", NodeKind::METHOD),
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
    /// `citation_owns_format_errors` matched any symbol whose name contained "error" anywhere in
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
        ]
    }

    #[test]
    fn no_requirement_is_closed_by_an_unrelated_repository_symbol() {
        let mut checked = 0;
        for requirement in all_flow_requirements() {
            // `CitedRoles` requirements delegate to the evidence-role classifier, which is coarse
            // on purpose: it answers "is this source evidence at all", not "does this prove my
            // step". The carriers are the per-requirement checks and the only predicates that
            // claim to separate one requirement from everything else, so they are what this
            // corpus holds to account.
            if !matches!(requirement.evidence, EvidencePredicate::CitedCarrier(_)) {
                continue;
            }
            for symbol in unrelated_repository_symbols() {
                checked += 1;
                assert!(
                    !requirement.evidence.citation_proves(&symbol),
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
            checked >= 300,
            "the negative corpus must actually be exercised against the tables (checked {checked})"
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
                    // Role-classified predicates are deliberately coarse; the carriers are the
                    // per-requirement checks, so they are what this invariant holds to account.
                    if !matches!(left.evidence, EvidencePredicate::CitedCarrier(_))
                        || !matches!(right.evidence, EvidencePredicate::CitedCarrier(_))
                    {
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
            checked_pairs >= 15,
            "this invariant must actually be exercising carrier-backed requirement pairs (checked \
             {checked_pairs})"
        );
    }
}
