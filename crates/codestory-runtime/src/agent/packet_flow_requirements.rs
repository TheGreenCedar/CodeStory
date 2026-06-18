//! Generic packet flow requirements shared by planning, probes, and sufficiency.

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
use codestory_contracts::api::PacketTaskClassDto;

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

#[derive(Debug, Clone, Copy)]
pub(crate) struct FlowRequirement {
    pub id: &'static str,
    pub role: FlowRole,
    pub query_seeds: &'static [&'static str],
    pub coverage_mode: CoverageMode,
}

impl FlowRequirement {
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
    if packet_terms_indicate_request_dispatch_flow(terms)
        || packet_terms_indicate_server_request_dispatch_flow(terms)
    {
        requirements.extend_from_slice(REQUEST_DISPATCH_FLOW);
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
    },
    FlowRequirement {
        id: "indexing_storage",
        role: FlowRole::StateOrStorage,
        query_seeds: &["file discovery", "symbol extraction", "storage persistence"],
        coverage_mode: CoverageMode::AllowsSourceRange,
    },
];

const REQUEST_DISPATCH_FLOW: &[FlowRequirement] = &[
    FlowRequirement {
        id: "request_entrypoint",
        role: FlowRole::Registration,
        query_seeds: &["request entrypoint", "route registration"],
        coverage_mode: CoverageMode::RequiresResolvedSourceOrGraph,
    },
    FlowRequirement {
        id: "request_dispatch",
        role: FlowRole::Dispatch,
        query_seeds: &["request dispatch", "handler dispatch", "transport adapter"],
        coverage_mode: CoverageMode::RequiresResolvedSourceOrGraph,
    },
    FlowRequirement {
        id: "request_terminal",
        role: FlowRole::TerminalBoundary,
        query_seeds: &["response finalization", "transport send"],
        coverage_mode: CoverageMode::AllowsSourceRange,
    },
];

const URL_SESSION_FLOW: &[FlowRequirement] = &[
    FlowRequirement {
        id: "session_request",
        role: FlowRole::Entrypoint,
        query_seeds: &["session request creation", "request task resume"],
        coverage_mode: CoverageMode::RequiresResolvedSourceOrGraph,
    },
    FlowRequirement {
        id: "session_callbacks",
        role: FlowRole::Dispatch,
        query_seeds: &["session delegate callbacks", "data request validation"],
        coverage_mode: CoverageMode::AllowsSourceRange,
    },
];

const CLIENT_PUBLIC_FACADE_REQUIREMENT: FlowRequirement = FlowRequirement {
    id: "client_public_facade",
    role: FlowRole::Entrypoint,
    query_seeds: &["http top level helper", "public client facade"],
    coverage_mode: CoverageMode::RequiresResolvedSourceOrGraph,
};

const CLIENT_INTERFACE_HELPERS_REQUIREMENT: FlowRequirement = FlowRequirement {
    id: "client_interface_helpers",
    role: FlowRole::Entrypoint,
    query_seeds: &["client convenience method", "client interface helper"],
    coverage_mode: CoverageMode::RequiresResolvedSourceOrGraph,
};

const CLIENT_REQUEST_FINALIZATION_REQUIREMENT: FlowRequirement = FlowRequirement {
    id: "client_request_finalization",
    role: FlowRole::TransformOrValidate,
    query_seeds: &["request finalization", "transport-ready request object"],
    coverage_mode: CoverageMode::RequiresResolvedSourceOrGraph,
};

const CLIENT_TRANSPORT_SEND_REQUIREMENT: FlowRequirement = FlowRequirement {
    id: "client_transport_send",
    role: FlowRole::Dispatch,
    query_seeds: &["transport send", "client send implementation"],
    coverage_mode: CoverageMode::RequiresResolvedSourceOrGraph,
};

const CLIENT_RESPONSE_MATERIALIZATION_REQUIREMENT: FlowRequirement = FlowRequirement {
    id: "client_response_materialization",
    role: FlowRole::TerminalBoundary,
    query_seeds: &["request response", "response stream boundary"],
    coverage_mode: CoverageMode::RequiresResolvedSourceOrGraph,
};

const HOOK_PUBLIC_EXPORT_REQUIREMENT: FlowRequirement = FlowRequirement {
    id: "hook_public_export",
    role: FlowRole::Entrypoint,
    query_seeds: &["public hook export", "hook argument wrapper"],
    coverage_mode: CoverageMode::AllowsSourceRange,
};

const HOOK_KEY_SERIALIZATION_REQUIREMENT: FlowRequirement = FlowRequirement {
    id: "hook_key_serialization",
    role: FlowRole::TransformOrValidate,
    query_seeds: &["key serialization", "serialize hook key"],
    coverage_mode: CoverageMode::AllowsSourceRange,
};

const HOOK_CACHE_HELPER_REQUIREMENT: FlowRequirement = FlowRequirement {
    id: "hook_cache_helper",
    role: FlowRole::StateOrStorage,
    query_seeds: &["cache helper", "cache state helper"],
    coverage_mode: CoverageMode::AllowsSourceRange,
};

const HOOK_MUTATION_FLOW_REQUIREMENT: FlowRequirement = FlowRequirement {
    id: "hook_mutation_flow",
    role: FlowRole::Dispatch,
    query_seeds: &["mutation helper", "mutate dispatch"],
    coverage_mode: CoverageMode::AllowsSourceRange,
};

const COMMAND_SERVER_BOOTSTRAP_REQUIREMENT: FlowRequirement = FlowRequirement {
    id: "command_server_bootstrap",
    role: FlowRole::Entrypoint,
    query_seeds: &["server bootstrap", "command server entrypoint"],
    coverage_mode: CoverageMode::RequiresResolvedSourceOrGraph,
};

const COMMAND_EVENT_LOOP_REQUIREMENT: FlowRequirement = FlowRequirement {
    id: "command_event_loop",
    role: FlowRole::Dispatch,
    query_seeds: &["event loop", "event loop source"],
    coverage_mode: CoverageMode::RequiresResolvedSourceOrGraph,
};

const COMMAND_NETWORK_INPUT_REQUIREMENT: FlowRequirement = FlowRequirement {
    id: "command_network_input",
    role: FlowRole::Dispatch,
    query_seeds: &["network input", "network command input"],
    coverage_mode: CoverageMode::RequiresResolvedSourceOrGraph,
};

const COMMAND_DISPATCH_REQUIREMENT: FlowRequirement = FlowRequirement {
    id: "command_dispatch",
    role: FlowRole::Dispatch,
    query_seeds: &["command dispatch", "command table dispatch"],
    coverage_mode: CoverageMode::RequiresResolvedSourceOrGraph,
};

const SQL_SCHEMA_FLOW: &[FlowRequirement] = &[
    FlowRequirement {
        id: "sql_tables",
        role: FlowRole::StateOrStorage,
        query_seeds: &["sql table definitions", "CREATE TABLE"],
        coverage_mode: CoverageMode::AllowsLexicalSource,
    },
    FlowRequirement {
        id: "sql_relationships",
        role: FlowRole::Configuration,
        query_seeds: &["foreign key relationships", "schema constraints"],
        coverage_mode: CoverageMode::AllowsLexicalSource,
    },
];

const HTML_CSS_FLOW: &[FlowRequirement] = &[
    FlowRequirement {
        id: "html_app_shell",
        role: FlowRole::Entrypoint,
        query_seeds: &["html app shell", "module script entry"],
        coverage_mode: CoverageMode::AllowsLexicalSource,
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
    },
];

const CSS_ANIMATION_FLOW: &[FlowRequirement] = &[
    FlowRequirement {
        id: "css_animation_entrypoint",
        role: FlowRole::Entrypoint,
        query_seeds: &["animation stylesheet entrypoint", "css animation imports"],
        coverage_mode: CoverageMode::AllowsLexicalSource,
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
    },
    FlowRequirement {
        id: "form_custom_validation",
        role: FlowRole::TransformOrValidate,
        query_seeds: &["custom validation", "custom error rendering"],
        coverage_mode: CoverageMode::AllowsLexicalSource,
    },
    FlowRequirement {
        id: "form_submit_guard",
        role: FlowRole::TerminalBoundary,
        query_seeds: &["submit prevent default", "submit invalid guard"],
        coverage_mode: CoverageMode::AllowsLexicalSource,
    },
];

const SHELL_INSTALL_FLOW: &[FlowRequirement] = &[
    FlowRequirement {
        id: "shell_installer_bootstrap",
        role: FlowRole::Entrypoint,
        query_seeds: &["shell installer bootstrap", "install download helpers"],
        coverage_mode: CoverageMode::AllowsLexicalSource,
    },
    FlowRequirement {
        id: "shell_function_dispatch",
        role: FlowRole::Dispatch,
        query_seeds: &["shell function dispatch", "conditional version use"],
        coverage_mode: CoverageMode::AllowsLexicalSource,
    },
    FlowRequirement {
        id: "shell_completion",
        role: FlowRole::TerminalBoundary,
        query_seeds: &["shell completion"],
        coverage_mode: CoverageMode::DiagnosticOnly,
    },
];

const BUFFERED_IO_FLOW: &[FlowRequirement] = &[
    FlowRequirement {
        id: "buffered_storage",
        role: FlowRole::StateOrStorage,
        query_seeds: &["buffer storage", "source sink buffer"],
        coverage_mode: CoverageMode::AllowsSourceRange,
    },
    FlowRequirement {
        id: "buffered_read_write",
        role: FlowRole::Dispatch,
        query_seeds: &["source read buffer", "sink write buffer"],
        coverage_mode: CoverageMode::RequiresResolvedSourceOrGraph,
    },
];

const LOG_HANDLER_FLOW: &[FlowRequirement] = &[
    FlowRequirement {
        id: "logger_event",
        role: FlowRole::Entrypoint,
        query_seeds: &["logger record", "record creation"],
        coverage_mode: CoverageMode::RequiresResolvedSourceOrGraph,
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
    },
];

const SITE_BUILD_FLOW: &[FlowRequirement] = &[
    FlowRequirement {
        id: "site_lifecycle",
        role: FlowRole::Entrypoint,
        query_seeds: &["site build lifecycle", "site process phases"],
        coverage_mode: CoverageMode::RequiresResolvedSourceOrGraph,
    },
    FlowRequirement {
        id: "site_terminal",
        role: FlowRole::TerminalBoundary,
        query_seeds: &["read generate render write", "renderer render"],
        coverage_mode: CoverageMode::AllowsSourceRange,
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
    },
    FlowRequirement {
        id: "mapper_execution",
        role: FlowRole::Dispatch,
        query_seeds: &["mapping execution plan", "source destination mapping"],
        coverage_mode: CoverageMode::RequiresResolvedSourceOrGraph,
    },
];

const RUNTIME_FORMATTING_FLOW: &[FlowRequirement] = &[
    FlowRequirement {
        id: "format_arguments",
        role: FlowRole::TransformOrValidate,
        query_seeds: &["format arguments", "format output"],
        coverage_mode: CoverageMode::RequiresResolvedSourceOrGraph,
    },
    FlowRequirement {
        id: "format_errors",
        role: FlowRole::ErrorOrFallback,
        query_seeds: &["format error", "error formatting"],
        coverage_mode: CoverageMode::AllowsSourceRange,
    },
];

const SEARCH_EXECUTION_FLOW: &[FlowRequirement] = &[
    FlowRequirement {
        id: "search_entrypoint",
        role: FlowRole::Entrypoint,
        query_seeds: &["search entrypoint", "argument planning"],
        coverage_mode: CoverageMode::RequiresResolvedSourceOrGraph,
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
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::packet_terms::packet_probe_terms;

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
}
